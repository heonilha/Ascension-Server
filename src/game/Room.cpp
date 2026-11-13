#include "game/Room.h"

#include "core/Clock.h"
#include "core/Log.h"
#include "game/CombatRules.h"
#include "net/Session.h"

#include <algorithm>
#include <cmath>
#include <numbers>

namespace asc::game
{
    using namespace proto;

    namespace
    {
        const char* ModeName(RoomMode mode) { return mode == RoomMode::PvP ? "PvP" : "PvE"; }
    }

    Room::Room(asio::io_context& io, RoomMode mode, std::uint32_t teamCode, RoomOptions options)
        : strand_(asio::make_strand(io)), tickTimer_(strand_), mode_(mode), rules_(RulesFor(mode)), teamCode_(teamCode),
          options_(options)
    {
    }

    void Room::Start()
    {
        asio::dispatch(strand_, [self = shared_from_this()] {
            if (self->running_) return;
            self->running_ = true;
            self->nextSlamMs_ = ServerTimeMs() + self->options_.pveSlamIntervalMs;
            if (self->mode_ == RoomMode::PvE && self->options_.pveMonsters)
            {
                const std::uint32_t nowMs = ServerTimeMs();
                self->monsters_ = std::make_unique<MonsterField>(self->options_.map.get(), self->teamCode_ * 2654435761u ^ nowMs, nowMs);
            }
            asio::co_spawn(self->strand_, self->TickLoop(self), asio::detached);
            Log("방 {} 생성 ({}, 무적 {}ms, 되감기 상한 {}ms{})", self->teamCode_, ModeName(self->mode_), self->rules_.iframeMs,
                self->rules_.rewindCapMs, self->monsters_ ? ", 몬스터 있음" : "");
        });
    }

    void Room::Stop()
    {
        asio::post(strand_, [self = shared_from_this()] {
            self->running_ = false;
            self->tickTimer_.cancel();
            self->members_.clear();
            self->attacks_.clear();
            self->monsters_.reset();
            Log("방 {} 닫힘", self->teamCode_);
        });
    }

    void Room::Enter(std::shared_ptr<net::Session> session)
    {
        asio::post(strand_, [self = shared_from_this(), session = std::move(session)]() mutable {
            self->DoEnter(std::move(session));
        });
    }

    void Room::Leave(std::uint32_t playerId)
    {
        asio::post(strand_, [self = shared_from_this(), playerId] { self->DoLeave(playerId); });
    }

    void Room::Input(std::uint32_t playerId, const C_Input& packet, std::vector<InputCommand> commands)
    {
        asio::post(strand_, [self = shared_from_this(), playerId, packet, commands = std::move(commands)] {
            self->DoInput(playerId, packet, commands);
        });
    }

    void Room::Dodge(std::uint32_t playerId, const C_Dodge& packet)
    {
        asio::post(strand_, [self = shared_from_this(), playerId, packet] { self->DoDodge(playerId, packet); });
    }

    void Room::Attack(std::uint32_t playerId, const C_Attack& packet)
    {
        asio::post(strand_, [self = shared_from_this(), playerId, packet] { self->DoAttack(playerId, packet); });
    }

    void Room::Respawn(std::uint32_t playerId)
    {
        asio::post(strand_, [self = shared_from_this(), playerId] { self->DoRespawn(playerId); });
    }

    void Room::ReportRtt(std::uint32_t playerId, std::uint32_t rttMs)
    {
        asio::post(strand_, [self = shared_from_this(), playerId, rttMs] {
            if (Member* member = self->Find(playerId)) member->rttMs = std::min<std::uint32_t>(rttMs, 5000);
        });
    }

