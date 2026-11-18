//! 공유 이동 계산. 계산 자체는 C++ 원본(`protocol/AscensionMovement.h`)이 하고, 이 크레이트는 FFI로 부른다.
//!
//! 언리얼 클라이언트가 같은 헤더로 미리 움직이므로(예측), 서버가 같은 코드를 써야 같은 입력에서
//! 같은 위치가 나온다. 그래서 이동·지형 기하 계산은 Rust로 다시 쓰지 않는다.
//!
//! 상수는 편의를 위해 Rust에도 적어 두고, 헤더 값과 같은지 시험(`constants_match_header`)으로 확인한다.

use asc_protocol::{InputCommand, MoveSimState};
use std::ptr::NonNull;

pub const TICK_RATE: i32 = 60;
pub const TICK: f32 = 1.0 / TICK_RATE as f32;
/// 달리기 450cm/s
pub const RUN_SPEED: f32 = 450.0;
pub const ACCELERATION: f32 = 2048.0;
pub const BRAKING: f32 = 2048.0;
/// 주인공 캡슐 반지름
pub const RADIUS: f32 = 32.0;
/// 캡슐 반높이
pub const HALF_HEIGHT: f32 = 88.0;
/// z=0 바닥에 선 캡슐 중심 높이
pub const GROUND_Z: f32 = 90.0;
/// 회피: 0.65초(39틱) 동안 300cm
pub const DODGE_TICKS: i32 = 39;
pub const DODGE_DISTANCE: f32 = 300.0;
pub const DODGE_COOLDOWN_TICKS: i32 = TICK_RATE - 2;
/// `InputCommand::flags`의 회피 비트
pub const INPUT_DODGE: u8 = 1;

mod ffi {
    use asc_protocol::{InputCommand, MoveSimState};

    #[repr(C)]
    pub struct AscWorld {
        _opaque: [u8; 0],
    }

    #[repr(C)]
    pub struct ObstacleView {
        pub name: *const u8,
        pub name_length: usize,
        pub points: *const f32,
        pub point_count: usize,
        pub z_min: f32,
        pub z_max: f32,
        pub blocking: u8,
        pub solid: u8,
    }

    #[cfg(test)]
    #[repr(C)]
    #[derive(Default)]
    pub struct Constants {
        pub tick_rate: i32,
        pub dodge_ticks: i32,
        pub dodge_cooldown_ticks: i32,
        pub tick: f32,
        pub run_speed: f32,
        pub acceleration: f32,
        pub braking: f32,
        pub radius: f32,
        pub half_height: f32,
        pub ground_z: f32,
        pub dodge_distance: f32,
        pub input_dodge: u8,
        pub protocol_version: u16,
    }

    extern "C" {
        pub fn asc_world_parse(text: *const u8, length: usize, error: *mut u8, error_capacity: usize) -> *mut AscWorld;
        pub fn asc_world_free(world: *mut AscWorld);
        pub fn asc_world_obstacle_count(world: *const AscWorld) -> usize;
        pub fn asc_world_obstacle(world: *const AscWorld, index: usize, out: *mut ObstacleView) -> i32;
        pub fn asc_world_signed_distance(world: *const AscWorld, index: usize, x: f32, y: f32, outward: *mut f32) -> f32;
        pub fn asc_move_step(state: *mut MoveSimState, input: *const InputCommand, world: *const AscWorld);
        #[cfg(test)]
        pub fn asc_move_constants(out: *mut Constants);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// 장애물 하나. `World`에서 빌려 온다.
#[derive(Clone, Copy, Debug)]
pub struct Obstacle<'a> {
    pub name: &'a str,
    /// 윗면이 계단 높이(45cm)보다 높다. 공격 엄폐 판정용
    pub blocking: bool,
    /// 이동에서 단단한 장애물(낮은 받침 포함, 머리 위 구조물 제외)
    pub solid: bool,
    pub z_min: f32,
    pub z_max: f32,
    /// 반시계 볼록 다각형(cm)
    pub polygon: &'a [Vec2],
}

/// 충돌 지형. 서버 시작 때 한 번 읽고 이후에는 읽기만 한다.
pub struct World {
    raw: NonNull<ffi::AscWorld>,
}

// 만든 뒤에는 바꾸지 않고, C++ 쪽 함수도 읽기만 한다. 여러 방(태스크)에서 동시에 써도 된다.
unsafe impl Send for World {}
unsafe impl Sync for World {}

impl World {
    /// 서버용 충돌 파일(`data/CombatLab.txt`) 내용을 읽는다. 실패하면 이유를 돌려준다.
    pub fn parse(text: &str) -> Result<World, String> {
        let mut error = [0u8; 256];
        // SAFETY: text와 error는 호출 동안 유효하고 길이를 함께 넘긴다. C++ 쪽은 error를 널 문자로 끝낸다.
        let raw = unsafe { ffi::asc_world_parse(text.as_ptr(), text.len(), error.as_mut_ptr(), error.len()) };
        match NonNull::new(raw) {
            Some(raw) => Ok(World { raw }),
            None => {
                let length = error.iter().position(|&b| b == 0).unwrap_or(error.len());
                Err(String::from_utf8_lossy(&error[..length]).into_owned())
            }
        }
    }

