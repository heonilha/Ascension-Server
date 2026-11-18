#include "game/Monsters.h"

#include <algorithm>
#include <cmath>
#include <numbers>

namespace asc::game
{
    using proto::MonsterState;
    using T = MonsterTuning;

    namespace
    {
        constexpr float kPi = std::numbers::pi_v<float>;
        constexpr float kDegrees = 180.f / kPi;

        float Distance(float ax, float ay, float bx, float by) { return std::hypot(ax - bx, ay - by); }
    }

    MonsterField::MonsterField(const CollisionMap* map, std::uint32_t seed, std::uint32_t nowMs)
        : map_(map), rng_(seed), startMs_(nowMs), reinforcementReadyMs_(nowMs + 2500), refillReadyMs_(nowMs + 1000), eliteReadyMs_(nowMs + 60000)
    {
    }

    float MonsterField::Random(float low, float high)
    {
        return std::uniform_real_distribution<float>(low, high)(rng_);
    }

    bool MonsterField::Overlaps(float x, float y, float radius) const
    {
        if (std::abs(x) > kArenaHalfWidth - radius || std::abs(y) > kArenaHalfDepth - radius) return true;
        return map_ && map_->SolidOverlap({x, y}, radius);
    }

    bool MonsterField::SweepBlocked(float x0, float y0, float x1, float y1, float radius) const
    {
        const float length = Distance(x0, y0, x1, y1);
        const int steps = std::max(1, static_cast<int>(std::ceil(length / 10.f)));
        for (int i = 1; i <= steps; ++i)
        {
            const float t = static_cast<float>(i) / static_cast<float>(steps);
            if (Overlaps(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, radius)) return true;
        }
        return false;
    }

    void MonsterField::PushOut(float& x, float& y, float radius) const
    {
        if (map_)
        {
            move::Vec2 p{x, y};
            if (map_->PushOutSolid(p, radius)) x = p.x, y = p.y;
        }
        x = std::clamp(x, -kArenaHalfWidth + radius, kArenaHalfWidth - radius);
        y = std::clamp(y, -kArenaHalfDepth + radius, kArenaHalfDepth - radius);
    }

    bool MonsterField::LineOfSight(float x0, float y0, float x1, float y1) const
    {
        return !map_ || !map_->LineBlocked({x0, y0, move::kGroundZ}, {x1, y1, move::kGroundZ});
    }

    Monster& MonsterField::SpawnAt(std::uint8_t kind, float x, float y, std::uint32_t nowMs)
    {
        Monster monster;
        monster.id = nextMonsterId_++;
        if (nextMonsterId_ == 0) nextMonsterId_ = 1; // 0은 "몬스터 아님"으로 쓴다
        monster.kind = std::min<std::uint8_t>(kind, 2);
        monster.hp = T::kMaxHp[monster.kind];
        monster.x = x;
        monster.y = y;
        monster.deadlineMs = nowMs;
        ++totalSpawned_;
        return monsters_.emplace_back(std::move(monster));
    }

    void MonsterField::Update(std::uint32_t nowMs, float dt, std::span<const MonsterPlayer> players, MonsterEvents& events)
    {
        if (spawning_) UpdateSpawning(nowMs, players);
        for (Monster& monster : monsters_) UpdateMonster(monster, nowMs, dt, players, events);
        Separate();
        UpdateWaves(nowMs, dt, events); // 몬스터를 지울 수 있으므로 마지막에 한다
    }

    // ---- 증원 ----

    void MonsterField::UpdateSpawning(std::uint32_t nowMs, std::span<const MonsterPlayer> players)
    {
        if (std::ranges::none_of(players, &MonsterPlayer::alive)) return;
        const float elapsed = static_cast<float>(nowMs - startMs_) / 1000.f;
        if (nowMs >= reinforcementReadyMs_)
        {
            SpawnBatch(std::min(4 + static_cast<int>(elapsed / 60.f), 8), false, nowMs, players);
            reinforcementReadyMs_ = nowMs + static_cast<std::uint32_t>(std::max(1.5f, 2.5f - elapsed / 240.f) * 1000.f);
        }
        if (nowMs >= refillReadyMs_)
        {
            const int target = std::min(12 + static_cast<int>(elapsed / 30.f) * 2, 28);
            SpawnBatch(std::clamp(target - static_cast<int>(monsters_.size()), 0, 4), false, nowMs, players);
            refillReadyMs_ = nowMs + 1000;
        }
        if (nowMs >= eliteReadyMs_)
        {
            SpawnBatch(1, true, nowMs, players);
            eliteReadyMs_ = nowMs + 60000;
        }
    }

