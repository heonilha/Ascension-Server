//! 방의 자리 배치: 시작 위치와 지형 검사 반지름. (C++ 서버 `Room`의 상수·`SpawnPosition`)

use asc_movement::GROUND_Z;
use asc_protocol::{RoomMode, Vec3, MAX_PVP_PLAYERS};

/// 현재 데모 맵(개방형 폐허 정원)의 이동 범위. 클라이언트 `AAscensionMode::ArenaHalfWidth/Depth`와 같다.
pub const ARENA_HALF_WIDTH: f32 = 2800.0;
pub const ARENA_HALF_DEPTH: f32 = 2400.0;

/// 지형 검사 반지름. 클라이언트 캡슐(32)은 장애물에 파고들지 않으므로 여유를 두고 작게 본다.
/// 도착 위치는 24cm(8cm 여유), 이동 경로는 16cm로 쓸어 본다(기둥 모서리를 스치는 정상 이동은 통과).
pub const TERRAIN_RADIUS: f32 = 24.0;
pub const TERRAIN_SWEEP_RADIUS: f32 = 16.0;

/// `slot`번째로 들어온 플레이어의 시작 위치.
pub fn spawn_position(mode: RoomMode, slot: usize) -> Vec3 {
    match mode {
        RoomMode::PvP => {
            // 맵 중앙을 둘러싼 반지름 6m 원 위에 고르게 세운다.
            let angle = 2.0 * std::f32::consts::PI * slot as f32 / MAX_PVP_PLAYERS as f32;
            Vec3 { x: 600.0 * angle.cos(), y: 600.0 * angle.sin(), z: GROUND_Z }
        }
        // 클라이언트 데모의 시작 위치(-750, 0) 근처에 1.5m 간격으로 나란히 세운다.
        RoomMode::PvE => Vec3 { x: -750.0, y: -150.0 + 150.0 * slot as f32, z: GROUND_Z },
    }
}