    pub fn obstacle_count(&self) -> usize {
        // SAFETY: raw는 Drop 전까지 유효하다.
        unsafe { ffi::asc_world_obstacle_count(self.raw.as_ptr()) }
    }

    pub fn obstacle(&self, index: usize) -> Option<Obstacle<'_>> {
        let mut view = std::mem::MaybeUninit::<ffi::ObstacleView>::uninit();
        // SAFETY: 성공(1)하면 view가 모두 채워지고, 포인터는 self가 살아 있는 동안 유효하다(World는 바뀌지 않는다).
        // move::Vec2는 float 2개라 Vec2(#[repr(C)])와 배치가 같다.
        unsafe {
            if ffi::asc_world_obstacle(self.raw.as_ptr(), index, view.as_mut_ptr()) == 0 {
                return None;
            }
            let view = view.assume_init();
            let name = std::slice::from_raw_parts(view.name, view.name_length);
            Some(Obstacle {
                name: std::str::from_utf8(name).unwrap_or("?"),
                blocking: view.blocking != 0,
                solid: view.solid != 0,
                z_min: view.z_min,
                z_max: view.z_max,
                polygon: std::slice::from_raw_parts(view.points.cast::<Vec2>(), view.point_count),
            })
        }
    }

    pub fn obstacles(&self) -> impl Iterator<Item = Obstacle<'_>> {
        (0..self.obstacle_count()).filter_map(|index| self.obstacle(index))
    }

    /// 점에서 `index`번 장애물 경계까지 거리(안쪽이면 음수)와 밀어낼 방향(바깥쪽 단위 벡터).
    /// 경계 위에 정확히 있으면 방향은 (0, 0)이다.
    pub fn signed_distance(&self, index: usize, point: Vec2) -> (f32, Vec2) {
        assert!(index < self.obstacle_count(), "obstacle index {index} out of range");
        let mut outward = [0f32; 2];
        // SAFETY: index는 위에서 확인했고 outward는 float 2개다.
        let distance = unsafe { ffi::asc_world_signed_distance(self.raw.as_ptr(), index, point.x, point.y, outward.as_mut_ptr()) };
        (distance, Vec2 { x: outward[0], y: outward[1] })
    }
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World").field("obstacles", &self.obstacle_count()).finish()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        // SAFETY: asc_world_parse가 돌려준 포인터를 한 번만 해제한다.
        unsafe { ffi::asc_world_free(self.raw.as_ptr()) }
    }
}