    void MonsterField::SpawnBatch(int count, bool elite, std::uint32_t nowMs, std::span<const MonsterPlayer> players)
    {
        std::vector<const MonsterPlayer*> alive;
        for (const MonsterPlayer& player : players)
        {
            if (player.alive) alive.push_back(&player);
        }
        if (alive.empty()) return;
        const float elapsed = static_cast<float>(nowMs - startMs_) / 1000.f;

        for (int i = 0; i < count; ++i)
        {
            if (monsters_.size() >= proto::kMaxMonsters) break;
            for (int attempt = 0; attempt < 150; ++attempt)
            {
                // 플레이어 한 명을 골라 그 둘레 6.5~12.5m 고리 안에 놓는다.
                const MonsterPlayer& anchor = *alive[std::uniform_int_distribution<std::size_t>(0, alive.size() - 1)(rng_)];
                const float angle = Random(0.f, 2.f * kPi), distance = Random(T::kSpawnMinDistance, T::kSpawnMaxDistance);
                const float x = anchor.x + distance * std::cos(angle), y = anchor.y + distance * std::sin(angle);
                if (std::abs(x) > kArenaHalfWidth - 100.f || std::abs(y) > kArenaHalfDepth - 100.f) continue;
                if (Overlaps(x, y, T::kSpawnClearRadius)) continue;
                if (std::ranges::any_of(alive, [&](const MonsterPlayer* p) { return Distance(x, y, p->x, p->y) < T::kSpawnPlayerDistance; })) continue;
                if (std::ranges::any_of(monsters_, [&](const Monster& m) { return Distance(x, y, m.x, m.y) < T::kSpawnSeparation; })) continue;

                const std::uint8_t kind = elite ? 2 : Random(0.f, 1.f) < (elapsed < 120.f ? .25f : .5f) ? 1 : 0;
                SpawnAt(kind, x, y, nowMs);
                break;
            }
        }
    }

    // ---- 몬스터 한 마리 ----

    void MonsterField::BeginTell(Monster& monster, const MonsterPlayer& target, std::uint32_t startMs, MonsterEvents& events)
    {
        const float toX = target.x - monster.x, toY = target.y - monster.y, length = std::hypot(toX, toY);
        if (length > 1e-3f)
        {
            monster.dirX = toX / length;
            monster.dirY = toY / length;
        }
        monster.yaw = std::atan2(monster.dirY, monster.dirX) * kDegrees;
        monster.phase = MonsterState::Tell;
        monster.deadlineMs = startMs + (monster.kind == 1 ? T::kChargeTellMs : monster.strike == 1 ? T::kArcSecondTellMs : T::kArcTellMs);
        events.tells.push_back({monster.id, monster.kind, false, monster.x, monster.y, monster.yaw, startMs, monster.deadlineMs});
    }

    void MonsterField::Chase(Monster& monster, float toX, float toY, float dt)
    {
        const float length = std::hypot(toX, toY);
        if (length < 1e-3f) return;
        const float desiredX = toX / length, desiredY = toY / length;
        // 정면부터 좌우로 넓혀 가며 막히지 않은 첫 방향으로 간다.
        for (const float degrees : {0.f, 45.f, -45.f, 90.f, -90.f, 135.f, -135.f})
        {
            const float c = std::cos(degrees / kDegrees), s = std::sin(degrees / kDegrees);
            const float dirX = desiredX * c - desiredY * s, dirY = desiredX * s + desiredY * c;
            if (SweepBlocked(monster.x, monster.y, monster.x + dirX * T::kSteerProbe, monster.y + dirY * T::kSteerProbe, T::kSteerRadius)) continue;
            monster.x += dirX * T::kChaseSpeed * dt;
            monster.y += dirY * T::kChaseSpeed * dt;
            monster.yaw = std::atan2(dirY, dirX) * kDegrees;
            break;
        }
        PushOut(monster.x, monster.y, T::kRadius);
    }

