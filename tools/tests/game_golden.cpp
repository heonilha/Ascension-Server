// 충돌 지형·몬스터 계산 기준 기록을 만든다. Rust로 옮긴 코드(rust/crates/game)가 C++ 원본과 같은 결과를 내는지 비교하는 데 쓴다.
//
// ascension_game_golden <data/CombatLab.txt> <출력 파일>
//
// 기록은 rust/crates/game/tests/golden/game_v6.txt에 저장해 두고, Rust 시험(tests/golden.rs)이 같은 입력을 돌려 비교한다.
// 입력 만드는 방법을 바꾸면 tests/golden.rs도 똑같이 바꾼다. 규칙을 일부러 고쳤다면 기록을 다시 만든다(Test.ps1 -UpdateGolden).
// 몬스터 시나리오는 증원을 끄고 자리를 직접 정한다(증원 난수는 두 구현이 다르다).
#include "game/CollisionMap.h"
#include "game/Monsters.h"

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <string>
#include <vector>

using namespace asc;
using namespace asc::game;

namespace
{
    std::uint32_t Bits(float value)
    {
        std::uint32_t bits;
        std::memcpy(&bits, &value, sizeof(bits));
        return bits;
    }

    // FNV-1a 32비트. 값 하나를 리틀 엔디언 4바이트로 넣는다.
    struct Hasher
    {
        std::uint32_t value = 2166136261u;
        void Add(std::uint32_t v)
        {
            for (int i = 0; i < 4; ++i)
            {
                value ^= (v >> (8 * i)) & 0xffu;
                value *= 16777619u;
            }
        }
        void Add(float v) { Add(Bits(v)); }
    };

    struct Random
    {
        std::uint32_t seed;
        std::uint32_t Next()
        {
            seed = seed * 1664525u + 1013904223u;
            return seed >> 8;
        }
    };
}

