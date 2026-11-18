// 충돌 지형 단위 시험. 실제 맵 파일(data/CombatLab.txt)로 대표 지점을 확인한다.
//
// ascension_terrain_test [data/CombatLab.txt]
//
// 좌표는 클라이언트 Scripts/CreateMap.py의 배치에서 가져왔다. 캡슐 중심 높이 92(바닥 z=0에 선 주인공).
#include "game/CollisionMap.h"
#include "game/Room.h"

#include "core/Platform.h"

#include <cmath>
#include <cstdlib>
#include <numbers>
#include <print>
#include <string>

using namespace asc;
using namespace asc::game;
using proto::Vec3;

namespace
{
    int failures = 0;

    void Check(bool condition, const char* name)
    {
        std::println("{} {}", condition ? "PASS" : "FAIL", name);
        if (!condition) ++failures;
    }

    // 회전한 직사각형 안쪽 좌표 → 월드 좌표
    Vec3 Local(float cx, float cy, float yawDegrees, float lx, float ly)
    {
        const float r = yawDegrees * std::numbers::pi_v<float> / 180.f;
        return {cx + lx * std::cos(r) - ly * std::sin(r), cy + lx * std::sin(r) + ly * std::cos(r), 92.f};
    }
}

int main(int argc, char** argv)
{
    InitPlatform();
    const std::string path = argc > 1 ? argv[1] : FindDataFile(argv[0], "data/CombatLab.txt");
    std::string error;
    const auto loaded = CollisionMap::Load(path, error);
    if (!loaded)
    {
        std::println("FAIL load: {}", error);
        return EXIT_FAILURE;
    }
    const CollisionMap& map = *loaded;
    constexpr float r = Room::kTerrainRadius, sweep = Room::kTerrainSweepRadius;

    Check(map.Obstacles().size() == 45, "obstacle_count_45");

    // 폐허 0: 중심(-1550,-850), 기둥 A(-1710,-850) 반지름 55, 받침 ±75 높이 40, 상인방 z 265~335
    Check(map.Blocked({-1710.f, -850.f, 92.f}, r), "column_center_blocked");
    Check(map.Blocked({-1710.f + 55.f + 20.f, -850.f, 92.f}, r), "column_edge_within_radius_blocked");
    Check(!map.Blocked({-1710.f + 55.f + 30.f, -850.f, 92.f}, r), "column_edge_outside_radius_free");
    Check(!map.Blocked({-1550.f, -850.f, 92.f}, r), "walk_under_lintel_free");
    Check(!map.Blocked({-1710.f + 70.f, -850.f + 70.f, 92.f}, r), "step_on_column_foot_free");

    // 부서진 벽 0: 중심(-1320,-690), 25도 회전, 반너비 135×35
    Check(map.Blocked(Local(-1320.f, -690.f, 25.f, 0.f, 0.f), r), "rotated_wall_center_blocked");
    Check(map.Blocked(Local(-1320.f, -690.f, 25.f, 120.f, 0.f), r), "rotated_wall_along_length_blocked");
    Check(!map.Blocked(Local(-1320.f, -690.f, 25.f, 0.f, 35.f + 30.f), r), "rotated_wall_beside_free");

    // 엄폐 바위 0(-2200,-1300): 구 충돌은 가장 작은 축 반지름 95
    Check(map.Blocked({-2200.f + 95.f + 20.f, -1300.f, 92.f}, r), "cover_rock_radius95_blocked");
    Check(!map.Blocked({-2200.f + 95.f + 30.f, -1300.f, 92.f}, r), "cover_rock_outside_free");

    // 보이지 않는 경계벽(x 2800~2820)
    Check(map.Blocked({2790.f, 0.f, 92.f}, r), "east_limit_blocked");
    Check(!map.Blocked({2760.f, 0.f, 92.f}, r), "near_east_limit_free");

    // 높이: 상인방 높이로 떠 있는 캡슐은 막힌다(높이 범위 검사)
    Check(map.Blocked({-1550.f, -850.f, 300.f}, r), "lintel_height_blocked");

    // 이동 경로: 기둥을 뚫고 지나가면 막히고, 옆으로 지나가면 통과
    Check(map.SweepBlocked({-1710.f, -1100.f, 92.f}, {-1710.f, -600.f, 92.f}, sweep), "sweep_through_column_blocked");
    Check(!map.SweepBlocked({-1610.f, -1100.f, 92.f}, {-1610.f, -600.f, 92.f}, sweep), "sweep_beside_column_free");
    // 기둥 둘레를 15Hz 표본 간격(약 30cm)으로 도는 정상 이동은 거부되면 안 된다
    bool aroundOk = true;
    for (int i = 0; i < 24; ++i)
    {
        const float a0 = i * 0.35f, a1 = (i + 1) * 0.35f, R = 55.f + 32.f;
        aroundOk &= !map.SweepBlocked({-1710.f + R * std::cos(a0), -850.f + R * std::sin(a0), 92.f},
                                      {-1710.f + R * std::cos(a1), -850.f + R * std::sin(a1), 92.f}, sweep);
    }
    Check(aroundOk, "circle_around_column_free");

    // 시야: 기둥 너머는 가리고, 빈 곳은 보인다. 상인방은 머리 위라 가리지 않는다
    Check(map.LineBlocked({-1710.f, -1100.f, 92.f}, {-1710.f, -600.f, 92.f}), "line_through_column_blocked");
    Check(!map.LineBlocked({0.f, 0.f, 92.f}, {300.f, 0.f, 92.f}), "line_open_free");
    Check(!map.LineBlocked({-1550.f, -1100.f, 92.f}, {-1550.f, -600.f, 92.f}), "line_under_lintel_free");

    // 시작 위치: 실제 캡슐 반지름(32)으로도 막히지 않아야 한다
    bool spawnsOk = true;
    for (int slot = 0; slot < proto::kMaxRoomPlayers; ++slot) spawnsOk &= !map.Blocked(Room::SpawnPosition(proto::RoomMode::PvE, slot), 32.f);
    for (int slot = 0; slot < proto::kMaxPvpPlayers; ++slot)
    {
        Vec3 p = Room::SpawnPosition(proto::RoomMode::PvP, slot);
        p.z = 92.f;
        spawnsOk &= !map.Blocked(p, 32.f);
    }
    Check(spawnsOk, "all_spawns_free");

    std::println("{} ({} failed)", failures == 0 ? "결과: PASS" : "결과: FAIL", failures);
    return failures == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
}
