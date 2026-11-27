//! PvE 방 하나의 몬스터와 플레이어 검기(투사체) 시뮬레이션.
//!
//! 1인 데모(클라이언트 CombatLab.cpp의 ACLEnemy·ACLBladeWave·TickSurvival)의 규칙을 서버로 옮긴 것이다.
//! - 종류: 0 망령 검병(부채꼴 베기), 1 폭주 망령(직선 돌진), 2 망령 지휘관(2연속 베기, 경직에 끊기지 않음)
//! - 추적: 가장 가까운 살아 있는 플레이어. 정면이 막히면 ±45·±90·±135도로 돌아간다.
//! - 캠프 몬스터(매치 필드): 공격받기 전에는 제자리에 서 있다. 맞으면 그 캠프 전체가 공격한 사람을 쫓고,
//!   캠프에서 너무 멀어지거나 대상을 놓치면 제자리로 돌아가 체력을 채운다(돌아가는 동안은 맞지 않는다).
//!   다 잡힌 캠프는 잠시 뒤 다시 채워진다.
//! - 피격: 경직 중 넉백(지형에 막히면 멈춤), 0.35초 동안은 다시 경직되지 않는다. 지휘관 외에는 예고가 끊긴다.
//! - 증원: 주기적 증원 + 목표 수 보충 + 60초마다 지휘관, 층당 최대 48마리.
//! - 검기: 가까운 순서로 관통하고 같은 몬스터를 두 번 맞히지 않는다. 벽에 막히고, 기둥 뒤 몬스터는 맞지 않는다.
//!
//! 네트워크를 모른다. 방이 `update`를 부르고, 생긴 일(`MonsterEvents`)을 패킷으로 바꿔 보낸다.
//! 플레이어에게 가는 피해는 여기서 정하지 않는다: 예고(`Tell`)와 돌진 접촉(`DashTouch`)을 알리면
//! 방이 회피 무적·지연 보정 판정을 거쳐 확정한다.
//!
//! 계산 순서는 C++ 원본(`src/game/Monsters.cpp`)과 같게 둔다. 증원을 끈 시나리오는 두 구현의 결과가
//! 비트 단위로 같은지 `tests/golden.rs`가 비교한다(증원은 난수 구현이 달라 규칙만 시험한다).

use crate::collision::CollisionMap;
use crate::field::CampSpot;
use crate::rng::Rng;
use crate::room_layout::{ARENA_HALF_DEPTH, ARENA_HALF_WIDTH, WORLD_HALF_DEPTH, WORLD_HALF_WIDTH};
use asc_movement::{Vec2, GROUND_Z};
use asc_protocol::{MonsterState, Vec3, MAX_MONSTERS};
use std::sync::Arc;

/// 몬스터 수치. 배열은 종류별 [검병, 폭주, 지휘관].
pub mod tuning {
    pub const MAX_HP: [f32; 3] = [50.0, 70.0, 150.0];
    /// 몬스터 캡슐 반지름
    pub const RADIUS: f32 = 35.0;
    pub const CHASE_SPEED: f32 = 220.0;
    /// 이 거리 앞까지 막혔는지 본다
    pub const STEER_PROBE: f32 = 110.0;
    pub const STEER_RADIUS: f32 = 30.0;
    // 베기(검병·지휘관)
    /// 이 거리 안이면 예고 시작
    pub const ARC_TRIGGER: f32 = 155.0;
    pub const ARC_RANGE: f32 = 180.0;
    pub const ARC_HALF_ANGLE: f32 = 55.0;
    pub const ARC_TELL_MS: u32 = 600;
    pub const ARC_SECOND_TELL_MS: u32 = 450;
    pub const ARC_DAMAGE: [f32; 3] = [10.0, 0.0, 16.0];
    pub const ARC_RECOVER_MS: [u32; 3] = [800, 0, 1500];
    // 돌진(폭주 망령)
    pub const CHARGE_TRIGGER: f32 = 360.0;
    pub const CHARGE_TELL_MS: u32 = 900;
    pub const DASH_MS: u32 = 500;
    pub const DASH_RECOVER_MS: u32 = 1200;
    pub const DASH_SPEED: f32 = 1200.0;
    /// 돌진 중 이 거리 안의 플레이어가 맞는다
    pub const DASH_HIT_DISTANCE: f32 = 105.0;
    pub const DASH_DAMAGE: f32 = 15.0;
    // 피격 반응
    pub const STAGGER_MS: u32 = 160;
    pub const ELITE_STAGGER_MS: u32 = 80;
    pub const REACTION_COOLDOWN_MS: u32 = 350;
    pub const ELITE_KNOCKBACK: f32 = 15.0;
    pub const KNOCKBACK_PER_DAMAGE: f32 = 1.5;
    pub const KNOCKBACK_MIN: f32 = 25.0;
    pub const KNOCKBACK_MAX: f32 = 65.0;
    // 증원(생존 모드 규칙)
    pub const SPAWN_MIN_DISTANCE: f32 = 650.0;
    pub const SPAWN_MAX_DISTANCE: f32 = 1250.0;
    /// 다른 몬스터와 이 이상 떨어져 생긴다
    pub const SPAWN_SEPARATION: f32 = 160.0;
    pub const SPAWN_CLEAR_RADIUS: f32 = 45.0;
    /// 다른 플레이어 바로 옆에는 생기지 않는다
    pub const SPAWN_PLAYER_DISTANCE: f32 = 400.0;
    // 검기
    pub const WAVE_SPEED: f32 = 1400.0;
    /// 검기 반지름에 더하는 몬스터 몸 크기
    pub const WAVE_HIT_PADDING: f32 = 32.0;
    pub const WAVE_WALL_RADIUS: f32 = 15.0;
    // 캠프(임시 수치)
    /// 다 잡힌 캠프가 다시 채워질 때까지
    pub const CAMP_RESPAWN_MS: u32 = 30_000;
    /// 캠프 중심에서 이보다 멀어지면 돌아간다
    pub const CAMP_LEASH: f32 = 1400.0;
    /// 쫓던 대상이 이보다 멀어지면 놓친다
    pub const CAMP_LOSE_DISTANCE: f32 = 1600.0;
    /// 제자리로 돌아가는 속도
    pub const RETURN_SPEED: f32 = 360.0;
}
use tuning as t;