    void Room::DoEnter(std::shared_ptr<net::Session> session)
    {
        if (Find(session->Id())) return;
        const int slot = FreeSlot();
        if (slot < 0)
        {
            // RoomManager가 정원을 먼저 확인하므로 여기 오면 버그다.
            Log("방 {} 빈 자리 없음 (세션 {})", teamCode_, session->Id());
            return;
        }

        Member member;
        member.session = session;
        member.playerId = session->Id();
        member.name = session->Name();
        member.slot = slot;
        member.position = SpawnPosition(mode_, slot);
        member.sim.x = member.position.x;
        member.sim.y = member.position.y;
        member.creditTime = Clock::now();
        member.history.push_back({ServerTimeMs(), member.position});

        // 새로 온 사람에게: 입장 결과 + 판정 규칙 + 이미 있던 사람들
        std::vector<PlayerInfo> others;
        for (const Member& existing : members_) others.push_back(ToInfo(existing));

        auto reply = Make<S_EnterRoom>();
        reply.result = EnterResult::Ok;
        reply.teamCode = teamCode_;
        reply.mode = mode_;
        reply.floor = 1;
        reply.spawnPosition = member.position;
        reply.spawnYaw = 0.f;
        reply.capacity = CapacityFor(mode_);
        reply.rules = rules_;
        reply.serverTimeMs = ServerTimeMs();
        session->Send(net::ToBuffer(reply, std::span<const PlayerInfo>(others)));

        // 이미 있던 사람들에게: 새로 온 사람
        auto joined = Make<S_PlayerJoined>();
        joined.player = ToInfo(member);
        Broadcast(net::ToBuffer(joined));

        members_.push_back(std::move(member));
        Log("방 {} 입장: {} (슬롯 {}, 인원 {}/{})", teamCode_, session->Name(), slot, members_.size(), CapacityFor(mode_));
    }

    void Room::DoLeave(std::uint32_t playerId)
    {
        const auto it = std::ranges::find(members_, playerId, &Member::playerId);
        if (it == members_.end()) return;
        Log("방 {} 퇴장: {}", teamCode_, it->name);
        members_.erase(it);

        auto left = Make<S_PlayerLeft>();
        left.playerId = playerId;
        Broadcast(net::ToBuffer(left));
    }

    void Room::DoInput(std::uint32_t playerId, const C_Input& packet, const std::vector<InputCommand>& commands)
    {
        Member* member = Find(playerId);
        if (!member) return;

        // 처리권: 실제 시간만큼 쌓인다. 입력을 더 빨리 보내도(속도 조작) 초당 약 62틱(60 × 1.03) 넘게 움직일 수 없다.
        const auto now = Clock::now();
        member->inputCredits = std::min(kMaxInputBacklog,
                                        member->inputCredits + std::chrono::duration<double>(now - member->creditTime).count() * move::kTickRate * kInputRateTolerance);
        member->creditTime = now;

        static const move::World kEmptyWorld;
        const move::World& world = options_.map ? options_.map->World() : kEmptyWorld;
        std::uint32_t dropped = 0;
        for (std::size_t i = 0; i < commands.size(); ++i)
        {
            const std::uint32_t sequence = packet.firstSequence + static_cast<std::uint32_t>(i);
            if (member->hasInput && sequence <= member->lastInputSequence) continue; // 이미 처리한 번호
            member->hasInput = true;
            member->lastInputSequence = sequence;
            if (member->inputCredits < 1.0)
            {
                ++dropped; // 버린 입력도 번호는 처리한 것으로 친다. 클라이언트는 확정 상태로 재조정된다
                continue;
            }
            member->inputCredits -= 1.0;
            const InputCommand& c = commands[i];
            move::Step(member->sim, {c.moveX, c.moveY, c.flags, c.dodgeX, c.dodgeY}, world);
        }
        if (!options_.map)
        {
            // 지형을 끈 시험 실행: 맵 범위만 지킨다
            member->sim.x = std::clamp(member->sim.x, -kArenaHalfWidth + move::kRadius, kArenaHalfWidth - move::kRadius);
            member->sim.y = std::clamp(member->sim.y, -kArenaHalfDepth + move::kRadius, kArenaHalfDepth - move::kRadius);
        }

        if (dropped > 0)
        {
            member->droppedInputs += dropped;
            if (now - member->lastDropLog > std::chrono::seconds(1))
            {
                member->lastDropLog = now;
                Log("방 {} 입력 초과로 버림: {} (누적 {}개)", teamCode_, member->name, member->droppedInputs);
            }
        }

        member->position = {member->sim.x, member->sim.y, move::kGroundZ};
        member->velocity = {member->sim.vx, member->sim.vy, 0.f};
        if (std::isfinite(packet.yaw)) member->yaw = packet.yaw;
        member->state = packet.state;
        member->dirty = true;
        member->ackDirty = true;

        // 판정 때 "명중 시각에 그 플레이어가 있던 곳"을 찾기 위해 도착 시각과 함께 남긴다.
        const std::uint32_t nowMs = ServerTimeMs();
        member->history.push_back({nowMs, member->position});
        while (member->history.size() > 1 && nowMs - member->history.front().serverMs > kHistoryMs) member->history.pop_front();
    }

