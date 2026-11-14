//! C++ 빌드(CMake)가 만든 기준 기록과 FFI 결과를 비트 단위로 비교한다.
//!
//! 기준 기록은 `tools/tests/movement_golden.cpp`가 만든다. 입력 만드는 방법은 그 파일과 똑같아야 한다.
//! 이 시험이 깨지면 (1) 이동 헤더를 고치고 기록을 다시 만들지 않았거나 (2) build.rs의 컴파일 옵션 때문에
//! 부동소수점 결과가 C++ 서버·클라이언트와 달라진 것이다. 기록 다시 만들기: `Scripts\Test.ps1 -UpdateGolden`

use asc_movement::{step, Vec2, World, INPUT_DODGE, RADIUS};
use asc_protocol::{InputCommand, MoveSimState};
use std::fmt::Write;
use std::path::Path;

fn hash(hash: &mut u32, bytes: &[u8]) {
    for &byte in bytes {
        *hash ^= byte as u32;
        *hash = hash.wrapping_mul(16777619);
    }
}

struct Random(u32);

impl Random {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0 >> 8
    }
}

#[test]
fn ffi_matches_cpp_golden() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let map = std::fs::read_to_string(root.join("../../../data/CombatLab.txt")).expect("data/CombatLab.txt");
    let world = World::parse(&map).expect("map parses");
    let golden = std::fs::read_to_string(root.join("tests/golden/movement_v4.txt")).expect("golden file");

    let mut actual = String::new();
    writeln!(actual, "# movement golden: protocol v{}, {} obstacles", asc_protocol::VERSION, world.obstacle_count()).unwrap();

    let solid: Vec<usize> = world.obstacles().enumerate().filter(|(_, o)| o.solid).map(|(i, _)| i).collect();
    let mut contact_ticks = 0;
    let mut dodges = 0;

    for run in 0..8i32 {
        let mut random = Random(1000 + run as u32);
        let mut state = MoveSimState {
            x: (run * 600 - 2100) as f32,
            y: if run % 2 == 0 { -700.0 } else { 700.0 },
            vx: 0.0,
            vy: 0.0,
            dodge_tick: 0,
            cooldown: 0,
            dodge_x: 0.0,
            dodge_y: 0.0,
        };
        let (mut mx, mut my) = (0f32, 0f32);
        let mut run_hash = 2166136261u32;
        for tick in 0..3600 {
            let (a, b, c) = (random.next(), random.next(), random.next());
            if tick % 20 == 0 {
                if a % 7 == 0 {
                    mx = 0.0;
                    my = 0.0;
                } else {
                    mx = ((a % 201) as i32 - 100) as f32 / 100.0;
                    my = ((b % 201) as i32 - 100) as f32 / 100.0;
                }
            }
            let input = InputCommand { move_x: mx, move_y: my, flags: if c % 97 == 0 { INPUT_DODGE } else { 0 }, dodge_x: mx, dodge_y: my };
            let was_dodging = state.dodge_tick != 0;
            step(&mut state, &input, &world);
            if !was_dodging && state.dodge_tick != 0 {
                dodges += 1;
            }

            // 기록과 별개로, 이 입력이 실제로 벽에 닿는지 센다(충돌 계산이 시험에 포함되는지 확인).
            let position = Vec2 { x: state.x, y: state.y };
            if solid.iter().any(|&index| world.signed_distance(index, position).0 <= RADIUS + 0.01) {
                contact_ticks += 1;
            }

            let (x, y, vx, vy, dodge_tick, cooldown, dodge_x, dodge_y) =
                (state.x, state.y, state.vx, state.vy, state.dodge_tick, state.cooldown, state.dodge_x, state.dodge_y);
            for value in [x, y, vx, vy] {
                hash(&mut run_hash, &value.to_le_bytes());
            }
            hash(&mut run_hash, &dodge_tick.to_le_bytes());
            hash(&mut run_hash, &cooldown.to_le_bytes());
            hash(&mut run_hash, &dodge_x.to_le_bytes());
            hash(&mut run_hash, &dodge_y.to_le_bytes());
            if tick % 300 == 299 {
                writeln!(
                    actual,
                    "run {run} tick {} {:08x} {:08x} {:08x} {:08x} {dodge_tick} {cooldown} {:08x} {:08x}",
                    tick + 1,
                    x.to_bits(),
                    y.to_bits(),
                    vx.to_bits(),
                    vy.to_bits(),
                    dodge_x.to_bits(),
                    dodge_y.to_bits()
                )
                .unwrap();
            }
        }
        writeln!(actual, "run {run} hash {run_hash:08x}").unwrap();
    }

    let mut distance_hash = 2166136261u32;
    for index in 0..world.obstacle_count() {
        for gx in (-2800..=2800).step_by(175) {
            for gy in (-2400..=2400).step_by(150) {
                let (distance, normal) = world.signed_distance(index, Vec2 { x: gx as f32, y: gy as f32 });
                for value in [distance, normal.x, normal.y] {
                    hash(&mut distance_hash, &value.to_le_bytes());
                }
            }
        }
    }
    writeln!(actual, "distance hash {distance_hash:08x}").unwrap();

    assert!(contact_ticks > 200, "입력이 벽에 거의 닿지 않는다: {contact_ticks}틱");
    assert!(dodges > 50, "회피가 거의 없다: {dodges}번");

    let expected: Vec<&str> = golden.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    for (line, (e, a)) in expected.iter().zip(&actual).enumerate() {
        assert_eq!(e, a, "기준 기록과 다르다: {}번째 줄", line + 1);
    }
    assert_eq!(expected.len(), actual.len(), "줄 수가 다르다");
    println!("{} lines match, contact ticks {contact_ticks}, dodges {dodges}", actual.len());
}