    void MonsterField::UpdateMonster(Monster& monster, std::uint32_t nowMs, float dt, std::span<const MonsterPlayer> players, MonsterEvents& events)
    {
        // 경직: AI를 멈추고 남은 넉백 거리를 경직 시간에 나눠 민다. 지형에 막히면 거기서 끝난다.
        if (nowMs < monster.staggerUntilMs)
        {
            if (monster.knockRemaining > 0.f)
            {
                const float remainingTime = static_cast<float>(monster.staggerUntilMs - nowMs) / 1000.f;
                const float step = std::min(monster.knockRemaining, monster.knockRemaining * dt / std::max(dt, remainingTime + dt));
                const float x = monster.x + monster.knockX * step, y = monster.y + monster.knockY * step;
                if (Overlaps(x, y, T::kRadius)) monster.knockRemaining = 0.f;
                else
                {
                    monster.x = x;
                    monster.y = y;
                    monster.knockRemaining -= step;
                }
            }
            return;
        }
        if (monster.knockRemaining > 0.f)
        {
            // 경직이 틱 사이에 끝났다: 남은 넉백을 마저 민다(20Hz 틱에서 마지막 조각을 잃지 않게).
            const float x = monster.x + monster.knockX * monster.knockRemaining, y = monster.y + monster.knockY * monster.knockRemaining;
            if (!Overlaps(x, y, T::kRadius)) monster.x = x, monster.y = y;
            monster.knockRemaining = 0.f;
        }

        // 가장 가까운 살아 있는 플레이어
        const MonsterPlayer* target = nullptr;
        float distance = 1e30f;
        for (const MonsterPlayer& player : players)
        {
            const float d = Distance(player.x, player.y, monster.x, monster.y);
            if (player.alive && d < distance) target = &player, distance = d;
        }
        if (!target)
        {
            if (monster.phase == MonsterState::Tell) events.tells.push_back({monster.id, monster.kind, true});
            monster.phase = MonsterState::Idle;
            monster.strike = 0;
            return;
        }
        if (monster.phase == MonsterState::Idle) monster.phase = MonsterState::Chase;
        const float toX = target->x - monster.x, toY = target->y - monster.y;

        switch (monster.phase)
        {
        case MonsterState::Chase:
        {
            const float trigger = monster.kind == 1 ? T::kChargeTrigger : T::kArcTrigger;
            if (distance < trigger && LineOfSight(monster.x, monster.y, target->x, target->y)) BeginTell(monster, *target, nowMs, events);
            else Chase(monster, toX, toY, dt);
            break;
        }
        case MonsterState::Tell:
            if (nowMs < monster.deadlineMs) break;
            // 예고가 끝난 정확한 시각(deadlineMs) 기준으로 다음 단계를 잡는다. 틱(50ms) 경계에 밀리지 않는다.
            if (monster.kind == 1)
            {
                monster.phase = MonsterState::Dash;
                monster.deadlineMs += T::kDashMs;
                monster.dashHits.clear();
            }
            else if (monster.kind == 2 && monster.strike == 0)
            {
                monster.strike = 1;
                BeginTell(monster, *target, monster.deadlineMs, events); // 지휘관: 첫 베기 직후 두 번째 예고
            }
            else
            {
                monster.phase = MonsterState::Recover;
                monster.deadlineMs += T::kArcRecoverMs[monster.kind];
                monster.strike = 0;
            }
            break;
        case MonsterState::Dash:
        {
            // 10cm씩 나눠 움직여 벽을 뚫지 않고, 지나가며 닿은 플레이어를 놓치지 않는다.
            const float total = T::kDashSpeed * dt;
            const int steps = std::max(1, static_cast<int>(std::ceil(total / 10.f)));
            for (int i = 0; i < steps && monster.phase == MonsterState::Dash; ++i)
            {
                const float x = monster.x + monster.dirX * total / static_cast<float>(steps), y = monster.y + monster.dirY * total / static_cast<float>(steps);
                if (Overlaps(x, y, T::kRadius))
                {
                    monster.phase = MonsterState::Recover;
                    monster.deadlineMs = nowMs + T::kDashRecoverMs;
                    break;
                }
                monster.x = x;
                monster.y = y;
                for (const MonsterPlayer& player : players)
                {
                    if (!player.alive || std::ranges::contains(monster.dashHits, player.id)) continue;
                    if (Distance(player.x, player.y, x, y) >= T::kDashHitDistance || !LineOfSight(x, y, player.x, player.y)) continue;
                    monster.dashHits.push_back(player.id);
                    events.dashTouches.push_back({monster.id, player.id, x, y});
                }
            }
            if (monster.phase == MonsterState::Dash && nowMs >= monster.deadlineMs)
            {
                monster.phase = MonsterState::Recover;
                monster.deadlineMs = nowMs + T::kDashRecoverMs;
            }
            break;
        }
        case MonsterState::Recover:
            if (monster.kind != 2 && distance > 170.f) Chase(monster, toX, toY, dt);
            if (nowMs >= monster.deadlineMs) monster.phase = MonsterState::Chase;
            break;
        default:
            break;
        }
    }