    void Room::DoDodge(std::uint32_t playerId, const C_Dodge& packet)
    {
        Member* member = Find(playerId);
        if (!member || member->dead) return;

        const std::uint32_t arrivalMs = ServerTimeMs();
        const std::uint32_t allowed = AllowedRewindMs(rules_, member->rttMs);
        const std::uint32_t acceptedMs = ClampPressTime(packet.pressServerMs, arrivalMs, allowed);

        // 재사용 대기는 인정한 시각끼리 비교한다. 시각을 앞당겨도 직전 회피보다 앞설 수 없다.
        if (!member->dodges.empty() && acceptedMs + kCooldownSlackMs < member->dodges.back().serverMs + rules_.dodgeCooldownMs)
        {
            Log("방 {} 회피 거부(재사용 대기): {} (직전 {}ms, 이번 {}ms)", teamCode_, member->name, member->dodges.back().serverMs, acceptedMs);
            return;
        }

        member->dodges.push_back({acceptedMs, arrivalMs - acceptedMs});
        while (member->dodges.size() > 4) member->dodges.pop_front();
        const int requested = static_cast<int>(arrivalMs) - static_cast<int>(packet.pressServerMs);
        Log("방 {} 회피: {} (요청 되감기 {}ms → 인정 {}ms, 허용 {}ms)", teamCode_, member->name, requested, arrivalMs - acceptedMs, allowed);
    }

    void Room::DoAttack(std::uint32_t playerId, const C_Attack& packet)
    {
        if (!std::isfinite(packet.yaw)) return;
        Member* member = Find(playerId);
        if (!member || member->dead) return;
        const std::uint32_t nowMs = ServerTimeMs();

        if (mode_ == RoomMode::PvE)
        {
            // PvE: 검기(투사체)로 몬스터를 친다.
            if (monsters_ && (packet.kind == AttackKind::Wave || packet.kind == AttackKind::HeavyWave)) DoWave(*member, packet, nowMs);
            return;
        }
        if (packet.kind != AttackKind::Slash) return;

        // PvP 근접 공격. 시작은 서버 도착 시각 기준이다. 공격 준비 시간이 공격자 쪽 지연을 흡수하므로 되감지 않는다.
        if (member->hasAttacked && nowMs - member->lastAttackMs < kSlashCooldownMs) return;
        member->hasAttacked = true;
        member->lastAttackMs = nowMs;

        const float radians = packet.yaw * std::numbers::pi_v<float> / 180.f;
        ActiveAttack attack;
        attack.attackerId = playerId;
        attack.kind = AttackKind::Slash;
        attack.hitMs = nowMs + rules_.attackWindupMs;
        attack.origin = member->position;
        attack.center = {member->position.x + kSlashReach * std::cos(radians), member->position.y + kSlashReach * std::sin(radians), member->position.z};
        attack.radius = kSlashRadius;
        attack.damage = kSlashDamage;
        StartAttack(std::move(attack), nowMs);
    }