int main(int argc, char** argv)
{
    if (argc < 3)
    {
        std::fprintf(stderr, "usage: ascension_game_golden <map file> <output file>\n");
        return 2;
    }
    std::string error;
    const auto map = CollisionMap::Load(argv[1], error);
    if (!map)
    {
        std::fprintf(stderr, "cannot read map %s\n", argv[1]);
        return 2;
    }
    std::ofstream out(argv[2], std::ios::binary);
    if (!out)
    {
        std::fprintf(stderr, "cannot write %s\n", argv[2]);
        return 2;
    }
    char line[256];
    std::snprintf(line, sizeof(line), "# game golden: protocol v%u, %zu obstacles\n", static_cast<unsigned>(proto::kVersion), map->Obstacles().size());
    out << line;

    // ---- 충돌 지형: 맵 전체 격자 ----
    {
        Hasher hash;
        int blocked = 0, high = 0, solid = 0, pushed = 0;
        for (int gx = -2850; gx <= 2850; gx += 75)
        {
            for (int gy = -2450; gy <= 2450; gy += 70)
            {
                const float x = static_cast<float>(gx), y = static_cast<float>(gy);
                const bool b = map->Blocked({x, y, 92.f}, 24.f), h = map->Blocked({x, y, 300.f}, 24.f), s = map->SolidOverlap({x, y}, 35.f);
                move::Vec2 p{x, y};
                const bool m = map->PushOutSolid(p, 35.f);
                blocked += b, high += h, solid += s, pushed += m;
                hash.Add(std::uint32_t{b}), hash.Add(std::uint32_t{h}), hash.Add(std::uint32_t{s}), hash.Add(std::uint32_t{m}), hash.Add(p.x), hash.Add(p.y);
            }
        }
        std::snprintf(line, sizeof(line), "collision grid blocked %d high %d solid %d pushed %d hash %08x\n", blocked, high, solid, pushed, hash.value);
        out << line;
    }

    // ---- 충돌 지형: 두 점 사이 이동 경로·시야 (짝수 번째는 맵 전체, 홀수 번째는 3m 안 짧은 구간) ----
    {
        Hasher hash;
        Random random{77};
        int sweeps = 0, lines = 0;
        for (int i = 0; i < 4000; ++i)
        {
            const std::uint32_t c = random.Next(), d = random.Next(), e = random.Next(), f = random.Next();
            const int ax = static_cast<int>(c % 5601) - 2800, ay = static_cast<int>(d % 4801) - 2400;
            const int bx = i % 2 == 0 ? static_cast<int>(e % 5601) - 2800 : ax + static_cast<int>(e % 601) - 300;
            const int by = i % 2 == 0 ? static_cast<int>(f % 4801) - 2400 : ay + static_cast<int>(f % 601) - 300;
            const proto::Vec3 a{static_cast<float>(ax), static_cast<float>(ay), 92.f}, b{static_cast<float>(bx), static_cast<float>(by), 92.f};
            const bool sweep = map->SweepBlocked(a, b, 16.f), sight = map->LineBlocked(a, b);
            sweeps += sweep, lines += sight;
            hash.Add(std::uint32_t{sweep}), hash.Add(std::uint32_t{sight});
        }
        std::snprintf(line, sizeof(line), "collision pairs sweep %d line %d hash %08x\n", sweeps, lines, hash.value);
        out << line;
    }

    // ---- 몬스터·검기: 60초(1200틱). 플레이어 둘이 정해진 길을 오가며 검기를 쏘고, 6마리 아래로 줄면 정해진 자리에 채운다 ----
    {
        MonsterField field(&*map, 1, 0);
        field.SetSpawning(false);
        std::uint32_t now = 0;
        for (int i = 0; i < 12; ++i) field.SpawnAt(static_cast<std::uint8_t>(i % 3), static_cast<float>(-900 + i * 150), i % 2 == 0 ? -500.f : 500.f, now);
        int counter = 0;
        Hasher hash;
        MonsterEvents events;
        std::vector<MonsterPlayer> players(2);
        std::size_t tells = 0, touches = 0, hits = 0, waveEnds = 0;
        for (int tick = 0; tick < 1200; ++tick)
        {
            now += 50;
            players[0] = {1, static_cast<float>(-750 + (tick % 400) * 3), 0.f, true};
            players[1] = {2, 600.f, static_cast<float>(400 - (tick % 300) * 2), !(tick >= 600 && tick < 800)};
            events.Clear();
            if (field.Monsters().size() < 6)
            {
                field.SpawnAt(static_cast<std::uint8_t>(counter % 3), static_cast<float>(-1000 + (counter * 331) % 2000), static_cast<float>(-800 + (counter * 197) % 1600), now);
                ++counter;
            }
            if (tick % 13 == 0) field.SpawnWave(1, players[0].x, players[0].y, static_cast<float>((tick * 37) % 360), 20.f, 55.f, 1, 1200, now);
            if (tick % 41 == 0) field.SpawnWave(2, players[1].x, players[1].y, static_cast<float>((tick * 53) % 360), 60.f, 150.f, 6, 1600, now);
            field.Update(now, .05f, players, events);

            hash.Add(static_cast<std::uint32_t>(field.Monsters().size()));
            for (const Monster& m : field.Monsters())
            {
                hash.Add(std::uint32_t{m.id}), hash.Add(std::uint32_t{m.kind}), hash.Add(static_cast<std::uint32_t>(m.phase));
                hash.Add(m.x), hash.Add(m.y), hash.Add(m.yaw), hash.Add(m.hp), hash.Add(m.deadlineMs), hash.Add(m.knockRemaining), hash.Add(m.staggerUntilMs);
            }
            hash.Add(static_cast<std::uint32_t>(events.tells.size()));
            for (const auto& t : events.tells)
            {
                hash.Add(std::uint32_t{t.monsterId}), hash.Add(std::uint32_t{t.kind}), hash.Add(std::uint32_t{t.cancel});
                hash.Add(t.x), hash.Add(t.y), hash.Add(t.yaw), hash.Add(t.startMs), hash.Add(t.hitMs);
            }
            hash.Add(static_cast<std::uint32_t>(events.dashTouches.size()));
            for (const auto& t : events.dashTouches) hash.Add(std::uint32_t{t.monsterId}), hash.Add(t.playerId), hash.Add(t.x), hash.Add(t.y);
            hash.Add(static_cast<std::uint32_t>(events.hits.size()));
            for (const auto& h : events.hits)
            {
                hash.Add(std::uint32_t{h.monsterId}), hash.Add(h.attackerId), hash.Add(h.damage), hash.Add(h.hpAfter), hash.Add(h.x), hash.Add(h.y);
            }
            hash.Add(static_cast<std::uint32_t>(events.waveEnds.size()));
            for (const auto& w : events.waveEnds) hash.Add(w.waveId), hash.Add(w.x), hash.Add(w.y);
            hash.Add(static_cast<std::uint32_t>(field.WaveCount()));

            tells += events.tells.size(), touches += events.dashTouches.size(), hits += events.hits.size(), waveEnds += events.waveEnds.size();
            if (tick % 50 == 49)
            {
                std::snprintf(line, sizeof(line), "monsters tick %d count %zu killed %u waves %zu hash %08x\n", tick + 1, field.Monsters().size(), field.TotalKilled(),
                              field.WaveCount(), hash.value);
                out << line;
            }
        }
        std::snprintf(line, sizeof(line), "monsters total spawned %u killed %u tells %zu touches %zu hits %zu wave_ends %zu\n", field.TotalSpawned(),
                      field.TotalKilled(), tells, touches, hits, waveEnds);
        out << line;
    }
    return out.good() ? 0 : 2;
}