    // 서로 겹친 몬스터를 떼어 놓는다. 예고·돌진·경직 중인 몬스터는 자리를 지키고 다른 쪽이 비킨다.
    void MonsterField::Separate()
    {
        const auto Movable = [](const Monster& m) {
            return m.knockRemaining <= 0.f && (m.phase == MonsterState::Chase || m.phase == MonsterState::Recover || m.phase == MonsterState::Idle);
        };
        constexpr float minDistance = T::kRadius * 2.f;
        for (std::size_t i = 0; i < monsters_.size(); ++i)
        {
            for (std::size_t j = i + 1; j < monsters_.size(); ++j)
            {
                Monster& a = monsters_[i];
                Monster& b = monsters_[j];
                float dx = b.x - a.x, dy = b.y - a.y;
                if (std::abs(dx) >= minDistance || std::abs(dy) >= minDistance) continue;
                float d = std::hypot(dx, dy);
                if (d >= minDistance) continue;
                const bool moveA = Movable(a), moveB = Movable(b);
                if (!moveA && !moveB) continue;
                if (d < 1e-3f) dx = 1.f, dy = 0.f, d = 1.f; // 완전히 겹쳤으면 아무 방향으로나
                const float push = minDistance - std::min(d, minDistance), shareA = moveA ? (moveB ? .5f : 1.f) : 0.f;
                a.x -= dx / d * push * shareA;
                a.y -= dy / d * push * shareA;
                b.x += dx / d * push * (1.f - shareA);
                b.y += dy / d * push * (1.f - shareA);
                if (moveA) PushOut(a.x, a.y, T::kRadius);
                if (moveB) PushOut(b.x, b.y, T::kRadius);
            }
        }
    }

    bool MonsterField::Damage(std::uint16_t monsterId, std::uint32_t attackerId, float damage, float pushX, float pushY, std::uint32_t nowMs,
                              MonsterEvents& events)
    {
        const auto it = std::ranges::find(monsters_, monsterId, &Monster::id);
        if (it == monsters_.end()) return false;
        Monster& monster = *it;
        monster.hp = std::max(0.f, monster.hp - damage);
        events.hits.push_back({monster.id, attackerId, damage, monster.hp, monster.x, monster.y});

        if (monster.hp <= 0.f)
        {
            if (monster.phase == MonsterState::Tell) events.tells.push_back({monster.id, monster.kind, true});
            monsters_.erase(it);
            ++totalKilled_;
            return true;
        }
        if (nowMs < monster.reactionReadyMs) return true; // 방금 경직됐으면 피해만 받는다

        const bool elite = monster.kind == 2;
        const std::uint32_t duration = elite ? T::kEliteStaggerMs : T::kStaggerMs;
        monster.staggerUntilMs = nowMs + duration;
        monster.reactionReadyMs = nowMs + T::kReactionCooldownMs;
        const float length = std::hypot(pushX, pushY);
        monster.knockX = length > 1e-3f ? pushX / length : 0.f;
        monster.knockY = length > 1e-3f ? pushY / length : 0.f;
        monster.knockRemaining = elite ? T::kEliteKnockback : std::clamp(damage * T::kKnockbackPerDamage, T::kKnockbackMin, T::kKnockbackMax);

        const bool telling = monster.phase == MonsterState::Tell;
        if (!elite)
        {
            // 검병·폭주 망령: 하던 공격이 끊기고 경직이 끝나면 다시 추적한다.
            if (telling) events.tells.push_back({monster.id, monster.kind, true});
            monster.phase = MonsterState::Recover;
            monster.deadlineMs = monster.staggerUntilMs;
            monster.strike = 0;
        }
        else
        {
            // 지휘관: 끊기지 않고 경직 시간만큼 늦어진다. 예고 중이면 새 명중 시각으로 다시 알린다.
            monster.deadlineMs += duration;
            if (telling)
            {
                events.tells.push_back({monster.id, monster.kind, true});
                events.tells.push_back({monster.id, monster.kind, false, monster.x, monster.y, monster.yaw, nowMs, monster.deadlineMs});
            }
        }
        return true;
    }

    // ---- 검기 ----