    void Room::DoWave(Member& member, const C_Attack& packet, std::uint32_t nowMs)
    {
        // 검기 사이 간격. 클라이언트 공격 동작(0.72초)보다 빨리 쏠 수 없다.
        const std::uint32_t interval = member.hasAttacked ? nowMs - member.lastAttackMs : UINT32_MAX;
        if (interval < kWaveCooldownMs) return;

        const bool heavy = packet.kind == AttackKind::HeavyWave;
        float charge = 0.f;
        if (heavy)
        {
            // 충전은 직전 공격이 끝난 뒤에야 시작할 수 있으므로, 지난 시간으로 모을 수 있었던 양까지만 인정한다.
            const float possible = interval == UINT32_MAX ? 1.f : std::min(1.f, static_cast<float>(interval - kWaveCooldownMs) / static_cast<float>(kFullChargeMs));
            charge = std::min(std::isfinite(packet.charge) ? std::clamp(packet.charge, 0.f, 1.f) : 0.f, possible);
            if (charge < kMinCharge - 0.02f)
            {
                Log("방 {} 충전 검기 거부: {} (요청 {:.2f}, 가능 {:.2f})", teamCode_, member.name, packet.charge, possible);
                return;
            }
        }
        member.hasAttacked = true;
        member.lastAttackMs = nowMs;

        const float radians = packet.yaw * std::numbers::pi_v<float> / 180.f;
        const float x = member.position.x + kWaveMuzzle * std::cos(radians), y = member.position.y + kWaveMuzzle * std::sin(radians);
        const float damage = heavy ? kHeavyDamage * (1.f + charge * 2.f) : kWaveDamage;
        const float radius = heavy ? kHeavyRadius + charge * kHeavyRadiusPerCharge : kWaveRadius;
        const int pierces = heavy ? kHeavyPierces + static_cast<int>(std::lround(charge * kHeavyPiercesPerCharge)) : 1;
        const std::uint32_t lifeMs = heavy ? kWaveLifeMs + static_cast<std::uint32_t>(charge * kHeavyLifePerChargeMs) : kWaveLifeMs;

        auto spawn = Make<S_WaveSpawn>();
        spawn.waveId = monsters_->SpawnWave(member.playerId, x, y, packet.yaw, damage, radius, pierces, lifeMs, nowMs);
        spawn.ownerId = member.playerId;
        spawn.x = x;
        spawn.y = y;
        spawn.yaw = packet.yaw;
        spawn.radius = radius;
        spawn.speed = MonsterTuning::kWaveSpeed;
        spawn.lifeMs = static_cast<std::uint16_t>(lifeMs);
        spawn.charge = charge;
        Broadcast(net::ToBuffer(spawn));
    }

    void Room::DoRespawn(std::uint32_t playerId)
    {
        Member* member = Find(playerId);
        if (!member || !member->dead) return;
        member->dead = false;
        member->hp = kMaxHp;
        member->protectedUntilMs = ServerTimeMs() + kRespawnProtectionMs;
        member->position = SpawnPosition(mode_, member->slot);
        member->velocity = {};
        member->sim = move::State{};
        member->sim.x = member->position.x;
        member->sim.y = member->position.y;
        member->dodges.clear();
        member->history.clear();
        member->history.push_back({ServerTimeMs(), member->position});
        member->dirty = member->ackDirty = true; // 본인은 S_MoveAck의 새 상태로 재조정된다

        auto packet = Make<S_Respawn>();
        packet.playerId = playerId;
        packet.position = member->position;
        packet.hp = member->hp;
        Broadcast(net::ToBuffer(packet));
        Log("방 {} 부활: {}", teamCode_, member->name);
    }

    std::uint32_t Room::StartAttack(ActiveAttack attack, std::uint32_t startMs, bool announce)
    {
        attack.attackId = nextAttackId_++;
        if (announce)
        {
            auto start = Make<S_AttackStart>();
            start.attackId = attack.attackId;
            start.attackerId = attack.attackerId;
            start.kind = attack.kind;
            start.startServerMs = startMs;
            start.hitServerMs = attack.hitMs;
            start.center = attack.center;
            // 부채꼴은 중심 사이 거리 기준 사거리를 알린다(판정은 radius + 몸 반지름으로 한다).
            start.radius = attack.halfAngle > 0.f ? attack.radius + kBodyRadius : attack.radius;
            start.damage = attack.damage;
            start.monsterId = attack.monsterId;
            start.yaw = attack.yaw;
            start.halfAngle = attack.halfAngle;
            Broadcast(net::ToBuffer(start));
        }
        attacks_.push_back(std::move(attack));
        return attacks_.back().attackId;
    }

    // ---- 몬스터 ----