/// 1틱(1/60초) 진행. 클라이언트 예측과 같은 계산이다.
pub fn step(state: &mut MoveSimState, input: &InputCommand, world: &World) {
    // SAFETY: 두 구조체는 헤더의 packed 구조체와 같은 배치이고(asc-protocol이 크기를 확인한다) C++ 쪽은 world를 읽기만 한다.
    unsafe { ffi::asc_move_step(state, input, world.raw.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: &str = "version 1\nobstacle Box 1 0.0 300.0 4 100.0 -100.0 300.0 -100.0 300.0 100.0 100.0 100.0\n";

    #[test]
    fn constants_match_header() {
        let mut c = ffi::Constants::default();
        // SAFETY: c는 C++의 AscMoveConstants와 같은 배치다.
        unsafe { ffi::asc_move_constants(&mut c) };
        assert_eq!(c.tick_rate, TICK_RATE);
        assert_eq!(c.dodge_ticks, DODGE_TICKS);
        assert_eq!(c.dodge_cooldown_ticks, DODGE_COOLDOWN_TICKS);
        assert_eq!(c.tick.to_bits(), TICK.to_bits());
        assert_eq!(c.run_speed, RUN_SPEED);
        assert_eq!(c.acceleration, ACCELERATION);
        assert_eq!(c.braking, BRAKING);
        assert_eq!(c.radius, RADIUS);
        assert_eq!(c.half_height, HALF_HEIGHT);
        assert_eq!(c.ground_z, GROUND_Z);
        assert_eq!(c.dodge_distance, DODGE_DISTANCE);
        assert_eq!(c.input_dodge, INPUT_DODGE);
        assert_eq!(c.protocol_version, asc_protocol::VERSION);
    }

    #[test]
    fn parse_reports_errors() {
        assert!(World::parse("version 2\n").unwrap_err().contains("version 2"));
        assert!(World::parse("version 1\nobstacle Bad 1 0 10 2 0 0 1 1\n").unwrap_err().contains("line 2"));
    }

    #[test]
    fn obstacles_are_exposed() {
        let world = World::parse(SQUARE).unwrap();
        assert_eq!(world.obstacle_count(), 1);
        let box_ = world.obstacle(0).unwrap();
        assert_eq!(box_.name, "Box");
        assert!(box_.blocking && box_.solid);
        assert_eq!((box_.z_min, box_.z_max), (0.0, 300.0));
        assert_eq!(box_.polygon.len(), 4);
        assert_eq!(box_.polygon[2], Vec2 { x: 300.0, y: 100.0 });
        assert!(world.obstacle(1).is_none());

        let (outside, normal) = world.signed_distance(0, Vec2 { x: 50.0, y: 0.0 });
        assert_eq!(outside, 50.0);
        assert_eq!(normal, Vec2 { x: -1.0, y: 0.0 });
        let (inside, _) = world.signed_distance(0, Vec2 { x: 120.0, y: 0.0 });
        assert_eq!(inside, -20.0);
    }

    #[test]
    fn running_into_a_wall_stops_at_radius() {
        let world = World::parse(SQUARE).unwrap();
        let mut state = MoveSimState { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, dodge_tick: 0, cooldown: 0, dodge_x: 0.0, dodge_y: 0.0 };
        let input = InputCommand { move_x: 1.0, move_y: 0.0, flags: 0, dodge_x: 0.0, dodge_y: 0.0 };
        for _ in 0..120 {
            step(&mut state, &input, &world);
        }
        // 벽(x=100)에서 반지름만큼 떨어진 곳에 멈추고, 벽 쪽 속도는 없어진다.
        assert!(({ state.x } - (100.0 - RADIUS)).abs() < 0.01, "x = {}", { state.x });
        assert!({ state.vx }.abs() < 0.01);
    }

    #[test]
    fn dodge_travels_fixed_distance_and_starts_cooldown() {
        let world = World::parse("version 1\nobstacle Far 1 0.0 300.0 3 9000.0 9000.0 9100.0 9000.0 9000.0 9100.0\n").unwrap();
        let mut state = MoveSimState { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, dodge_tick: 0, cooldown: 0, dodge_x: 0.0, dodge_y: 0.0 };
        let dodge = InputCommand { move_x: 0.0, move_y: 0.0, flags: INPUT_DODGE, dodge_x: 0.0, dodge_y: 2.0 };
        let idle = InputCommand { move_x: 0.0, move_y: 0.0, flags: 0, dodge_x: 0.0, dodge_y: 0.0 };
        step(&mut state, &dodge, &world);
        assert_eq!({ state.cooldown }, DODGE_COOLDOWN_TICKS as u16);
        for _ in 1..DODGE_TICKS {
            step(&mut state, &idle, &world);
        }
        assert_eq!({ state.dodge_tick }, 0);
        assert!(({ state.y } - DODGE_DISTANCE).abs() < 0.01, "y = {}", { state.y });
        assert_eq!({ state.x }, 0.0);
    }
}
