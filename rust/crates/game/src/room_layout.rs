//! 방의 자리 배치: 시작 위치와 지형 검사 반지름. (C++ 서버 `Room`의 상수·`SpawnPosition`)

use asc_movement::GROUND_Z;
use asc_protocol::{Vec3, MAX_TEAMS};

/// 현재 데모 맵(개방형 폐허 정원)의 이동 범위. 클라이언트 `AAscensionMode::ArenaHalfWidth/Depth`와 같다.
pub const ARENA_HALF_WIDTH: f32 = 2800.0;
pub const ARENA_HALF_DEPTH: f32 = 2400.0;

/// 지형 검사 반지름. 클라이언트 캡슐(32)은 장애물에 파고들지 않으므로 여유를 두고 작게 본다.
/// 도착 위치는 24cm(8cm 여유), 이동 경로는 16cm로 쓸어 본다(기둥 모서리를 스치는 정상 이동은 통과).
pub const TERRAIN_RADIUS: f32 = 24.0;
pub const TERRAIN_SWEEP_RADIUS: f32 = 16.0;

/// 매치에서 팀 기지(팀 시작 위치)가 놓이는 원의 반지름. 팀 1~4가 대각선 네 방향(45·135·225·315도)에 선다.
pub const TEAM_BASE_RADIUS: f32 = 1700.0;
/// 같은 팀원 사이 간격
pub const TEAM_MEMBER_SPACING: f32 = 150.0;

/// 연습 방 `slot`번째 플레이어의 시작 위치: 클라이언트 데모의 시작 위치(-750, 0) 근처에 1.5m 간격으로 나란히 세운다.
pub fn practice_spawn_position(slot: usize) -> Vec3 {
    Vec3 { x: -750.0, y: -150.0 + 150.0 * slot as f32, z: GROUND_Z }
}

/// 매치 팀 `team`(1~4)의 `member`번째(0~2) 시작 위치. 기지 중심에서 원의 접선 방향으로 나란히 세운다.
pub fn team_spawn_position(team: u8, member: usize) -> Vec3 {
    let index = (team.clamp(1, MAX_TEAMS) - 1) as f32;
    let angle = (45.0 + 90.0 * index).to_radians();
    let (cos, sin) = (angle.cos(), angle.sin());
    let offset = (member as f32 - 1.0) * TEAM_MEMBER_SPACING;
    Vec3 { x: TEAM_BASE_RADIUS * cos - offset * sin, y: TEAM_BASE_RADIUS * sin + offset * cos, z: GROUND_Z }
}