    void Room::UpdateMonsters(std::uint32_t nowMs, float dt)
    {
        if (!monsters_) return;
        std::vector<MonsterPlayer> players;
        players.reserve(members_.size());
        for (const Member& member : members_) players.push_back({member.playerId, member.position.x, member.position.y, !member.dead});

        monsterEvents_.Clear();
        const auto simStart = Clock::now();
        monsters_->Update(nowMs, dt, players, monsterEvents_);
        const double simMs = std::chrono::duration<double, std::milli>(Clock::now() - simStart).count();
        monsterSimSumMs_ += simMs;
        monsterSimMaxMs_ = std::max(monsterSimMaxMs_, simMs);
        ApplyMonsterEvents(nowMs);

        // 살아 있는 몬스터 전부를 보낸다. 0마리가 되면 빈 목록을 한 번 보내 클라이언트가 지우게 한다.
        const std::vector<Monster>& monsters = monsters_->Monsters();
        if (monsters.empty() && !sentMonsters_) return;
        std::vector<MonsterEntry> entries;
        entries.reserve(monsters.size());
        for (const Monster& monster : monsters)
        {
            MonsterEntry entry{};
            entry.id = monster.id;
            entry.kind = monster.kind;
            entry.state = monster.ShownState(nowMs);
            entry.x = static_cast<std::int16_t>(std::lround(monster.x));
            entry.y = static_cast<std::int16_t>(std::lround(monster.y));
            entry.yaw = static_cast<std::uint8_t>(std::lround(monster.yaw / 360.f * 256.f) & 0xFF);
            entry.hp = static_cast<std::uint16_t>(std::ceil(monster.hp));
            entries.push_back(entry);
        }
        auto batch = Make<S_MonsterBatch>();
        batch.serverTick = tick_;
        Broadcast(net::ToBuffer(batch, std::span<const MonsterEntry>(entries)));
        sentMonsters_ = !monsters.empty();
    }

    void Room::ApplyMonsterEvents(std::uint32_t nowMs)
    {
        using Tuning = MonsterTuning;
        // 예고 시작·취소는 일어난 순서대로 처리한다.
        for (const MonsterEvents::Tell& tell : monsterEvents_.tells)
        {
            if (tell.cancel)
            {
                const auto it = monsterAttacks_.find(tell.monsterId);
                if (it == monsterAttacks_.end()) continue;
                const std::uint32_t attackId = it->second;
                monsterAttacks_.erase(it);
                // 명중 시각이 이미 지나 판정을 기다리는 공격은 그대로 둔다(이미 휘두른 공격이다).
                if (std::ranges::any_of(attacks_, [&](const ActiveAttack& a) { return a.attackId == attackId && a.hitMs <= nowMs; })) continue;
                std::erase_if(attacks_, [&](const ActiveAttack& a) { return a.attackId == attackId; });
                auto cancel = Make<S_AttackCancel>();
                cancel.attackId = attackId;
                Broadcast(net::ToBuffer(cancel));
                continue;
            }
            const Vec3 at{tell.x, tell.y, move::kGroundZ};
            if (tell.kind == 1)
            {
                // 돌진: 예고만 알린다. 피해는 돌진 중 실제로 닿았을 때(DashTouch) 판정한다.
                auto start = Make<S_AttackStart>();
                start.attackId = nextAttackId_++;
                start.kind = AttackKind::MonsterCharge;
                start.startServerMs = tell.startMs;
                start.hitServerMs = tell.hitMs; // 출발 시각
                start.center = at;
                start.radius = Tuning::kDashSpeed * static_cast<float>(Tuning::kDashMs) / 1000.f; // 돌진 거리
                start.damage = Tuning::kDashDamage;
                start.monsterId = tell.monsterId;
                start.yaw = tell.yaw;
                Broadcast(net::ToBuffer(start));
                monsterAttacks_[tell.monsterId] = start.attackId;
            }
            else
            {
                ActiveAttack attack;
                attack.kind = AttackKind::MonsterArc;
                attack.hitMs = tell.hitMs;
                attack.origin = attack.center = at;
                attack.radius = Tuning::kArcRange - kBodyRadius;
                attack.damage = Tuning::kArcDamage[tell.kind];
                attack.yaw = tell.yaw;
                attack.halfAngle = Tuning::kArcHalfAngle;
                attack.monsterId = tell.monsterId;
                monsterAttacks_[tell.monsterId] = StartAttack(std::move(attack), tell.startMs);
            }
        }
        // 돌진 접촉: 닿은 순간이 명중 시각이다. 그 플레이어만, 회피 무적·지연 보정을 거쳐 판정한다.
        for (const MonsterEvents::DashTouch& touch : monsterEvents_.dashTouches)
        {
            ActiveAttack attack;
            attack.kind = AttackKind::MonsterCharge;
            attack.hitMs = nowMs;
            attack.origin = attack.center = {touch.x, touch.y, move::kGroundZ};
            attack.radius = Tuning::kDashHitDistance - kBodyRadius;
            attack.damage = Tuning::kDashDamage;
            attack.monsterId = touch.monsterId;
            attack.onlyTargetId = touch.playerId;
            StartAttack(std::move(attack), nowMs, false);
        }
        for (const MonsterEvents::Hit& hit : monsterEvents_.hits)
        {
            auto packet = Make<S_MonsterHit>();
            packet.monsterId = hit.monsterId;
            packet.attackerId = hit.attackerId;
            packet.damage = hit.damage;
            packet.hpAfter = static_cast<std::uint16_t>(std::ceil(hit.hpAfter));
            packet.x = hit.x;
            packet.y = hit.y;
            Broadcast(net::ToBuffer(packet));
            if (hit.hpAfter <= 0.f) monsterAttacks_.erase(hit.monsterId);
        }
        for (const MonsterEvents::WaveEnd& end : monsterEvents_.waveEnds)
        {
            auto packet = Make<S_WaveEnd>();
            packet.waveId = end.waveId;
            packet.x = end.x;
            packet.y = end.y;
            Broadcast(net::ToBuffer(packet));
        }
    }