const PI: f32 = std::f32::consts::PI;
const DEGREES: f32 = 180.0 / PI;

fn distance(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    (ax - bx).hypot(ay - by)
}

#[derive(Clone, Debug)]
pub struct Monster {
    pub id: u16,
    pub kind: u8,
    /// Idle·Chase·Tell·Dash·Recover (경직은 따로 겹친다)
    pub phase: MonsterState,
    pub x: f32,
    pub y: f32,
    /// 도
    pub yaw: f32,
    pub hp: f32,
    /// 지금 단계가 끝나는 시각
    pub deadline_ms: u32,
    // 예고·돌진 방향
    pub dir_x: f32,
    pub dir_y: f32,
    /// 지휘관의 몇 번째 베기인가
    pub strike: u8,
    pub stagger_until_ms: u32,
    pub reaction_ready_ms: u32,
    pub knock_x: f32,
    pub knock_y: f32,
    pub knock_remaining: f32,
    /// 이번 돌진에 이미 맞은 플레이어
    pub dash_hits: Vec<u32>,
    pub max_hp: f32,
    /// 공격 피해 배율(균열 보스 등). 1 = 기본
    pub power: f32,
    /// 0 필드, 1 균열 전투장. 같은 구역의 플레이어만 노린다
    pub zone: u8,
    /// 캠프 몬스터의 제자리. `None`이면 생존 모드 몬스터(항상 가장 가까운 플레이어를 쫓는다)
    pub home: Option<(f32, f32)>,
    /// 속한 캠프 번호
    pub camp: Option<usize>,
    /// 캠프 몬스터가 쫓는 플레이어
    pub aggro: Option<u32>,
    /// 제자리로 돌아가는 중(맞지 않는다)
    pub returning: bool,
}

impl Monster {
    /// 클라이언트에 보여 줄 상태(경직이 단계보다 우선한다).
    pub fn shown_state(&self, now_ms: u32) -> MonsterState {
        if now_ms < self.stagger_until_ms {
            MonsterState::Stagger
        } else {
            self.phase
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonsterPlayer {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub alive: bool,
    /// 0 필드, 1 균열 전투장
    pub zone: u8,
}

/// 공격 예고 시작(`hit_ms`에 베기 명중 또는 돌진 출발) 또는 취소(`cancel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tell {
    pub monster_id: u16,
    pub kind: u8,
    pub cancel: bool,
    pub x: f32,
    pub y: f32,
    pub yaw: f32,
    pub start_ms: u32,
    pub hit_ms: u32,
    /// 피해 배율(`Monster::power`)
    pub power: f32,
}

impl Tell {
    fn canceled(monster: &Monster) -> Tell {
        Tell { monster_id: monster.id, kind: monster.kind, cancel: true, x: 0.0, y: 0.0, yaw: 0.0, start_ms: 0, hit_ms: 0, power: monster.power }
    }
}

/// 돌진하는 몬스터가 플레이어에 닿았다.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DashTouch {
    pub monster_id: u16,
    pub player_id: u32,
    pub x: f32,
    pub y: f32,
    pub power: f32,
}

/// 몬스터가 피해를 입었다(`hp_after` 0 = 사망).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub monster_id: u16,
    /// 0 망령 검병, 1 폭주 망령, 2 망령 지휘관(처치 경험치 계산용)
    pub kind: u8,
    pub attacker_id: u32,
    pub damage: f32,
    pub hp_after: f32,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveEnd {
    pub wave_id: u32,
    pub x: f32,
    pub y: f32,
}

/// `update`·피해 처리 중에 생긴 일. 방이 읽어 패킷과 공격 판정으로 바꾼다.
#[derive(Debug, Default)]
pub struct MonsterEvents {
    /// 일어난 순서대로 쌓인다: 같은 틱에 예고를 시작하고 곧바로 맞아 끊길 수 있으므로 순서를 지켜 처리해야 한다.
    pub tells: Vec<Tell>,
    pub dash_touches: Vec<DashTouch>,
    pub hits: Vec<Hit>,
    pub wave_ends: Vec<WaveEnd>,
}

impl MonsterEvents {
    pub fn clear(&mut self) {
        self.tells.clear();
        self.dash_touches.clear();
        self.hits.clear();
        self.wave_ends.clear();
    }
}

struct Wave {
    id: u32,
    owner_id: u32,
    x: f32,
    y: f32,
    dir_x: f32,
    dir_y: f32,
    damage: f32,
    radius: f32,
    pierces: i32,
    end_ms: u32,
    hit_monsters: Vec<u16>,
}

/// 지형 검사 묶음. 지형이 없으면(지형 끔) 맵 범위만 지킨다.
#[derive(Clone, Copy)]
struct Terrain<'a>(Option<&'a CollisionMap>);