    std::uint32_t MonsterField::SpawnWave(std::uint32_t ownerId, float x, float y, float yawDegrees, float damage, float radius, int pierces,
                                          std::uint32_t lifeMs, std::uint32_t nowMs)
    {
        Wave wave;
        wave.id = nextWaveId_++;
        wave.ownerId = ownerId;
        wave.x = x;
        wave.y = y;
        wave.dirX = std::cos(yawDegrees / kDegrees);
        wave.dirY = std::sin(yawDegrees / kDegrees);
        wave.damage = damage;
        wave.radius = radius;
        wave.pierces = std::max(1, pierces);
        wave.endMs = nowMs + lifeMs;
        waves_.push_back(std::move(wave));
        return waves_.back().id;
    }

    void MonsterField::UpdateWaves(std::uint32_t nowMs, float dt, MonsterEvents& events)
    {
        struct Contact
        {
            std::uint16_t monsterId;
            float entry; // 이번 구간에서 검기가 그 몬스터에 닿기 시작하는 거리
        };
        std::vector<Contact> contacts;

        for (std::size_t index = 0; index < waves_.size();)
        {
            Wave& wave = waves_[index];
            // 이번 틱에 갈 구간. 벽이나 맵 끝에 막히면 그 앞에서 멈춘다.
            float length = T::kWaveSpeed * dt;
            bool blocked = false;
            for (float s = 10.f; s <= length; s += 10.f)
            {
                const float x = wave.x + wave.dirX * s, y = wave.y + wave.dirY * s;
                const bool outside = std::abs(x) > kArenaHalfWidth || std::abs(y) > kArenaHalfDepth;
                if (outside || (map_ && map_->Blocked({x, y, move::kGroundZ}, T::kWaveWallRadius)))
                {
                    blocked = true;
                    length = s - 10.f;
                    break;
                }
            }
            const float endX = wave.x + wave.dirX * length, endY = wave.y + wave.dirY * length;

            // 구간에 걸친 몬스터를 가까운 순서로. 이미 맞힌 몬스터와 기둥 뒤 몬스터는 뺀다.
            contacts.clear();
            const float reach = wave.radius + T::kWaveHitPadding;
            for (const Monster& monster : monsters_)
            {
                if (std::ranges::contains(wave.hitMonsters, monster.id)) continue;
                const float offsetX = monster.x - wave.x, offsetY = monster.y - wave.y;
                const float along = offsetX * wave.dirX + offsetY * wave.dirY;
                const float nearest = std::clamp(along, 0.f, length);
                const float gapX = offsetX - wave.dirX * nearest, gapY = offsetY - wave.dirY * nearest;
                if (gapX * gapX + gapY * gapY > reach * reach || !LineOfSight(wave.x, wave.y, monster.x, monster.y)) continue;
                const float sideSquared = std::max(0.f, offsetX * offsetX + offsetY * offsetY - along * along);
                contacts.push_back({monster.id, std::clamp(along - std::sqrt(std::max(0.f, reach * reach - sideSquared)), 0.f, length)});
            }
            std::ranges::sort(contacts, [](const Contact& a, const Contact& b) { return a.entry == b.entry ? a.monsterId < b.monsterId : a.entry < b.entry; });

            bool spent = false;
            float stopX = endX, stopY = endY;
            const std::uint32_t waveId = wave.id, ownerId = wave.ownerId;
            const float damage = wave.damage, dirX = wave.dirX, dirY = wave.dirY, startX = wave.x, startY = wave.y;
            for (const Contact& contact : contacts)
            {
                waves_[index].hitMonsters.push_back(contact.monsterId);
                Damage(contact.monsterId, ownerId, damage, dirX, dirY, nowMs, events);
                if (--waves_[index].pierces <= 0)
                {
                    spent = true;
                    stopX = startX + dirX * contact.entry;
                    stopY = startY + dirY * contact.entry;
                    break;
                }
            }

            if (spent || blocked)
            {
                events.waveEnds.push_back({waveId, stopX, stopY});
                waves_.erase(waves_.begin() + static_cast<std::ptrdiff_t>(index));
                continue;
            }
            waves_[index].x = endX;
            waves_[index].y = endY;
            if (nowMs >= waves_[index].endMs)
            {
                waves_.erase(waves_.begin() + static_cast<std::ptrdiff_t>(index)); // 수명이 다함: 클라이언트도 lifeMs로 안다
                continue;
            }
            ++index;
        }
    }
}