    void Room::UpdateCombat(std::uint32_t nowMs)
    {
        // 서버 내려찍기 시험 패턴: 플레이어를 번갈아 노린다.
        if (options_.pveSlamIntervalMs > 0 && mode_ == RoomMode::PvE && !members_.empty() && nowMs >= nextSlamMs_)
        {
            nextSlamMs_ = nowMs + options_.pveSlamIntervalMs;
            for (std::size_t i = 0; i < members_.size(); ++i)
            {
                const Member& target = members_[slamTarget_++ % members_.size()];
                if (target.dead) continue;
                ActiveAttack attack;
                attack.kind = AttackKind::Slam;
                attack.hitMs = nowMs + kSlamWindupMs;
                attack.origin = attack.center = target.position;
                attack.radius = kSlamRadius;
                attack.damage = kSlamDamage;
                StartAttack(std::move(attack), nowMs);
                break;
            }
        }

        // 판정 유예: 플레이어마다 "명중 시각 + 허용 되감기"가 지나야 확정한다.
        // 그때까지 도착한 회피는 누른 시각이 명중 전이면 모두 반영된다.
        for (ActiveAttack& attack : attacks_)
        {
            for (Member& member : members_)
            {
                if (member.playerId == attack.attackerId || std::ranges::contains(attack.resolved, member.playerId)) continue;
                if (attack.onlyTargetId != 0 && attack.onlyTargetId != member.playerId) continue;
                const std::uint32_t allowed = AllowedRewindMs(rules_, member.rttMs);
                if (nowMs < attack.hitMs + allowed) continue;
                attack.resolved.push_back(member.playerId);
                if (!member.dead) Resolve(attack, member, allowed);
            }
        }
        // 허용 되감기는 상한을 넘지 않으므로 이 시점이면 모든 플레이어 판정이 끝났다.
        std::erase_if(attacks_, [&](const ActiveAttack& attack) { return nowMs > attack.hitMs + rules_.rewindCapMs + 200; });
    }

