//! C++ 원본(`src/game`)이 만든 기준 기록과 Rust로 옮긴 계산을 비교한다.
//!
//! 기준 기록은 `tools/tests/game_golden.cpp`가 만든다. 입력 만드는 방법은 그 파일과 똑같아야 한다.
//! 이 시험이 깨지면 옮긴 코드의 계산이 C++ 원본과 달라진 것이다. 규칙을 일부러 고쳤다면 C++ 쪽도 같이 고치고
//! `Scripts\Test.ps1 -UpdateGolden`으로 기록을 다시 만든다(C++ 서버를 없앤 뒤에는 이 기록이 회귀 시험 기준이 된다).

use asc_game::monsters::{MonsterEvents, MonsterField, MonsterPlayer};
use asc_game::CollisionMap;
use asc_movement::Vec2;
use asc_protocol::Vec3;
use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

struct Hasher(u32);

impl Hasher {
    fn new() -> Hasher {
        Hasher(2166136261)
    }

    fn add(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.0 ^= byte as u32;
            self.0 = self.0.wrapping_mul(16777619);
        }
    }

    fn add_f(&mut self, value: f32) {
        self.add(value.to_bits());
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
fn rust_port_matches_cpp_golden() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let map = Arc::new(CollisionMap::load(&root.join("../../../data/CombatLab.txt")).expect("map loads"));
    let golden = std::fs::read_to_string(root.join("tests/golden/game_v5.txt")).expect("golden file");

    let mut actual = String::new();
    writeln!(actual, "# game golden: protocol v{}, {} obstacles", asc_protocol::VERSION, map.obstacle_count()).unwrap();

    // ---- 충돌 지형: 맵 전체 격자 ----
    {
        let mut hash = Hasher::new();
        let (mut blocked, mut high, mut solid, mut pushed) = (0, 0, 0, 0);
        for gx in (-2850..=2850).step_by(75) {
            for gy in (-2450..=2450).step_by(70) {
                let (x, y) = (gx as f32, gy as f32);
                let b = map.blocked(Vec3 { x, y, z: 92.0 }, 24.0);
                let h = map.blocked(Vec3 { x, y, z: 300.0 }, 24.0);
                let s = map.solid_overlap(Vec2 { x, y }, 35.0);
                let mut p = Vec2 { x, y };
                let m = map.push_out_solid(&mut p, 35.0);
                blocked += b as i32;
                high += h as i32;
                solid += s as i32;
                pushed += m as i32;
                for flag in [b, h, s, m] {
                    hash.add(flag as u32);
                }
                hash.add_f(p.x);
                hash.add_f(p.y);
            }
        }
        writeln!(actual, "collision grid blocked {blocked} high {high} solid {solid} pushed {pushed} hash {:08x}", hash.0).unwrap();
    }

    // ---- 충돌 지형: 두 점 사이 이동 경로·시야 ----
    {
        let mut hash = Hasher::new();
        let mut random = Random(77);
        let (mut sweeps, mut lines) = (0, 0);
        for i in 0..4000 {
            let (c, d, e, f) = (random.next(), random.next(), random.next(), random.next());
            let (ax, ay) = ((c % 5601) as i32 - 2800, (d % 4801) as i32 - 2400);
            let bx = if i % 2 == 0 { (e % 5601) as i32 - 2800 } else { ax + (e % 601) as i32 - 300 };
            let by = if i % 2 == 0 { (f % 4801) as i32 - 2400 } else { ay + (f % 601) as i32 - 300 };
            let (a, b) = (Vec3 { x: ax as f32, y: ay as f32, z: 92.0 }, Vec3 { x: bx as f32, y: by as f32, z: 92.0 });
            let (sweep, sight) = (map.sweep_blocked(a, b, 16.0), map.line_blocked(a, b));
            sweeps += sweep as i32;
            lines += sight as i32;
            hash.add(sweep as u32);
            hash.add(sight as u32);
        }
        writeln!(actual, "collision pairs sweep {sweeps} line {lines} hash {:08x}", hash.0).unwrap();
    }

    // ---- 몬스터·검기: 60초(1200틱) ----
    {
        let mut field = MonsterField::new(Some(map.clone()), 1, 0);
        field.set_spawning(false);
        let mut now = 0u32;
        for i in 0..12i32 {
            field.spawn_at((i % 3) as u8, (-900 + i * 150) as f32, if i % 2 == 0 { -500.0 } else { 500.0 }, now);
        }
        let mut counter = 0i32;
        let mut hash = Hasher::new();
        let mut events = MonsterEvents::default();
        let (mut tells, mut touches, mut hits, mut wave_ends) = (0, 0, 0, 0);
        for tick in 0..1200i32 {
            now += 50;
            let players = [
                MonsterPlayer { id: 1, x: (-750 + (tick % 400) * 3) as f32, y: 0.0, alive: true },
                MonsterPlayer { id: 2, x: 600.0, y: (400 - (tick % 300) * 2) as f32, alive: !(600..800).contains(&tick) },
            ];
            events.clear();
            if field.monsters().len() < 6 {
                field.spawn_at((counter % 3) as u8, (-1000 + (counter * 331) % 2000) as f32, (-800 + (counter * 197) % 1600) as f32, now);
                counter += 1;
            }
            if tick % 13 == 0 {
                field.spawn_wave(1, players[0].x, players[0].y, ((tick * 37) % 360) as f32, 20.0, 55.0, 1, 1200, now);
            }
            if tick % 41 == 0 {
                field.spawn_wave(2, players[1].x, players[1].y, ((tick * 53) % 360) as f32, 60.0, 150.0, 6, 1600, now);
            }
            field.update(now, 0.05, &players, &mut events);

            hash.add(field.monsters().len() as u32);
            for m in field.monsters() {
                hash.add(m.id as u32);
                hash.add(m.kind as u32);
                hash.add(m.phase.raw() as u32);
                for value in [m.x, m.y, m.yaw, m.hp] {
                    hash.add_f(value);
                }
                hash.add(m.deadline_ms);
                hash.add_f(m.knock_remaining);
                hash.add(m.stagger_until_ms);
            }
            hash.add(events.tells.len() as u32);
            for t in &events.tells {
                hash.add(t.monster_id as u32);
                hash.add(t.kind as u32);
                hash.add(t.cancel as u32);
                for value in [t.x, t.y, t.yaw] {
                    hash.add_f(value);
                }
                hash.add(t.start_ms);
                hash.add(t.hit_ms);
            }
            hash.add(events.dash_touches.len() as u32);
            for t in &events.dash_touches {
                hash.add(t.monster_id as u32);
                hash.add(t.player_id);
                hash.add_f(t.x);
                hash.add_f(t.y);
            }
            hash.add(events.hits.len() as u32);
            for h in &events.hits {
                hash.add(h.monster_id as u32);
                hash.add(h.attacker_id);
                for value in [h.damage, h.hp_after, h.x, h.y] {
                    hash.add_f(value);
                }
            }
            hash.add(events.wave_ends.len() as u32);
            for w in &events.wave_ends {
                hash.add(w.wave_id);
                hash.add_f(w.x);
                hash.add_f(w.y);
            }
            hash.add(field.wave_count() as u32);

            tells += events.tells.len();
            touches += events.dash_touches.len();
            hits += events.hits.len();
            wave_ends += events.wave_ends.len();
            if tick % 50 == 49 {
                writeln!(
                    actual,
                    "monsters tick {} count {} killed {} waves {} hash {:08x}",
                    tick + 1,
                    field.monsters().len(),
                    field.total_killed(),
                    field.wave_count(),
                    hash.0
                )
                .unwrap();
            }
        }
        writeln!(
            actual,
            "monsters total spawned {} killed {} tells {tells} touches {touches} hits {hits} wave_ends {wave_ends}",
            field.total_spawned(),
            field.total_killed()
        )
        .unwrap();
    }

    let expected: Vec<&str> = golden.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    for (line, (e, a)) in expected.iter().zip(&actual).enumerate() {
        assert_eq!(e, a, "기준 기록과 다르다: {}번째 줄", line + 1);
    }
    assert_eq!(expected.len(), actual.len(), "줄 수가 다르다");
    println!("{} lines match: {}", actual.len(), actual.last().unwrap());
}