impl Terrain<'_> {
    /// 이동 범위(반폭, 반높이). 지형이 있으면 필드 + 균열 전투장 전체(필드 경계는 벽이 막는다), 없으면 필드만.
    fn half(self) -> (f32, f32) {
        if self.0.is_some() {
            (WORLD_HALF_WIDTH, WORLD_HALF_DEPTH)
        } else {
            (ARENA_HALF_WIDTH, ARENA_HALF_DEPTH)
        }
    }

    /// 단단한 장애물 또는 맵 밖
    fn overlaps(self, x: f32, y: f32, radius: f32) -> bool {
        let (half_width, half_depth) = self.half();
        if x.abs() > half_width - radius || y.abs() > half_depth - radius {
            return true;
        }
        self.0.is_some_and(|map| map.solid_overlap(Vec2 { x, y }, radius))
    }

    fn sweep_blocked(self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32) -> bool {
        let length = distance(x0, y0, x1, y1);
        let steps = ((length / 10.0).ceil() as i32).max(1);
        (1..=steps).any(|i| {
            let t = i as f32 / steps as f32;
            self.overlaps(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, radius)
        })
    }

    fn push_out(self, x: &mut f32, y: &mut f32, radius: f32) {
        if let Some(map) = self.0 {
            let mut p = Vec2 { x: *x, y: *y };
            if map.push_out_solid(&mut p, radius) {
                *x = p.x;
                *y = p.y;
            }
        }
        let (half_width, half_depth) = self.half();
        *x = x.clamp(-half_width + radius, half_width - radius);
        *y = y.clamp(-half_depth + radius, half_depth - radius);
    }

    fn line_of_sight(self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        self.0.is_none_or(|map| !map.line_blocked(Vec3 { x: x0, y: y0, z: GROUND_Z }, Vec3 { x: x1, y: y1, z: GROUND_Z }))
    }
}

struct Camp {
    spot: CampSpot,
    /// 다 잡혔다: 이 시각에 다시 채운다
    respawn_at_ms: Option<u32>,
}

pub struct MonsterField {
    map: Option<Arc<CollisionMap>>,
    camps: Vec<Camp>,
    rng: Rng,
    monsters: Vec<Monster>,
    waves: Vec<Wave>,
    next_monster_id: u16,
    next_wave_id: u32,
    start_ms: u32,
    reinforcement_ready_ms: u32,
    refill_ready_ms: u32,
    elite_ready_ms: u32,
    total_spawned: u32,
    total_killed: u32,
    spawning: bool,
}

impl MonsterField {
    /// `map`이 없으면(지형 끔) 맵 범위만 지킨다.
    pub fn new(map: Option<Arc<CollisionMap>>, seed: u32, now_ms: u32) -> MonsterField {
        MonsterField {
            map,
            camps: Vec::new(),
            rng: Rng::new(seed),
            monsters: Vec::new(),
            waves: Vec::new(),
            next_monster_id: 1,
            next_wave_id: 1,
            start_ms: now_ms,
            reinforcement_ready_ms: now_ms.wrapping_add(2500),
            refill_ready_ms: now_ms.wrapping_add(1000),
            elite_ready_ms: now_ms.wrapping_add(60000),
            total_spawned: 0,
            total_killed: 0,
            spawning: true,
        }
    }

    pub fn monsters(&self) -> &[Monster] {
        &self.monsters
    }

    pub fn monster(&self, id: u16) -> Option<&Monster> {
        self.monsters.iter().find(|m| m.id == id)
    }

    pub fn monster_mut(&mut self, id: u16) -> Option<&mut Monster> {
        self.monsters.iter_mut().find(|m| m.id == id)
    }

    pub fn wave_count(&self) -> usize {
        self.waves.len()
    }

    pub fn total_spawned(&self) -> u32 {
        self.total_spawned
    }

    pub fn total_killed(&self) -> u32 {
        self.total_killed
    }

    /// 증원을 켜거나 끈다(시험·`--pve-monsters 0`).
    pub fn set_spawning(&mut self, enabled: bool) {
        self.spawning = enabled;
    }