    void Room::Resolve(const ActiveAttack& attack, Member& member, std::uint32_t allowedRewindMs)
    {
        // 위치 이력은 도착 시각으로 쌓이므로, 클라이언트가 명중 순간에 있던 위치는 한쪽 지연만큼 뒤에 도착한 표본이다.
        const std::uint32_t oneWayMs = std::min(member.rttMs / 2, allowedRewindMs);
        const Vec3 position = PositionAt(member, attack.hitMs + oneWayMs);
        const float distance = std::hypot(position.x - attack.center.x, position.y - attack.center.y);
        if (distance > attack.radius + kBodyRadius) return;
        if (attack.hitMs < member.protectedUntilMs) return; // 부활 직후 보호
        if (attack.halfAngle > 0.f && distance > 1.f)
        {
            // 부채꼴: 공격 방향과 대상 방향 사이 각이 반각 안이어야 한다.
            const float radians = attack.yaw * std::numbers::pi_v<float> / 180.f;
            const float dot = ((position.x - attack.center.x) * std::cos(radians) + (position.y - attack.center.y) * std::sin(radians)) / distance;
            if (dot < std::cos(attack.halfAngle * std::numbers::pi_v<float> / 180.f)) return;
        }
        // 엄폐: 공격 시작점과 대상 사이를 기둥·벽이 가리면 맞지 않는다(클라이언트 1인 모드의 시야 검사와 같은 원칙).
        if (options_.map && options_.map->LineBlocked(attack.origin, position))
        {
            Log("방 {} 판정: 공격 {} → {} 엄폐로 빗나감", teamCode_, attack.attackId, member.name);
            return;
        }

        const auto dodge = std::ranges::find_if(member.dodges, [&](const DodgeRecord& d) { return InIframe(rules_, d.serverMs, attack.hitMs); });

        auto result = Make<S_HitResult>();
        result.attackId = attack.attackId;
        result.targetId = member.playerId;
        result.hitServerMs = attack.hitMs;
        if (dodge != member.dodges.end())
        {
            result.result = HitResult::Dodged;
            result.damage = 0.f;
            result.dodgeServerMs = dodge->serverMs;
            result.rewindMs = static_cast<std::uint16_t>(dodge->rewindMs);
            Log("방 {} 판정: 공격 {} → {} 회피 (명중 {}ms보다 {}ms 전 회피, 되감기 {}ms)", teamCode_, attack.attackId, member.name,
                attack.hitMs, attack.hitMs - dodge->serverMs, dodge->rewindMs);
        }
        else
        {
            result.result = HitResult::Hit;
            result.damage = attack.damage;
            member.hp = std::max(0.f, member.hp - attack.damage);
            Log("방 {} 판정: 공격 {} → {} 피격 {:.0f} (HP {:.0f})", teamCode_, attack.attackId, member.name, attack.damage, member.hp);
        }
        result.hpAfter = member.hp;
        Broadcast(net::ToBuffer(result));

        // 사망: C_Respawn이 올 때까지 몬스터가 노리지 않고 공격·회피를 받지 않는다.
        if (member.hp <= 0.f)
        {
            member.dead = true;
            Log("방 {} 사망: {}", teamCode_, member.name);
        }
    }

    Vec3 Room::PositionAt(const Member& member, std::uint32_t serverMs)
    {
        // serverMs 이전의 가장 마지막 표본. 이력이 비었으면 현재 위치.
        Vec3 position = member.position;
        for (const PositionSample& sample : member.history)
        {
            if (sample.serverMs > serverMs) break;
            position = sample.position;
        }
        if (!member.history.empty() && member.history.front().serverMs > serverMs) position = member.history.front().position;
        return position;
    }

