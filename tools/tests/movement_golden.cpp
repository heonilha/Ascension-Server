// 이동 계산 기준 기록을 만든다. Rust 서버(rust/crates/movement)가 FFI로 부른 결과와 비트 단위로 비교하는 데 쓴다.
//
// ascension_movement_golden <data/CombatLab.txt> <출력 파일>
//
// 이 프로그램은 C++ 서버·봇과 같은 CMake 설정으로 빌드한다. 기록은 rust/crates/movement/tests/golden/movement_v4.txt에
// 저장해 두고, Rust 시험(tests/golden.rs)이 같은 입력을 돌려 같은 줄이 나오는지 본다.
// 입력 만드는 방법을 바꾸면 tests/golden.rs도 똑같이 바꾼다. 이동 코드를 일부러 고쳤다면 기록을 다시 만든다.
#include "AscensionMovement.h"

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <string>

using namespace asc;

namespace
{
    std::uint32_t Bits(float value)
    {
        std::uint32_t bits;
        std::memcpy(&bits, &value, sizeof(bits));
        return bits;
    }

    // FNV-1a 32비트
    void Hash(std::uint32_t& hash, const void* data, std::size_t size)
    {
        const auto* bytes = static_cast<const unsigned char*>(data);
        for (std::size_t i = 0; i < size; ++i)
        {
            hash ^= bytes[i];
            hash *= 16777619u;
        }
    }

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
        std::fprintf(stderr, "usage: ascension_movement_golden <map file> <output file>\n");
        return 2;
    }
    std::ifstream mapFile(argv[1], std::ios::binary);
    const std::string text((std::istreambuf_iterator<char>(mapFile)), std::istreambuf_iterator<char>());
    move::World world;
    std::string error;
    if (!mapFile || !move::ParseWorld(text, world, error))
    {
        std::fprintf(stderr, "cannot read map %s: %s\n", argv[1], error.c_str());
        return 2;
    }
    std::ofstream out(argv[2], std::ios::binary);
    if (!out)
    {
        std::fprintf(stderr, "cannot write %s\n", argv[2]);
        return 2;
    }
    char line[256];

    std::snprintf(line, sizeof(line), "# movement golden: protocol v%u, %zu obstacles\n", static_cast<unsigned>(proto::kVersion), world.obstacles.size());
    out << line;

    // 무작위 걷기 8번, 각 60초(3600틱). 20틱마다 방향을 바꾸고 가끔 멈추거나 회피한다.
    for (int run = 0; run < 8; ++run)
    {
        Random random{1000u + static_cast<std::uint32_t>(run)};
        move::State state;
        state.x = static_cast<float>(run * 600 - 2100);
        state.y = run % 2 == 0 ? -700.f : 700.f;
        float mx = 0.f, my = 0.f;
        std::uint32_t hash = 2166136261u;
        for (int tick = 0; tick < 3600; ++tick)
        {
            const std::uint32_t a = random.Next(), b = random.Next(), c = random.Next();
            if (tick % 20 == 0)
            {
                if (a % 7 == 0)
                {
                    mx = my = 0.f;
                }
                else
                {
                    mx = static_cast<float>(static_cast<int>(a % 201) - 100) / 100.f;
                    my = static_cast<float>(static_cast<int>(b % 201) - 100) / 100.f;
                }
            }
            move::Input input;
            input.moveX = mx;
            input.moveY = my;
            input.flags = c % 97 == 0 ? move::kInputDodge : std::uint8_t{0};
            input.dodgeX = mx;
            input.dodgeY = my;
            move::Step(state, input, world);

            const proto::MoveSimState wire = move::ToProto(state);
            Hash(hash, &wire, sizeof(wire));
            if (tick % 300 == 299)
            {
                std::snprintf(line, sizeof(line), "run %d tick %d %08x %08x %08x %08x %u %u %08x %08x\n", run, tick + 1, Bits(state.x), Bits(state.y),
                              Bits(state.vx), Bits(state.vy), static_cast<unsigned>(state.dodgeTick), static_cast<unsigned>(state.cooldown),
                              Bits(state.dodgeX), Bits(state.dodgeY));
                out << line;
            }
        }
        std::snprintf(line, sizeof(line), "run %d hash %08x\n", run, hash);
        out << line;
    }

    // 지형 거리 계산: 맵 전체 격자에서 모든 장애물까지 거리와 밀어낼 방향
    std::uint32_t hash = 2166136261u;
    for (const move::Obstacle& obstacle : world.obstacles)
    {
        for (int gx = -2800; gx <= 2800; gx += 175)
        {
            for (int gy = -2400; gy <= 2400; gy += 150)
            {
                move::Vec2 normal{};
                const float distance = move::SignedDistance({static_cast<float>(gx), static_cast<float>(gy)}, obstacle.polygon, &normal);
                const std::uint32_t values[3] = {Bits(distance), Bits(normal.x), Bits(normal.y)};
                Hash(hash, values, sizeof(values));
            }
        }
    }
    std::snprintf(line, sizeof(line), "distance hash %08x\n", hash);
    out << line;
    return out.good() ? 0 : 2;
}