    fn terrain(&self) -> Terrain<'_> {
        Terrain(self.map.as_deref())
    }

    /// 원하는 자리에 몬스터를 놓는다(증원 규칙을 거치지 않는다). 증원도 자리를 정한 뒤 이 함수를 쓴다.
    pub fn spawn_at(&mut self, kind: u8, x: f32, y: f32, now_ms: u32) -> &mut Monster {
        let id = self.next_monster_id;
        self.next_monster_id = self.next_monster_id.wrapping_add(1);
        if self.next_monster_id == 0 {
            self.next_monster_id = 1; // 0은 "몬스터 아님"으로 쓴다
        }
        let kind = kind.min(2);
        self.total_spawned += 1;
        self.monsters.push(Monster {
            id,
            kind,
            phase: MonsterState::Chase,
            x,
            y,
            yaw: 0.0,
            hp: t::MAX_HP[kind as usize],
            deadline_ms: now_ms,
            dir_x: 1.0,
            dir_y: 0.0,
            strike: 0,
            stagger_until_ms: 0,
            reaction_ready_ms: 0,
            knock_x: 0.0,
            knock_y: 0.0,
            knock_remaining: 0.0,
            dash_hits: Vec::new(),
            max_hp: t::MAX_HP[kind as usize],
            power: 1.0,
            zone: 0,
            home: None,
            camp: None,
            aggro: None,
            returning: false,
        });
        self.monsters.last_mut().unwrap()
    }

    /// 몬스터 하나를 없앤다(균열 보스 정리 등). 예고 중이면 취소를 알린다.
    pub fn remove(&mut self, id: u16, events: &mut MonsterEvents) {
        if let Some(index) = self.monsters.iter().position(|m| m.id == id) {
            if self.monsters[index].phase == MonsterState::Tell {
                events.tells.push(Tell::canceled(&self.monsters[index]));
            }
            self.monsters.remove(index);
        }
    }

    /// 캠프를 등록하고 바로 채운다(매치 필드). 캠프 몬스터는 공격받기 전에는 움직이지 않는다.
    pub fn set_camps(&mut self, spots: &[CampSpot], now_ms: u32) {
        self.monsters.retain(|m| m.camp.is_none());
        self.camps = spots.iter().map(|spot| Camp { spot: spot.clone(), respawn_at_ms: None }).collect();
        for index in 0..self.camps.len() {
            self.fill_camp(index, now_ms);
        }
    }

    fn fill_camp(&mut self, index: usize, now_ms: u32) {
        let spot = self.camps[index].spot.clone();
        let count = spot.kinds.len().max(1);
        for (k, &kind) in spot.kinds.iter().enumerate() {
            if self.monsters.len() >= MAX_MONSTERS as usize {
                return;
            }
            // 캠프 중심 둘레에 고르게. 막힌 자리면 조금씩 돌려 본다.
            let mut place = (spot.x, spot.y);
            for attempt in 0..12 {
                let angle = (k as f32 + attempt as f32 * 0.37) * 2.0 * PI / count as f32;
                let ring = spot.radius * if kind == 2 { 0.15 } else { 0.55 };
                let (x, y) = (spot.x + ring * angle.cos(), spot.y + ring * angle.sin());
                if !self.terrain().overlaps(x, y, t::SPAWN_CLEAR_RADIUS) {
                    place = (x, y);
                    break;
                }
            }
            let monster = self.spawn_at(kind, place.0, place.1, now_ms);
            monster.home = Some(place);
            monster.camp = Some(index);
            monster.phase = MonsterState::Idle;
            monster.yaw = (place.1 - spot.y).atan2(place.0 - spot.x) * DEGREES;
        }
    }

    fn update_camps(&mut self, now_ms: u32) {
        for index in 0..self.camps.len() {
            if self.monsters.iter().any(|m| m.camp == Some(index)) {
                continue;
            }
            match self.camps[index].respawn_at_ms {
                None => self.camps[index].respawn_at_ms = Some(now_ms.wrapping_add(t::CAMP_RESPAWN_MS)),
                Some(at) if now_ms >= at => {
                    self.camps[index].respawn_at_ms = None;
                    self.fill_camp(index, now_ms);
                }
                Some(_) => {}
            }
        }
    }

    /// 한 틱 진행: 증원 → 몬스터 AI·이동 → 검기. `dt`는 초.
    pub fn update(&mut self, now_ms: u32, dt: f32, players: &[MonsterPlayer], events: &mut MonsterEvents) {
        if self.spawning {
            self.update_spawning(now_ms, players);
        }
        if !self.camps.is_empty() {
            self.update_camps(now_ms);
        }
        let terrain = Terrain(self.map.as_deref());
        for monster in &mut self.monsters {
            update_monster(terrain, monster, now_ms, dt, players, events);
        }
        self.separate();
        self.update_waves(now_ms, dt, events); // 몬스터를 지울 수 있으므로 마지막에 한다
    }

    // ---- 증원 ----

    fn update_spawning(&mut self, now_ms: u32, players: &[MonsterPlayer]) {
        if !players.iter().any(|p| p.alive) {
            return;
        }
        let elapsed = now_ms.wrapping_sub(self.start_ms) as f32 / 1000.0;
        if now_ms >= self.reinforcement_ready_ms {
            self.spawn_batch((4 + (elapsed / 60.0) as i32).min(8), false, now_ms, players);
            self.reinforcement_ready_ms = now_ms.wrapping_add(((2.5 - elapsed / 240.0).max(1.5) * 1000.0) as u32);
        }
        if now_ms >= self.refill_ready_ms {
            let target = (12 + (elapsed / 30.0) as i32 * 2).min(28);
            self.spawn_batch((target - self.monsters.len() as i32).clamp(0, 4), false, now_ms, players);
            self.refill_ready_ms = now_ms.wrapping_add(1000);
        }
        if now_ms >= self.elite_ready_ms {
            self.spawn_batch(1, true, now_ms, players);
            self.elite_ready_ms = now_ms.wrapping_add(60000);
        }
    }

    fn spawn_batch(&mut self, count: i32, elite: bool, now_ms: u32, players: &[MonsterPlayer]) {
        let alive: Vec<MonsterPlayer> = players.iter().copied().filter(|p| p.alive).collect();
        if alive.is_empty() {
            return;
        }
        let elapsed = now_ms.wrapping_sub(self.start_ms) as f32 / 1000.0;

        for _ in 0..count {
            if self.monsters.len() >= MAX_MONSTERS as usize {
                break;
            }
            for _attempt in 0..150 {
                // 플레이어 한 명을 골라 그 둘레 6.5~12.5m 고리 안에 놓는다.
                let anchor = alive[self.rng.index(alive.len())];
                let angle = self.rng.range(0.0, 2.0 * PI);
                let ring = self.rng.range(t::SPAWN_MIN_DISTANCE, t::SPAWN_MAX_DISTANCE);
                let (x, y) = (anchor.x + ring * angle.cos(), anchor.y + ring * angle.sin());
                if x.abs() > ARENA_HALF_WIDTH - 100.0 || y.abs() > ARENA_HALF_DEPTH - 100.0 {
                    continue;
                }
                if self.terrain().overlaps(x, y, t::SPAWN_CLEAR_RADIUS) {
                    continue;
                }
                if alive.iter().any(|p| distance(x, y, p.x, p.y) < t::SPAWN_PLAYER_DISTANCE) {
                    continue;
                }
                if self.monsters.iter().any(|m| distance(x, y, m.x, m.y) < t::SPAWN_SEPARATION) {
                    continue;
                }
                let kind = if elite {
                    2
                } else if self.rng.range(0.0, 1.0) < (if elapsed < 120.0 { 0.25 } else { 0.5 }) {
                    1
                } else {
                    0
                };
                self.spawn_at(kind, x, y, now_ms);
                break;
            }
        }
    }

    /// 서로 겹친 몬스터를 떼어 놓는다. 예고·돌진·경직 중인 몬스터는 자리를 지키고 다른 쪽이 비킨다.
    fn separate(&mut self) {
        fn movable(m: &Monster) -> bool {
            m.knock_remaining <= 0.0 && matches!(m.phase, MonsterState::Chase | MonsterState::Recover | MonsterState::Idle)
        }
        const MIN_DISTANCE: f32 = t::RADIUS * 2.0;
        let terrain = Terrain(self.map.as_deref());
        let monsters = &mut self.monsters;
        for i in 0..monsters.len() {
            for j in i + 1..monsters.len() {
                let (left, right) = monsters.split_at_mut(j);
                let (a, b) = (&mut left[i], &mut right[0]);
                let (mut dx, mut dy) = (b.x - a.x, b.y - a.y);
                if dx.abs() >= MIN_DISTANCE || dy.abs() >= MIN_DISTANCE {
                    continue;
                }
                let mut d = dx.hypot(dy);
                if d >= MIN_DISTANCE {
                    continue;
                }
                let (move_a, move_b) = (movable(a), movable(b));
                if !move_a && !move_b {
                    continue;
                }
                if d < 1e-3 {
                    // 완전히 겹쳤으면 아무 방향으로나
                    dx = 1.0;
                    dy = 0.0;
                    d = 1.0;
                }
                let push = MIN_DISTANCE - d.min(MIN_DISTANCE);
                let share_a: f32 = if move_a {
                    if move_b {
                        0.5
                    } else {
                        1.0
                    }
                } else {
                    0.0
                };
                a.x -= dx / d * push * share_a;
                a.y -= dy / d * push * share_a;
                b.x += dx / d * push * (1.0 - share_a);
                b.y += dy / d * push * (1.0 - share_a);
                if move_a {
                    terrain.push_out(&mut a.x, &mut a.y, t::RADIUS);
                }
                if move_b {
                    terrain.push_out(&mut b.x, &mut b.y, t::RADIUS);
                }
            }
        }
    }

    /// 몬스터 하나에 피해. 없는 id면 false.
    #[allow(clippy::too_many_arguments)]
    pub fn damage(&mut self, monster_id: u16, attacker_id: u32, damage: f32, push_x: f32, push_y: f32, now_ms: u32, events: &mut MonsterEvents) -> bool {
        let Some(index) = self.monsters.iter().position(|m| m.id == monster_id) else {
            return false;
        };
        if self.monsters[index].returning {
            return true; // 제자리로 돌아가는 중에는 맞지 않는다
        }
        // 캠프 몬스터: 맞으면 그 캠프 전체가 공격한 사람을 쫓는다
        if let (Some(camp), true) = (self.monsters[index].camp, attacker_id != 0) {
            for other in &mut self.monsters {
                if other.camp == Some(camp) && !other.returning && other.aggro.is_none() {
                    other.aggro = Some(attacker_id);
                    if other.phase == MonsterState::Idle {
                        other.phase = MonsterState::Chase;
                    }
                }
            }
        }
        let monster = &mut self.monsters[index];
        monster.hp = (monster.hp - damage).max(0.0);
        events.hits.push(Hit { monster_id: monster.id, kind: monster.kind, attacker_id, damage, hp_after: monster.hp, x: monster.x, y: monster.y });

        if monster.hp <= 0.0 {
            if monster.phase == MonsterState::Tell {
                events.tells.push(Tell::canceled(monster));
            }
            self.monsters.remove(index);
            self.total_killed += 1;
            return true;
        }
        if now_ms < monster.reaction_ready_ms {
            return true; // 방금 경직됐으면 피해만 받는다
        }

        let elite = monster.kind == 2;
        let duration = if elite { t::ELITE_STAGGER_MS } else { t::STAGGER_MS };
        monster.stagger_until_ms = now_ms.wrapping_add(duration);
        monster.reaction_ready_ms = now_ms.wrapping_add(t::REACTION_COOLDOWN_MS);
        let length = push_x.hypot(push_y);
        monster.knock_x = if length > 1e-3 { push_x / length } else { 0.0 };
        monster.knock_y = if length > 1e-3 { push_y / length } else { 0.0 };
        monster.knock_remaining = if elite { t::ELITE_KNOCKBACK } else { (damage * t::KNOCKBACK_PER_DAMAGE).clamp(t::KNOCKBACK_MIN, t::KNOCKBACK_MAX) };

        let telling = monster.phase == MonsterState::Tell;
        if !elite {
            // 검병·폭주 망령: 하던 공격이 끊기고 경직이 끝나면 다시 추적한다.
            if telling {
                events.tells.push(Tell::canceled(monster));
            }
            monster.phase = MonsterState::Recover;
            monster.deadline_ms = monster.stagger_until_ms;
            monster.strike = 0;
        } else {
            // 지휘관: 끊기지 않고 경직 시간만큼 늦어진다. 예고 중이면 새 명중 시각으로 다시 알린다.
            monster.deadline_ms = monster.deadline_ms.wrapping_add(duration);
            if telling {
                events.tells.push(Tell::canceled(monster));
                events.tells.push(Tell {
                    monster_id: monster.id,
                    kind: monster.kind,
                    cancel: false,
                    x: monster.x,
                    y: monster.y,
                    yaw: monster.yaw,
                    start_ms: now_ms,
                    hit_ms: monster.deadline_ms,
                    power: monster.power,
                });
            }
        }
        true
    }

    // ---- 검기 ----

    /// 검기 발사. 만든 검기 id를 돌려준다.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_wave(&mut self, owner_id: u32, x: f32, y: f32, yaw_degrees: f32, damage: f32, radius: f32, pierces: i32, life_ms: u32, now_ms: u32) -> u32 {
        let id = self.next_wave_id;
        self.next_wave_id = self.next_wave_id.wrapping_add(1);
        self.waves.push(Wave {
            id,
            owner_id,
            x,
            y,
            dir_x: (yaw_degrees / DEGREES).cos(),
            dir_y: (yaw_degrees / DEGREES).sin(),
            damage,
            radius,
            pierces: pierces.max(1),
            end_ms: now_ms.wrapping_add(life_ms),
            hit_monsters: Vec::new(),
        });
        id
    }

    fn update_waves(&mut self, now_ms: u32, dt: f32, events: &mut MonsterEvents) {
        struct Contact {
            monster_id: u16,
            /// 이번 구간에서 검기가 그 몬스터에 닿기 시작하는 거리
            entry: f32,
        }
        let mut contacts: Vec<Contact> = Vec::new();

        let mut index = 0;
        while index < self.waves.len() {
            let terrain = Terrain(self.map.as_deref());
            let wave = &self.waves[index];
            // 이번 틱에 갈 구간. 벽이나 맵 끝에 막히면 그 앞에서 멈춘다.
            let mut length = t::WAVE_SPEED * dt;
            let mut blocked = false;
            let mut s = 10.0f32;
            while s <= length {
                let (x, y) = (wave.x + wave.dir_x * s, wave.y + wave.dir_y * s);
                let (half_width, half_depth) = terrain.half();
                let outside = x.abs() > half_width || y.abs() > half_depth;
                if outside || terrain.0.is_some_and(|map| map.blocked(Vec3 { x, y, z: GROUND_Z }, t::WAVE_WALL_RADIUS)) {
                    blocked = true;
                    length = s - 10.0;
                    break;
                }
                s += 10.0;
            }
            let (end_x, end_y) = (wave.x + wave.dir_x * length, wave.y + wave.dir_y * length);

            // 구간에 걸친 몬스터를 가까운 순서로. 이미 맞힌 몬스터와 기둥 뒤 몬스터는 뺀다.
            contacts.clear();
            let reach = wave.radius + t::WAVE_HIT_PADDING;
            for monster in &self.monsters {
                if wave.hit_monsters.contains(&monster.id) {
                    continue;
                }
                let (offset_x, offset_y) = (monster.x - wave.x, monster.y - wave.y);
                let along = offset_x * wave.dir_x + offset_y * wave.dir_y;
                let nearest = along.clamp(0.0, length);
                let (gap_x, gap_y) = (offset_x - wave.dir_x * nearest, offset_y - wave.dir_y * nearest);
                if gap_x * gap_x + gap_y * gap_y > reach * reach || !terrain.line_of_sight(wave.x, wave.y, monster.x, monster.y) {
                    continue;
                }
                let side_squared = (offset_x * offset_x + offset_y * offset_y - along * along).max(0.0);
                contacts.push(Contact { monster_id: monster.id, entry: (along - (reach * reach - side_squared).max(0.0).sqrt()).clamp(0.0, length) });
            }
            contacts.sort_by(|a, b| a.entry.partial_cmp(&b.entry).unwrap_or(std::cmp::Ordering::Equal).then(a.monster_id.cmp(&b.monster_id)));

            let mut spent = false;
            let (mut stop_x, mut stop_y) = (end_x, end_y);
            let (wave_id, owner_id, damage, dir_x, dir_y, start_x, start_y) = (wave.id, wave.owner_id, wave.damage, wave.dir_x, wave.dir_y, wave.x, wave.y);
            for contact in &contacts {
                self.waves[index].hit_monsters.push(contact.monster_id);
                self.damage(contact.monster_id, owner_id, damage, dir_x, dir_y, now_ms, events);
                self.waves[index].pierces -= 1;
                if self.waves[index].pierces <= 0 {
                    spent = true;
                    stop_x = start_x + dir_x * contact.entry;
                    stop_y = start_y + dir_y * contact.entry;
                    break;
                }
            }

            if spent || blocked {
                events.wave_ends.push(WaveEnd { wave_id, x: stop_x, y: stop_y });
                self.waves.remove(index);
                continue;
            }
            self.waves[index].x = end_x;
            self.waves[index].y = end_y;
            if now_ms >= self.waves[index].end_ms {
                self.waves.remove(index); // 수명이 다함: 클라이언트도 life_ms로 안다
                continue;
            }
            index += 1;
        }
    }
}

