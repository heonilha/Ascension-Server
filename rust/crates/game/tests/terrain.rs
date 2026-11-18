//! 충돌 지형 단위 시험. 실제 맵 파일(data/CombatLab.txt)로 대표 지점을 확인한다.
//! (C++ `tools/tests/terrain_test.cpp`를 옮긴 것. 확인 이름도 같다.)
//!
//! 좌표는 클라이언트 Scripts/CreateMap.py의 배치에서 가져왔다. 캡슐 중심 높이 92(바닥 z=0에 선 주인공).

use asc_game::room_layout::{spawn_position, TERRAIN_RADIUS as R, TERRAIN_SWEEP_RADIUS as SWEEP};
use asc_game::CollisionMap;
use asc_protocol::{RoomMode, Vec3, MAX_PVP_PLAYERS, MAX_ROOM_PLAYERS};
use std::path::Path;

fn load() -> CollisionMap {
    CollisionMap::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/CombatLab.txt")).expect("map loads")
}

fn at(x: f32, y: f32) -> Vec3 {
    Vec3 { x, y, z: 92.0 }
}

/// 회전한 직사각형 안쪽 좌표 → 월드 좌표
fn local(cx: f32, cy: f32, yaw_degrees: f32, lx: f32, ly: f32) -> Vec3 {
    let r = yaw_degrees * std::f32::consts::PI / 180.0;
    at(cx + lx * r.cos() - ly * r.sin(), cy + lx * r.sin() + ly * r.cos())
}

#[test]
fn obstacle_count_45() {
    assert_eq!(load().obstacle_count(), 45);
}

#[test]
fn columns_walls_and_rocks() {
    let map = load();
    // 폐허 0: 중심(-1550,-850), 기둥 A(-1710,-850) 반지름 55, 받침 ±75 높이 40, 상인방 z 265~335
    assert!(map.blocked(at(-1710.0, -850.0), R), "column_center_blocked");
    assert!(map.blocked(at(-1710.0 + 55.0 + 20.0, -850.0), R), "column_edge_within_radius_blocked");
    assert!(!map.blocked(at(-1710.0 + 55.0 + 30.0, -850.0), R), "column_edge_outside_radius_free");
    assert!(!map.blocked(at(-1550.0, -850.0), R), "walk_under_lintel_free");
    assert!(!map.blocked(at(-1710.0 + 70.0, -850.0 + 70.0), R), "step_on_column_foot_free");

    // 부서진 벽 0: 중심(-1320,-690), 25도 회전, 반너비 135×35
    assert!(map.blocked(local(-1320.0, -690.0, 25.0, 0.0, 0.0), R), "rotated_wall_center_blocked");
    assert!(map.blocked(local(-1320.0, -690.0, 25.0, 120.0, 0.0), R), "rotated_wall_along_length_blocked");
    assert!(!map.blocked(local(-1320.0, -690.0, 25.0, 0.0, 35.0 + 30.0), R), "rotated_wall_beside_free");

    // 엄폐 바위 0(-2200,-1300): 구 충돌은 가장 작은 축 반지름 95
    assert!(map.blocked(at(-2200.0 + 95.0 + 20.0, -1300.0), R), "cover_rock_radius95_blocked");
    assert!(!map.blocked(at(-2200.0 + 95.0 + 30.0, -1300.0), R), "cover_rock_outside_free");

    // 보이지 않는 경계벽(x 2800~2820)
    assert!(map.blocked(at(2790.0, 0.0), R), "east_limit_blocked");
    assert!(!map.blocked(at(2760.0, 0.0), R), "near_east_limit_free");

    // 높이: 상인방 높이로 떠 있는 캡슐은 막힌다(높이 범위 검사)
    assert!(map.blocked(Vec3 { x: -1550.0, y: -850.0, z: 300.0 }, R), "lintel_height_blocked");
}

#[test]
fn movement_paths() {
    let map = load();
    // 이동 경로: 기둥을 뚫고 지나가면 막히고, 옆으로 지나가면 통과
    assert!(map.sweep_blocked(at(-1710.0, -1100.0), at(-1710.0, -600.0), SWEEP), "sweep_through_column_blocked");
    assert!(!map.sweep_blocked(at(-1610.0, -1100.0), at(-1610.0, -600.0), SWEEP), "sweep_beside_column_free");
    // 기둥 둘레를 15Hz 표본 간격(약 30cm)으로 도는 정상 이동은 거부되면 안 된다
    for i in 0..24 {
        let (a0, a1, radius) = (i as f32 * 0.35, (i + 1) as f32 * 0.35, 55.0 + 32.0);
        let from = at(-1710.0 + radius * a0.cos(), -850.0 + radius * a0.sin());
        let to = at(-1710.0 + radius * a1.cos(), -850.0 + radius * a1.sin());
        assert!(!map.sweep_blocked(from, to, SWEEP), "circle_around_column_free (step {i})");
    }
}

#[test]
fn line_of_sight() {
    let map = load();
    // 시야: 기둥 너머는 가리고, 빈 곳은 보인다. 상인방은 머리 위라 가리지 않는다
    assert!(map.line_blocked(at(-1710.0, -1100.0), at(-1710.0, -600.0)), "line_through_column_blocked");
    assert!(!map.line_blocked(at(0.0, 0.0), at(300.0, 0.0)), "line_open_free");
    assert!(!map.line_blocked(at(-1550.0, -1100.0), at(-1550.0, -600.0)), "line_under_lintel_free");
}

#[test]
fn all_spawns_free() {
    let map = load();
    // 시작 위치: 실제 캡슐 반지름(32)으로도 막히지 않아야 한다
    for slot in 0..MAX_ROOM_PLAYERS as usize {
        assert!(!map.blocked(spawn_position(RoomMode::PvE, slot), 32.0), "PvE slot {slot}");
    }
    for slot in 0..MAX_PVP_PLAYERS as usize {
        let mut p = spawn_position(RoomMode::PvP, slot);
        p.z = 92.0;
        assert!(!map.blocked(p, 32.0), "PvP slot {slot}");
    }
}
