// 몬스터·검기 시뮬레이션 단위 시험. 네트워크 없이 MonsterField에 시각을 직접 넣어 돌린다(결과가 매번 같다).
//
// ascension_monster_test [data/CombatLab.txt]
#include "game/Monsters.h"

#include "core/Platform.h"

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <print>
#include <string>
#include <vector>

using namespace asc;
using namespace asc::game;
using proto::MonsterState;
using T = MonsterTuning;

namespace
{
    int failures = 0;

    void Check(bool condition, const char* name)
    {
        std::println("{} {}", condition ? "PASS" : "FAIL", name);
        if (!condition) ++failures;
    }

    // 20Hz로 seconds만큼 진행한다. 틱마다 visit(nowMs, events)를 부른다.
    template <class Visit>
    void Run(MonsterField& field, std::uint32_t& nowMs, float seconds, const std::vector<MonsterPlayer>& players, Visit visit)
    {
        MonsterEvents events;
        for (int i = 0; i < static_cast<int>(seconds * 20.f + .5f); ++i)
        {
            nowMs += 50;
            events.Clear();
            field.Update(nowMs, .05f, players, events);
            visit(nowMs, events);
        }
    }

    const Monster* Find(const MonsterField& field, std::uint16_t id)
    {
        const auto it = std::ranges::find(field.Monsters(), id, &Monster::id);
        return it == field.Monsters().end() ? nullptr : &*it;
    }
}