// ---- 몬스터 한 마리 ----

fn begin_tell(monster: &mut Monster, target: &MonsterPlayer, start_ms: u32, events: &mut MonsterEvents) {
    let (to_x, to_y) = (target.x - monster.x, target.y - monster.y);
    let length = to_x.hypot(to_y);
    if length > 1e-3 {
        monster.dir_x = to_x / length;
        monster.dir_y = to_y / length;
    }
    monster.yaw = monster.dir_y.atan2(monster.dir_x) * DEGREES;
    monster.phase = MonsterState::Tell;
    let tell_ms = if monster.kind == 1 {
        t::CHARGE_TELL_MS
    } else if monster.strike == 1 {
        t::ARC_SECOND_TELL_MS
    } else {
        t::ARC_TELL_MS
    };
    monster.deadline_ms = start_ms.wrapping_add(tell_ms);
    events.tells.push(Tell {
        monster_id: monster.id,
        kind: monster.kind,
        cancel: false,
        x: monster.x,
        y: monster.y,
        yaw: monster.yaw,
        start_ms,
        hit_ms: monster.deadline_ms,
        power: monster.power,
    });
}

fn chase(terrain: Terrain, monster: &mut Monster, to_x: f32, to_y: f32, dt: f32) {
    walk(terrain, monster, to_x, to_y, t::CHASE_SPEED, dt);
}