    asio::awaitable<void> Room::TickLoop([[maybe_unused]] std::shared_ptr<Room> self)
    {
        auto next = Clock::now();
        while (running_)
        {
            next += kTickInterval;
            // 멈춰 있다 깨어났을 때 밀린 틱을 몰아서 돌지 않는다.
            if (Clock::now() - next > std::chrono::seconds(1)) next = Clock::now();
            tickTimer_.expires_at(next);
            asio::error_code ec;
            co_await tickTimer_.async_wait(asio::redirect_error(asio::use_awaitable, ec));
            if (!running_) break;
            ++tick_;

            // 실제로 지난 시간만큼 진행한다(타이머가 늦게 깨어나도 몬스터 속도가 변하지 않는다).
            const auto tickStart = Clock::now();
            const float dt = lastTickTime_ == Clock::time_point{}
                                 ? 0.05f
                                 : std::clamp(std::chrono::duration<float>(tickStart - lastTickTime_).count(), 0.001f, 0.1f);
            lastTickTime_ = tickStart;
            const std::uint32_t nowMs = ServerTimeMs();
            UpdateMonsters(nowMs, dt); // 몬스터가 새 공격을 예고한 뒤
            UpdateCombat(nowMs);       // 명중 시각이 지난 공격을 판정한다

            // 입력을 보낸 본인에게 마지막 처리 번호와 확정 상태를 알린다(클라이언트 재조정 기준).
            for (Member& member : members_)
            {
                if (!member.ackDirty) continue;
                member.ackDirty = false;
                auto ack = Make<S_MoveAck>();
                ack.sequence = member.lastInputSequence;
                ack.state = move::ToProto(member.sim);
                member.session->Send(net::ToBuffer(ack));
            }

            std::vector<MoveEntry> entries;
            for (Member& member : members_)
            {
                if (!member.dirty) continue;
                member.dirty = false;
                entries.push_back({member.playerId, member.position, member.velocity, member.yaw, member.state});
            }
            if (!entries.empty())
            {
                // 모두에게 같은 버퍼를 보낸다. 클라이언트는 자기 playerId 항목을 무시한다.
                auto batch = Make<S_MoveBatch>();
                batch.serverTick = tick_;
                Broadcast(net::ToBuffer(batch, std::span<const MoveEntry>(entries)));
            }

            // 부하 기록: 몬스터가 있는 방의 틱 처리 시간을 10초마다 남긴다.
            const double costMs = std::chrono::duration<double, std::milli>(Clock::now() - tickStart).count();
            tickCostSumMs_ += costMs;
            tickCostMaxMs_ = std::max(tickCostMaxMs_, costMs);
            if (++tickCostSamples_ >= 200)
            {
                if (monsters_ && !members_.empty())
                {
                    Log("방 {} 부하: 플레이어 {}명, 몬스터 {}마리(누적 생성 {}, 처치 {}), 틱 평균 {:.3f}ms 최대 {:.3f}ms (그중 몬스터 계산 평균 {:.3f}ms 최대 {:.3f}ms)",
                        teamCode_, members_.size(), monsters_->Monsters().size(), monsters_->TotalSpawned(), monsters_->TotalKilled(),
                        tickCostSumMs_ / tickCostSamples_, tickCostMaxMs_, monsterSimSumMs_ / tickCostSamples_, monsterSimMaxMs_);
                }
                tickCostSumMs_ = tickCostMaxMs_ = monsterSimSumMs_ = monsterSimMaxMs_ = 0;
                tickCostSamples_ = 0;
            }
        }
    }

    void Room::Broadcast(const net::Buffer& buffer, std::uint32_t exceptPlayerId)
    {
        for (const Member& member : members_)
        {
            if (member.playerId != exceptPlayerId) member.session->Send(buffer);
        }
    }

    Room::Member* Room::Find(std::uint32_t playerId)
    {
        const auto it = std::ranges::find(members_, playerId, &Member::playerId);
        return it != members_.end() ? &*it : nullptr;
    }

    int Room::FreeSlot() const
    {
        for (int slot = 0; slot < CapacityFor(mode_); ++slot)
        {
            if (std::ranges::none_of(members_, [slot](const Member& m) { return m.slot == slot; })) return slot;
        }
        return -1;
    }

    Vec3 Room::SpawnPosition(RoomMode mode, int slot)
    {
        if (mode == RoomMode::PvP)
        {
            // 맵 중앙을 둘러싼 반지름 6m 원 위에 고르게 세운다.
            const float angle = 2.f * std::numbers::pi_v<float> * static_cast<float>(slot) / static_cast<float>(kMaxPvpPlayers);
            return {600.f * std::cos(angle), 600.f * std::sin(angle), move::kGroundZ};
        }
        // 클라이언트 데모의 시작 위치(-750, 0) 근처에 1.5m 간격으로 나란히 세운다.
        return {-750.f, -150.f + 150.f * static_cast<float>(slot), move::kGroundZ};
    }

    PlayerInfo Room::ToInfo(const Member& member)
    {
        PlayerInfo info{};
        info.playerId = member.playerId;
        CopyName(info.name, member.name.c_str());
        info.position = member.position;
        info.yaw = member.yaw;
        return info;
    }
}