int main(int argc, char** argv)
{
    InitPlatform();
    const std::string path = argc > 1 ? argv[1] : FindDataFile(argv[0], "data/CombatLab.txt");
    std::string error;
    const auto map = CollisionMap::Load(path, error);
    if (!map)
    {
        std::println("FAIL load: {}", error);
        return EXIT_FAILURE;
    }
    const auto Clearance = [&](float x, float y) {
        float best = 1e9f;
        for (const move::Obstacle& o : map->Obstacles())
        {
            if (move::Solid(o)) best = std::min(best, move::SignedDistance({x, y}, o.polygon));
        }
        return best;
    };

    // ---- 추적 속도 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 0;
        const std::uint16_t id = field.SpawnAt(0, 600.f, 0.f, now).id;
        Run(field, now, 1.f, {{1, 0.f, 0.f, true}}, [](auto, auto&) {});
        Check(std::abs(Find(field, id)->x - (600.f - 220.f)) < 3.f && Find(field, id)->phase == MonsterState::Chase, "chase_220cm_per_second");
    }

    // ---- 기둥 돌아가기: 남쪽 부서진 기둥(100,-650, 반지름 60)을 사이에 두고 추적 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 0;
        const std::uint16_t id = field.SpawnAt(0, 100.f, -350.f, now).id;
        float minClearance = 1e9f;
        bool reached = false;
        Run(field, now, 6.f, {{1, 100.f, -950.f, true}}, [&](auto, auto&) {
            const Monster* m = Find(field, id);
            minClearance = std::min(minClearance, Clearance(m->x, m->y));
            reached |= std::hypot(m->x - 100.f, m->y + 950.f) < 160.f;
        });
        Check(reached, "steers_around_column_to_player");
        Check(minClearance > T::kRadius - 1.f, "never_enters_obstacle_while_steering");
    }

    // ---- 검병: 예고 0.6초 → 빈틈 0.8초 → 다시 추적 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t id = field.SpawnAt(0, 140.f, 0.f, now).id;
        std::vector<MonsterEvents::Tell> tells;
        std::uint32_t recoverAt = 0, chaseAgainAt = 0;
        Run(field, now, 2.f, {{1, 0.f, 0.f, true}}, [&](std::uint32_t t, MonsterEvents& e) {
            tells.insert(tells.end(), e.tells.begin(), e.tells.end());
            const Monster* m = Find(field, id);
            if (!recoverAt && m->phase == MonsterState::Recover) recoverAt = t;
            if (recoverAt && !chaseAgainAt && m->phase != MonsterState::Recover) chaseAgainAt = t;
        });
        Check(!tells.empty() && !tells[0].cancel && tells[0].hitMs - tells[0].startMs == 600 && std::abs(std::abs(tells[0].yaw) - 180.f) < 1.f,
              "swordsman_tell_600ms_facing_player");
        Check(recoverAt != 0 && recoverAt - tells[0].hitMs < 50 && chaseAgainAt - tells[0].hitMs >= 800 && chaseAgainAt - tells[0].hitMs < 900,
              "swordsman_recovers_800ms_then_resumes");
        Check(tells.size() >= 2 && tells[1].startMs >= tells[0].hitMs + 800, "swordsman_attacks_again_after_recover");
    }

    // ---- 지휘관: 두 번째 예고는 첫 명중 시각에 바로 시작, 0.45초 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        field.SpawnAt(2, 140.f, 0.f, now);
        std::vector<MonsterEvents::Tell> tells;
        Run(field, now, 1.4f, {{1, 0.f, 0.f, true}}, [&](auto, MonsterEvents& e) { tells.insert(tells.end(), e.tells.begin(), e.tells.end()); });
        Check(tells.size() == 2 && tells[1].startMs == tells[0].hitMs && tells[1].hitMs - tells[1].startMs == 450, "elite_double_strike_timing");
    }

    // ---- 폭주 망령: 예고 0.9초 → 0.5초 동안 약 6m 돌진, 길 위의 플레이어에 한 번 닿음 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t id = field.SpawnAt(1, 300.f, 0.f, now).id;
        std::vector<MonsterEvents::Tell> tells;
        int touches = 0;
        float farthest = 300.f;
        Run(field, now, 1.6f, {{1, 0.f, 0.f, true}}, [&](auto, MonsterEvents& e) {
            tells.insert(tells.end(), e.tells.begin(), e.tells.end());
            touches += static_cast<int>(e.dashTouches.size());
            farthest = std::min(farthest, Find(field, id)->x);
        });
        Check(tells.size() == 1 && tells[0].kind == 1 && tells[0].hitMs - tells[0].startMs == 900, "charger_tell_900ms");
        Check(std::abs((300.f - farthest) - 600.f) < 65.f, "charger_dashes_about_600cm");
        Check(touches == 1, "charger_touches_player_once");
        Check(Find(field, id)->phase == MonsterState::Recover, "charger_recovers_after_dash");
    }

    // ---- 돌진은 벽에서 멈춘다: 동쪽 경계벽(x=2800)을 향해 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t id = field.SpawnAt(1, 2450.f, 0.f, now).id;
        float maxX = 0.f;
        Run(field, now, 1.6f, {{1, 2740.f, 0.f, true}}, [&](auto, auto&) { maxX = std::max(maxX, Find(field, id)->x); });
        Check(maxX <= 2800.f - T::kRadius + .5f && maxX > 2700.f, "charger_stops_at_wall");
    }

    // ---- 피격: 넉백 30cm(피해 20), 경직 0.16초, 예고 취소, 0.35초 안에는 다시 경직되지 않음 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t id = field.SpawnAt(0, 140.f, 0.f, now).id;
        MonsterEvents events;
        const std::vector<MonsterPlayer> players{{1, 0.f, 0.f, true}};
        now += 50;
        field.Update(now, .05f, players, events); // 예고 시작
        const bool telling = Find(field, id)->phase == MonsterState::Tell;
        events.Clear();
        field.Damage(id, 1, 20.f, 1.f, 0.f, now, events);
        const bool canceled = !events.tells.empty() && events.tells.back().cancel;
        Check(telling && canceled && events.hits.size() == 1 && events.hits[0].hpAfter == 30.f, "hit_cancels_tell_and_reports_hp");
        Check(Find(field, id)->ShownState(now) == MonsterState::Stagger, "hit_staggers");
        const float before = Find(field, id)->x;
        float pushed = 0.f; // 가장 멀리 밀린 거리(경직이 끝나면 다시 다가온다)
        Run(field, now, .2f, players, [&](auto, auto&) { pushed = std::max(pushed, Find(field, id)->x - before); });
        Check(std::abs(pushed - 30.f) < .1f, "knockback_30cm_for_20_damage");
        // 두 번째 피해(첫 피격 250ms 뒤): 피해는 들어가지만 다시 밀리지 않는다
        const float x2 = Find(field, id)->x;
        events.Clear();
        field.Damage(id, 1, 20.f, 1.f, 0.f, now, events);
        Check(Find(field, id)->hp == 10.f && Find(field, id)->ShownState(now) != MonsterState::Stagger && Find(field, id)->knockRemaining == 0.f && Find(field, id)->x == x2,
              "no_second_stagger_within_350ms");
        events.Clear();
        field.Damage(id, 1, 20.f, 1.f, 0.f, now, events);
        Check(Find(field, id) == nullptr && events.hits.back().hpAfter == 0.f && field.TotalKilled() == 1, "dies_at_zero_hp");
    }

    // ---- 지휘관은 맞아도 끊기지 않고 0.08초 늦춰 다시 알린다 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t id = field.SpawnAt(2, 140.f, 0.f, now).id;
        MonsterEvents events;
        const std::vector<MonsterPlayer> players{{1, 0.f, 0.f, true}};
        now += 50;
        field.Update(now, .05f, players, events);
        const std::uint32_t firstHit = events.tells.at(0).hitMs;
        events.Clear();
        field.Damage(id, 1, 20.f, 1.f, 0.f, now, events);
        Check(events.tells.size() == 2 && events.tells[0].cancel && !events.tells[1].cancel && events.tells[1].hitMs == firstHit + 80 &&
                  Find(field, id)->phase == MonsterState::Tell,
              "elite_keeps_attacking_with_80ms_delay");
    }

    // ---- 넉백은 벽을 뚫지 않는다 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t id = field.SpawnAt(0, 2800.f - T::kRadius - 5.f, 0.f, now).id;
        MonsterEvents events;
        field.Damage(id, 1, 40.f, 1.f, 0.f, now, events);
        Run(field, now, .2f, {}, [](auto, auto&) {});
        Check(Find(field, id)->x <= 2800.f - T::kRadius + .01f, "knockback_blocked_by_wall");
    }

    // ---- 검기: 일반은 가장 가까운 한 마리만, 충전은 가까운 순서로 한 번씩 관통 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t farId = field.SpawnAt(0, 500.f, 0.f, now).id, nearId = field.SpawnAt(0, 300.f, 0.f, now).id, side = field.SpawnAt(0, 300.f, 400.f, now).id;
        std::vector<MonsterEvents::Hit> hits;
        std::vector<MonsterEvents::WaveEnd> ends;
        const auto Collect = [&](auto, MonsterEvents& e) {
            hits.insert(hits.end(), e.hits.begin(), e.hits.end());
            ends.insert(ends.end(), e.waveEnds.begin(), e.waveEnds.end());
        };
        field.SpawnWave(7, 45.f, 0.f, 0.f, 20.f, 55.f, 1, 1200, now);
        Run(field, now, .6f, {}, Collect);
        Check(hits.size() == 1 && hits[0].monsterId == nearId && hits[0].attackerId == 7 && hits[0].damage == 20.f, "wave_hits_nearest_only");
        Check(ends.size() == 1 && field.WaveCount() == 0, "wave_ends_when_pierces_spent");
        Check(Find(field, farId)->hp == 50.f && Find(field, side)->hp == 50.f, "wave_leaves_far_and_side_untouched");

        hits.clear();
        field.SpawnWave(7, 45.f, 0.f, 0.f, 30.f, 70.f, 4, 1200, now);
        Run(field, now, 1.3f, {}, Collect);
        Check(hits.size() == 2 && hits[0].monsterId == nearId && hits[1].monsterId == farId, "piercing_wave_hits_each_once_in_order");
        Check(field.WaveCount() == 0, "wave_expires_after_lifetime");
    }

    // ---- 검기는 기둥에 막히고, 기둥 뒤 몬스터는 맞지 않는다 (남쪽 부서진 기둥 100,-650) ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 1000;
        const std::uint16_t hidden = field.SpawnAt(0, 100.f, -850.f, now).id;
        int hits = 0, ends = 0;
        field.SpawnWave(7, 100.f, -400.f, -90.f, 20.f, 55.f, 1, 1200, now);
        Run(field, now, 1.3f, {}, [&](auto, MonsterEvents& e) { hits += static_cast<int>(e.hits.size()), ends += static_cast<int>(e.waveEnds.size()); });
        Check(hits == 0 && ends == 1 && Find(field, hidden)->hp == 50.f, "wave_blocked_by_column");
    }

    // ---- 증원: 아무도 잡지 않으면 48마리에서 멈춘다. 생긴 자리는 장애물 밖, 플레이어에게서 4m 이상 ----
    {
        MonsterField field(&*map, 20261114, 0);
        std::uint32_t now = 0;
        std::size_t maxCount = 0, known = 0;
        bool spawnOk = true;
        std::uint32_t capAtMs = 0;
        const std::vector<MonsterPlayer> players{{1, -750.f, 0.f, true}, {2, 600.f, 400.f, true}};
        bool kinds[3] = {};
        Run(field, now, 70.f, players, [&](std::uint32_t t, auto&) {
            const auto& monsters = field.Monsters();
            for (std::size_t i = known; i < monsters.size(); ++i) // 이번 틱에 새로 생긴 몬스터(아직 움직이기 전 위치는 아니지만 한 틱 뒤)
            {
                kinds[monsters[i].kind] = true;
            }
            known = monsters.size();
            maxCount = std::max(maxCount, monsters.size());
            if (!capAtMs && monsters.size() == proto::kMaxMonsters) capAtMs = t;
            if (t == 59950)
            {
                // 상한에 걸려 있으면 60초의 지휘관이 생길 자리가 없다(1인 데모와 같은 규칙). 직전에 몇 마리 잡아 자리를 낸다.
                MonsterEvents scratch;
                for (int i = 0; i < 10; ++i) field.Damage(field.Monsters().front().id, 1, 1000.f, 0.f, 0.f, t, scratch);
                known = field.Monsters().size();
            }
            for (const Monster& m : monsters) spawnOk &= Clearance(m.x, m.y) > T::kRadius - 1.f && std::abs(m.x) <= 2800.f && std::abs(m.y) <= 2400.f;
        });
        std::println("  증원: 최대 {}마리, {}초에 상한 도달, 누적 생성 {}", maxCount, capAtMs / 1000.f, field.TotalSpawned());
        Check(maxCount == proto::kMaxMonsters && capAtMs > 0 && capAtMs < 45000, "spawns_up_to_cap_48_and_stops");
        Check(spawnOk, "monsters_stay_outside_obstacles_and_inside_arena");
        Check(kinds[0] && kinds[1] && kinds[2], "all_three_kinds_spawn_including_elite_at_60s");
        Check(field.Monsters().size() == proto::kMaxMonsters, "refills_to_cap_after_kills");
    }

    // ---- 살아 있는 플레이어가 없으면 증원도 추적도 멈춘다 ----
    {
        MonsterField field(&*map, 3, 0);
        std::uint32_t now = 0;
        const std::uint16_t id = field.SpawnAt(0, 600.f, 0.f, now).id;
        Run(field, now, 5.f, {{1, 0.f, 0.f, false}}, [](auto, auto&) {});
        Check(field.Monsters().size() == 1 && Find(field, id)->phase == MonsterState::Idle && Find(field, id)->x == 600.f, "idle_without_living_players");
    }

    std::println("{} ({} failed)", failures == 0 ? "결과: PASS" : "결과: FAIL", failures);
    return failures == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
}