fn walk(terrain: Terrain, monster: &mut Monster, to_x: f32, to_y: f32, speed: f32, dt: f32) {
    let length = to_x.hypot(to_y);
    if length < 1e-3 {
        return;
    }
    let (desired_x, desired_y) = (to_x / length, to_y / length);
    // 정면부터 좌우로 넓혀 가며 막히지 않은 첫 방향으로 간다.
    for degrees in [0.0f32, 45.0, -45.0, 90.0, -90.0, 135.0, -135.0] {
        let (c, s) = ((degrees / DEGREES).cos(), (degrees / DEGREES).sin());
        let (dir_x, dir_y) = (desired_x * c - desired_y * s, desired_x * s + desired_y * c);
        if terrain.sweep_blocked(monster.x, monster.y, monster.x + dir_x * t::STEER_PROBE, monster.y + dir_y * t::STEER_PROBE, t::STEER_RADIUS) {
            continue;
        }
        if speed * dt >= length {
            // 목적지에 닿는다(제자리로 돌아가기)
            monster.x += to_x;
            monster.y += to_y;
        } else {
            monster.x += dir_x * speed * dt;
            monster.y += dir_y * speed * dt;
        }
        monster.yaw = dir_y.atan2(dir_x) * DEGREES;
        break;
    }
    terrain.push_out(&mut monster.x, &mut monster.y, t::RADIUS);
}

fn update_monster(terrain: Terrain, monster: &mut Monster, now_ms: u32, dt: f32, players: &[MonsterPlayer], events: &mut MonsterEvents) {
    // 경직: AI를 멈추고 남은 넉백 거리를 경직 시간에 나눠 민다. 지형에 막히면 거기서 끝난다.
    if now_ms < monster.stagger_until_ms {
        if monster.knock_remaining > 0.0 {
            let remaining_time = (monster.stagger_until_ms - now_ms) as f32 / 1000.0;
            let step = monster.knock_remaining.min(monster.knock_remaining * dt / dt.max(remaining_time + dt));
            let (x, y) = (monster.x + monster.knock_x * step, monster.y + monster.knock_y * step);
            if terrain.overlaps(x, y, t::RADIUS) {
                monster.knock_remaining = 0.0;
            } else {
                monster.x = x;
                monster.y = y;
                monster.knock_remaining -= step;
            }
        }
        return;
    }
    if monster.knock_remaining > 0.0 {
        // 경직이 틱 사이에 끝났다: 남은 넉백을 마저 민다(20Hz 틱에서 마지막 조각을 잃지 않게).
        let (x, y) = (monster.x + monster.knock_x * monster.knock_remaining, monster.y + monster.knock_y * monster.knock_remaining);
        if !terrain.overlaps(x, y, t::RADIUS) {
            monster.x = x;
            monster.y = y;
        }
        monster.knock_remaining = 0.0;
    }

    // 캠프 몬스터: 제자리로 돌아가는 중이거나, 쫓을 대상이 없거나 너무 멀리 왔으면 돌아간다.
    if let Some((home_x, home_y)) = monster.home {
        if !monster.returning && monster.phase != MonsterState::Dash {
            let target = monster.aggro.and_then(|id| players.iter().find(|p| p.id == id && p.alive && p.zone == monster.zone));
            let lost = match target {
                Some(p) => distance(p.x, p.y, monster.x, monster.y) > t::CAMP_LOSE_DISTANCE || distance(monster.x, monster.y, home_x, home_y) > t::CAMP_LEASH,
                None => true,
            };
            if lost {
                if monster.phase == MonsterState::Tell {
                    events.tells.push(Tell::canceled(monster));
                }
                monster.aggro = None;
                monster.strike = 0;
                if distance(monster.x, monster.y, home_x, home_y) > 20.0 {
                    monster.returning = true;
                    monster.phase = MonsterState::Chase;
                } else {
                    monster.phase = MonsterState::Idle;
                    return;
                }
            }
        }
        if monster.returning {
            let (to_x, to_y) = (home_x - monster.x, home_y - monster.y);
            if to_x.hypot(to_y) <= 20.0 {
                monster.x = home_x;
                monster.y = home_y;
                monster.returning = false;
                monster.hp = monster.max_hp;
                monster.phase = MonsterState::Idle;
            } else {
                walk(terrain, monster, to_x, to_y, t::RETURN_SPEED, dt);
            }
            return;
        }
    }

    // 대상: 캠프 몬스터는 공격한 사람, 아니면 같은 구역에서 가장 가까운 살아 있는 플레이어
    let mut target: Option<&MonsterPlayer> = None;
    let mut target_distance = 1e30f32;
    for player in players {
        if monster.aggro.is_some_and(|id| id != player.id) || player.zone != monster.zone {
            continue;
        }
        let d = distance(player.x, player.y, monster.x, monster.y);
        if player.alive && d < target_distance {
            target = Some(player);
            target_distance = d;
        }
    }
    let Some(target) = target else {
        if monster.phase == MonsterState::Tell {
            events.tells.push(Tell::canceled(monster));
        }
        monster.phase = MonsterState::Idle;
        monster.strike = 0;
        return;
    };
    if monster.phase == MonsterState::Idle {
        monster.phase = MonsterState::Chase;
    }
    let (to_x, to_y) = (target.x - monster.x, target.y - monster.y);

    match monster.phase {
        MonsterState::Chase => {
            let trigger = if monster.kind == 1 { t::CHARGE_TRIGGER } else { t::ARC_TRIGGER };
            if target_distance < trigger && terrain.line_of_sight(monster.x, monster.y, target.x, target.y) {
                begin_tell(monster, target, now_ms, events);
            } else {
                chase(terrain, monster, to_x, to_y, dt);
            }
        }
        MonsterState::Tell => {
            if now_ms < monster.deadline_ms {
                return;
            }
            // 예고가 끝난 정확한 시각(deadline_ms) 기준으로 다음 단계를 잡는다. 틱(50ms) 경계에 밀리지 않는다.
            if monster.kind == 1 {
                monster.phase = MonsterState::Dash;
                monster.deadline_ms = monster.deadline_ms.wrapping_add(t::DASH_MS);
                monster.dash_hits.clear();
            } else if monster.kind == 2 && monster.strike == 0 {
                monster.strike = 1;
                begin_tell(monster, target, monster.deadline_ms, events); // 지휘관: 첫 베기 직후 두 번째 예고
            } else {
                monster.phase = MonsterState::Recover;
                monster.deadline_ms = monster.deadline_ms.wrapping_add(t::ARC_RECOVER_MS[monster.kind as usize]);
                monster.strike = 0;
            }
        }
        MonsterState::Dash => {
            // 10cm씩 나눠 움직여 벽을 뚫지 않고, 지나가며 닿은 플레이어를 놓치지 않는다.
            let total = t::DASH_SPEED * dt;
            let steps = ((total / 10.0).ceil() as i32).max(1);
            for _ in 0..steps {
                let (x, y) = (monster.x + monster.dir_x * total / steps as f32, monster.y + monster.dir_y * total / steps as f32);
                if terrain.overlaps(x, y, t::RADIUS) {
                    monster.phase = MonsterState::Recover;
                    monster.deadline_ms = now_ms.wrapping_add(t::DASH_RECOVER_MS);
                    break;
                }
                monster.x = x;
                monster.y = y;
                for player in players {
                    if !player.alive || player.zone != monster.zone || monster.dash_hits.contains(&player.id) {
                        continue;
                    }
                    if distance(player.x, player.y, x, y) >= t::DASH_HIT_DISTANCE || !terrain.line_of_sight(x, y, player.x, player.y) {
                        continue;
                    }
                    monster.dash_hits.push(player.id);
                    events.dash_touches.push(DashTouch { monster_id: monster.id, player_id: player.id, x, y, power: monster.power });
                }
            }
            if monster.phase == MonsterState::Dash && now_ms >= monster.deadline_ms {
                monster.phase = MonsterState::Recover;
                monster.deadline_ms = now_ms.wrapping_add(t::DASH_RECOVER_MS);
            }
        }
        MonsterState::Recover => {
            if monster.kind != 2 && target_distance > 170.0 {
                chase(terrain, monster, to_x, to_y, dt);
            }
            if now_ms >= monster.deadline_ms {
                monster.phase = MonsterState::Chase;
            }
        }
        MonsterState::Idle | MonsterState::Stagger => {}
    }
}
