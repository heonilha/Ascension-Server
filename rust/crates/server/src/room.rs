//! 방 하나: PvE 층 인스턴스(팀 하나, 최대 3명) 또는 PvP 스테이지(최대 16명).
//!
//! 방의 상태는 방 태스크 하나만 가진다. 세션은 `RoomHandle`로 명령을 보내고, 방 태스크가 받은 순서대로 처리한다.
//! 명령 처리 사이사이에 20Hz 틱이 돈다. 서로 다른 방은 다른 태스크라서 여러 스레드에서 동시에 돌아간다.
//! 방으로 가는 `RoomHandle`이 모두 사라지면(마지막 사람이 나가고 `RoomManager`가 지우면) 방 태스크가 끝난다.

use crate::clock::server_time_ms;
use crate::log;
use crate::session::{Buffer, SessionHandle};
use asc_game::combat_rules::{allowed_rewind_ms, capacity_for, clamp_press_time, in_iframe, rules_for, COOLDOWN_SLACK_MS};
use asc_game::monsters::{tuning, MonsterEvents, MonsterField, MonsterPlayer};
use asc_game::room_layout::{spawn_position, ARENA_HALF_DEPTH, ARENA_HALF_WIDTH};
use asc_game::CollisionMap;
use asc_movement::{World, GROUND_Z, RADIUS, TICK_RATE};
use asc_protocol::*;
use bytemuck::Zeroable;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// 서버 틱 주기. 이동 묶음을 이 주기로 보내고, 판정 유예가 끝난 공격을 확정한다.
const TICK_INTERVAL: Duration = Duration::from_millis(SERVER_TICK_MS as u64); // 20Hz

// 이동 입력 처리량 제한(속도 조작 방지). 실제 시간 1초에 입력 60개(이동 틱 수)의 1.03배만큼 처리권이 쌓이고,
// 입력 하나마다 하나를 쓴다. 네트워크 몰림을 위해 0.5초 분량까지 모아 두고, 넘치는 입력은 버린다.
// 3% 여유: 0.5초 넘는 끊김으로 처리권이 바닥나도 다시 차오르게 한다(여유가 없으면 그 뒤 작은 흔들림마다 입력을 버린다).
// 대가로 속도 조작의 상한이 달리기 속도의 103%가 된다.
const MAX_INPUT_BACKLOG: f64 = TICK_RATE as f64 * 0.5;
const INPUT_RATE_TOLERANCE: f64 = 1.03;

// 전투 수치(P1 시험값)
const MAX_HP: f32 = 100.0;
/// 부활 지점에 몰린 몬스터에게 바로 다시 죽지 않게
const RESPAWN_PROTECTION_MS: u32 = 2000;
/// 판정 범위에 더하는 몸 반지름
const BODY_RADIUS: f32 = 40.0;
/// 내려찍기 예고
const SLAM_WINDUP_MS: u32 = 800;
const SLAM_RADIUS: f32 = 220.0;
const SLAM_DAMAGE: f32 = 25.0;
/// PvP 근접 공격 재사용
const SLASH_COOLDOWN_MS: u32 = 600;
/// 공격자 앞 이 거리에 판정 원의 중심
const SLASH_REACH: f32 = 120.0;
const SLASH_RADIUS: f32 = 130.0;
const SLASH_DAMAGE: f32 = 20.0;
/// 위치 이력 보관 시간
const HISTORY_MS: u32 = 1000;

// 플레이어 검기(PvE). 1인 데모의 기본값이다. 성장(무기·각성·레벨) 보정은 아직 서버에 없다.
/// 검기 사이 최소 간격(클라이언트 공격 동작 0.72초에 여유)
const WAVE_COOLDOWN_MS: u32 = 600;
/// 캐릭터 앞 이 거리에서 나간다
const WAVE_MUZZLE: f32 = 45.0;
const WAVE_DAMAGE: f32 = 20.0;
const WAVE_RADIUS: f32 = 55.0;
const WAVE_LIFE_MS: u32 = 1200;
const HEAVY_DAMAGE: f32 = 35.0;
const HEAVY_RADIUS: f32 = 70.0;
const HEAVY_RADIUS_PER_CHARGE: f32 = 160.0;
const HEAVY_PIERCES: i32 = 4;
const HEAVY_PIERCES_PER_CHARGE: f32 = 6.0;
const HEAVY_LIFE_PER_CHARGE_MS: f32 = 800.0;
/// 0.6초 / 최대 3초
const MIN_CHARGE: f32 = 0.2;
/// 완충에 필요한 시간 3초에 10% 여유
const FULL_CHARGE_MS: u32 = 2700;

/// 방의 명령 대기열 크기. 가득 차면 보내는 세션의 읽기가 멈춘다(입력을 쏟아붓는 클라이언트가 메모리를 늘리지 못한다).
const COMMAND_QUEUE: usize = 1024;

#[derive(Clone, Default)]
pub struct RoomOptions {
    /// PvE 방에서 이 주기(ms)로 서버 내려찍기 시험 패턴을 낸다. 0이면 끈다.
    /// 회피 판정을 정해진 박자로 시험하는 용도다(수문장 패턴을 서버로 옮기면 그 패턴으로 바꾼다).
    pub pve_slam_interval_ms: u32,
    /// PvE 방에 서버 몬스터를 둔다(증원·추적·공격). 끄면 빈 방이다(회피 판정 시험용).
    pub pve_monsters: bool,
    /// 충돌 지형. 있으면 이동(장애물 안·통과)과 공격(엄폐)을 지형으로도 검사한다. 모든 방이 같은 것을 읽기만 한다.
    pub map: Option<Arc<CollisionMap>>,
}

pub enum RoomCommand {
    Enter { session: SessionHandle, name: String },
    Leave { player_id: u32 },
    Input { player_id: u32, packet: CInput, commands: Vec<InputCommand> },
    Dodge { player_id: u32, packet: CDodge },
    Attack { player_id: u32, packet: CAttack },
    ReportRtt { player_id: u32, rtt_ms: u32 },
    Respawn { player_id: u32 },
}

/// 방에 명령을 보내는 손잡이.
#[derive(Clone)]
pub struct RoomHandle {
    commands: mpsc::Sender<RoomCommand>,
}

impl RoomHandle {
    pub async fn send(&self, command: RoomCommand) {
        let _ = self.commands.send(command).await;
    }
}

/// 방 태스크를 띄운다. tokio 런타임 안에서 불러야 한다.
pub fn spawn(mode: RoomMode, team_code: u32, options: RoomOptions) -> RoomHandle {
    let (commands, receiver) = mpsc::channel(COMMAND_QUEUE);
    tokio::spawn(Room::new(mode, team_code, options).run(receiver));
    RoomHandle { commands }
}

#[derive(Clone, Copy)]
struct DodgeRecord {
    /// 인정한 회피 시작 시각
    server_ms: u32,
    /// 도착 시각 - 인정 시각
    rewind_ms: u32,
}

struct PositionSample {
    server_ms: u32,
    position: Vec3,
}

struct Member {
    session: SessionHandle,
    player_id: u32,
    name: String,
    slot: usize,
    position: Vec3,
    velocity: Vec3,
    yaw: f32,
    /// `MoveState` 값. 연출용이라 검사하지 않고 그대로 전달한다
    state: u8,
    dirty: bool,

    // 서버 권한 이동
    sim: MoveSimState,
    /// 마지막으로 처리(또는 버림)한 입력 번호
    last_input_sequence: u32,
    has_input: bool,
    ack_dirty: bool,
    input_credits: f64,
    credit_time: Instant,
    dropped_inputs: u32,
    last_drop_log: Option<Instant>,

    hp: f32,
    /// HP 0. C_Respawn까지 몬스터가 노리지 않고 공격·회피를 받지 않는다
    dead: bool,
    /// 부활 직후 보호: 이 시각 전에 명중하는 공격은 맞지 않는다
    protected_until_ms: u32,
    /// 0 = 아직 모름
    rtt_ms: u32,
    /// 최근 회피 몇 개(판정 유예 동안 필요한 만큼)
    dodges: VecDeque<DodgeRecord>,
    last_attack_ms: u32,
    has_attacked: bool,
    history: VecDeque<PositionSample>,
}

struct ActiveAttack {
    attack_id: u32,
    attacker_id: u32,
    kind: AttackKind,
    hit_ms: u32,
    /// 엄폐 검사 시작점: 공격자 위치(근접) 또는 범위 중심(내려찍기)
    origin: Vec3,
    center: Vec3,
    radius: f32,
    damage: f32,
    yaw: f32,
    /// 0보다 크면 center에서 yaw 방향 부채꼴(몬스터 베기)
    half_angle: f32,
    /// 몬스터 공격이면 그 몬스터
    monster_id: u16,
    /// 0이 아니면 이 플레이어만 판정한다(돌진 접촉)
    only_target_id: u32,
    /// 판정을 끝낸 플레이어
    resolved: Vec<u32>,
}

impl ActiveAttack {
    fn new(kind: AttackKind, hit_ms: u32, origin: Vec3, center: Vec3, radius: f32, damage: f32) -> ActiveAttack {
        ActiveAttack {
            attack_id: 0,
            attacker_id: 0,
            kind,
            hit_ms,
            origin,
            center,
            radius,
            damage,
            yaw: 0.0,
            half_angle: 0.0,
            monster_id: 0,
            only_target_id: 0,
            resolved: Vec::new(),
        }
    }
}

struct Room {
    members: Vec<Member>,
    attacks: Vec<ActiveAttack>,
    mode: RoomMode,
    rules: ModeRules,
    team_code: u32,
    options: RoomOptions,
    tick: u32,
    next_attack_id: u32,
    next_slam_ms: u32,
    slam_target: usize,

    // 몬스터(PvE 방, 옵션이 켜져 있을 때만)
    monsters: Option<MonsterField>,
    monster_events: MonsterEvents,
    /// 몬스터 → 예고 중인 공격 id
    monster_attacks: HashMap<u16, u32>,
    /// 직전 틱에 몬스터 목록을 보냈는가(0마리가 되면 빈 목록을 한 번 보낸다)
    sent_monsters: bool,
    last_tick_time: Option<Instant>,
    // 틱 처리 시간 통계(10초마다 로그)
    tick_cost_sum_ms: f64,
    tick_cost_max_ms: f64,
    monster_sim_sum_ms: f64,
    monster_sim_max_ms: f64,
    tick_cost_samples: u32,
}

fn mode_name(mode: RoomMode) -> &'static str {
    match mode {
        RoomMode::PvP => "PvP",
        RoomMode::PvE => "PvE",
    }
}

/// 지형을 끈 실행에서 이동 계산에 넘기는 빈 지형.
fn empty_world() -> &'static World {
    static EMPTY: OnceLock<World> = OnceLock::new();
    EMPTY.get_or_init(World::empty)
}

fn buffer<T: Packet>(packet: &T) -> Buffer {
    encode(packet).into()
}

fn to_info(member: &Member) -> PlayerInfo {
    let mut info = PlayerInfo::zeroed();
    info.player_id = member.player_id;
    copy_name(&mut info.name, &member.name);
    info.position = member.position;
    info.yaw = member.yaw;
    info
}

/// `server_ms` 이전의 가장 마지막 표본. 이력이 비었으면 현재 위치.
fn position_at(member: &Member, server_ms: u32) -> Vec3 {
    let mut position = member.position;
    for sample in &member.history {
        if sample.server_ms > server_ms {
            break;
        }
        position = sample.position;
    }
    if let Some(first) = member.history.front() {
        if first.server_ms > server_ms {
            position = first.position;
        }
    }
    position
}

impl Room {
    fn new(mode: RoomMode, team_code: u32, options: RoomOptions) -> Room {
        Room {
            members: Vec::new(),
            attacks: Vec::new(),
            mode,
            rules: rules_for(mode),
            team_code,
            options,
            tick: 0,
            next_attack_id: 1,
            next_slam_ms: 0,
            slam_target: 0,
            monsters: None,
            monster_events: MonsterEvents::default(),
            monster_attacks: HashMap::new(),
            sent_monsters: false,
            last_tick_time: None,
            tick_cost_sum_ms: 0.0,
            tick_cost_max_ms: 0.0,
            monster_sim_sum_ms: 0.0,
            monster_sim_max_ms: 0.0,
            tick_cost_samples: 0,
        }
    }

    async fn run(mut self, mut commands: mpsc::Receiver<RoomCommand>) {
        let now_ms = server_time_ms();
        self.next_slam_ms = now_ms.wrapping_add(self.options.pve_slam_interval_ms);
        if self.mode == RoomMode::PvE && self.options.pve_monsters {
            self.monsters = Some(MonsterField::new(self.options.map.clone(), self.team_code.wrapping_mul(2654435761) ^ now_ms, now_ms));
        }
        let (iframe_ms, rewind_cap_ms) = (self.rules.iframe_ms, self.rules.rewind_cap_ms);
        log!(
            "방 {} 생성 ({}, 무적 {}ms, 되감기 상한 {}ms{})",
            self.team_code,
            mode_name(self.mode),
            iframe_ms,
            rewind_cap_ms,
            if self.monsters.is_some() { ", 몬스터 있음" } else { "" }
        );

        let mut next = Instant::now();
        loop {
            next += TICK_INTERVAL;
            // 멈춰 있다 깨어났을 때 밀린 틱을 몰아서 돌지 않는다.
            let now = Instant::now();
            if now.saturating_duration_since(next) > Duration::from_secs(1) {
                next = now;
            }
            let wake = tokio::time::sleep_until(next.into());
            tokio::pin!(wake);
            loop {
                tokio::select! {
                    command = commands.recv() => match command {
                        Some(command) => self.handle(command),
                        None => {
                            log!("방 {} 닫힘", self.team_code);
                            return;
                        }
                    },
                    _ = &mut wake => break,
                }
            }
            self.update();
        }
    }

    fn handle(&mut self, command: RoomCommand) {
        match command {
            RoomCommand::Enter { session, name } => self.enter(session, name),
            RoomCommand::Leave { player_id } => self.leave(player_id),
            RoomCommand::Input { player_id, packet, commands } => self.input(player_id, &packet, &commands),
            RoomCommand::Dodge { player_id, packet } => self.dodge(player_id, &packet),
            RoomCommand::Attack { player_id, packet } => self.attack(player_id, &packet),
            RoomCommand::ReportRtt { player_id, rtt_ms } => {
                if let Some(index) = self.find(player_id) {
                    self.members[index].rtt_ms = rtt_ms.min(5000);
                }
            }
            RoomCommand::Respawn { player_id } => self.respawn(player_id),
        }
    }

    fn find(&self, player_id: u32) -> Option<usize> {
        self.members.iter().position(|m| m.player_id == player_id)
    }

    fn free_slot(&self) -> Option<usize> {
        (0..capacity_for(self.mode) as usize).find(|slot| self.members.iter().all(|m| m.slot != *slot))
    }

    fn broadcast(&self, buffer: Buffer) {
        for member in &self.members {
            member.session.send(buffer.clone());
        }
    }

    fn enter(&mut self, session: SessionHandle, name: String) {
        if self.find(session.id()).is_some() {
            return;
        }
        let Some(slot) = self.free_slot() else {
            // RoomManager가 정원을 먼저 확인하므로 여기 오면 버그다.
            log!("방 {} 빈 자리 없음 (세션 {})", self.team_code, session.id());
            return;
        };

        let position = spawn_position(self.mode, slot);
        let mut sim = MoveSimState::zeroed();
        sim.x = position.x;
        sim.y = position.y;
        let now_ms = server_time_ms();
        let member = Member {
            player_id: session.id(),
            session,
            name,
            slot,
            position,
            velocity: Vec3::zeroed(),
            yaw: 0.0,
            state: MoveState::Idle.raw(),
            dirty: false,
            sim,
            last_input_sequence: 0,
            has_input: false,
            ack_dirty: false,
            input_credits: MAX_INPUT_BACKLOG,
            credit_time: Instant::now(),
            dropped_inputs: 0,
            last_drop_log: None,
            hp: MAX_HP,
            dead: false,
            protected_until_ms: 0,
            rtt_ms: 0,
            dodges: VecDeque::new(),
            last_attack_ms: 0,
            has_attacked: false,
            history: VecDeque::from([PositionSample { server_ms: now_ms, position }]),
        };

        // 새로 온 사람에게: 입장 결과 + 판정 규칙 + 이미 있던 사람들
        let others: Vec<PlayerInfo> = self.members.iter().map(to_info).collect();
        let mut reply = make::<SEnterRoom>();
        reply.result = EnterResult::Ok.raw();
        reply.team_code = self.team_code;
        reply.mode = self.mode.raw();
        reply.floor = 1;
        reply.spawn_position = position;
        reply.spawn_yaw = 0.0;
        reply.capacity = capacity_for(self.mode);
        reply.rules = self.rules;
        reply.server_time_ms = now_ms;
        member.session.send(encode_with_entries(reply, &others).into());

        // 이미 있던 사람들에게: 새로 온 사람
        let mut joined = make::<SPlayerJoined>();
        joined.player = to_info(&member);
        self.broadcast(buffer(&joined));

        log!("방 {} 입장: {} (슬롯 {}, 인원 {}/{})", self.team_code, member.name, slot, self.members.len() + 1, capacity_for(self.mode));
        self.members.push(member);
    }

    fn leave(&mut self, player_id: u32) {
        let Some(index) = self.find(player_id) else { return };
        let member = self.members.remove(index);
        log!("방 {} 퇴장: {}", self.team_code, member.name);

        let mut left = make::<SPlayerLeft>();
        left.player_id = player_id;
        self.broadcast(buffer(&left));
    }

    fn input(&mut self, player_id: u32, packet: &CInput, commands: &[InputCommand]) {
        let Some(index) = self.find(player_id) else { return };
        let member = &mut self.members[index];

        // 처리권: 실제 시간만큼 쌓인다. 입력을 더 빨리 보내도(속도 조작) 초당 약 62틱(60 × 1.03) 넘게 움직일 수 없다.
        let now = Instant::now();
        member.input_credits =
            (member.input_credits + (now - member.credit_time).as_secs_f64() * TICK_RATE as f64 * INPUT_RATE_TOLERANCE).min(MAX_INPUT_BACKLOG);
        member.credit_time = now;

        let world: &World = match &self.options.map {
            Some(map) => map.world(),
            None => empty_world(),
        };
        let mut dropped = 0;
        for (i, command) in commands.iter().enumerate() {
            let sequence = packet.first_sequence.wrapping_add(i as u32);
            if member.has_input && sequence <= member.last_input_sequence {
                continue; // 이미 처리한 번호
            }
            member.has_input = true;
            member.last_input_sequence = sequence;
            if member.input_credits < 1.0 {
                dropped += 1; // 버린 입력도 번호는 처리한 것으로 친다. 클라이언트는 확정 상태로 재조정된다
                continue;
            }
            member.input_credits -= 1.0;
            asc_movement::step(&mut member.sim, command, world);
        }
        if self.options.map.is_none() {
            // 지형을 끈 시험 실행: 맵 범위만 지킨다
            member.sim.x = { member.sim.x }.clamp(-ARENA_HALF_WIDTH + RADIUS, ARENA_HALF_WIDTH - RADIUS);
            member.sim.y = { member.sim.y }.clamp(-ARENA_HALF_DEPTH + RADIUS, ARENA_HALF_DEPTH - RADIUS);
        }

        if dropped > 0 {
            member.dropped_inputs += dropped;
            if member.last_drop_log.is_none_or(|last| now - last > Duration::from_secs(1)) {
                member.last_drop_log = Some(now);
                log!("방 {} 입력 초과로 버림: {} (누적 {}개)", self.team_code, member.name, member.dropped_inputs);
            }
        }

        member.position = Vec3 { x: member.sim.x, y: member.sim.y, z: GROUND_Z };
        member.velocity = Vec3 { x: member.sim.vx, y: member.sim.vy, z: 0.0 };
        if { packet.yaw }.is_finite() {
            member.yaw = packet.yaw;
        }
        member.state = packet.state;
        member.dirty = true;
        member.ack_dirty = true;

        // 판정 때 "명중 시각에 그 플레이어가 있던 곳"을 찾기 위해 도착 시각과 함께 남긴다.
        let now_ms = server_time_ms();
        member.history.push_back(PositionSample { server_ms: now_ms, position: member.position });
        while member.history.len() > 1 && now_ms.wrapping_sub(member.history[0].server_ms) > HISTORY_MS {
            member.history.pop_front();
        }
    }

    fn dodge(&mut self, player_id: u32, packet: &CDodge) {
        let Some(index) = self.find(player_id) else { return };
        let member = &mut self.members[index];
        if member.dead {
            return;
        }

        let arrival_ms = server_time_ms();
        let press_ms = packet.press_server_ms;
        let allowed = allowed_rewind_ms(&self.rules, member.rtt_ms);
        let accepted_ms = clamp_press_time(press_ms, arrival_ms, allowed);

        // 재사용 대기는 인정한 시각끼리 비교한다. 시각을 앞당겨도 직전 회피보다 앞설 수 없다.
        if let Some(last) = member.dodges.back() {
            if accepted_ms + COOLDOWN_SLACK_MS < last.server_ms + self.rules.dodge_cooldown_ms as u32 {
                log!("방 {} 회피 거부(재사용 대기): {} (직전 {}ms, 이번 {}ms)", self.team_code, member.name, last.server_ms, accepted_ms);
                return;
            }
        }

        member.dodges.push_back(DodgeRecord { server_ms: accepted_ms, rewind_ms: arrival_ms - accepted_ms });
        while member.dodges.len() > 4 {
            member.dodges.pop_front();
        }
        let requested = arrival_ms as i32 as i64 - press_ms as i32 as i64;
        log!("방 {} 회피: {} (요청 되감기 {}ms → 인정 {}ms, 허용 {}ms)", self.team_code, member.name, requested, arrival_ms - accepted_ms, allowed);
    }

    fn attack(&mut self, player_id: u32, packet: &CAttack) {
        let yaw = packet.yaw;
        if !yaw.is_finite() {
            return;
        }
        let Some(index) = self.find(player_id) else { return };
        if self.members[index].dead {
            return;
        }
        let now_ms = server_time_ms();
        let kind = AttackKind::from_raw(packet.kind);

        if self.mode == RoomMode::PvE {
            // PvE: 검기(투사체)로 몬스터를 친다.
            if self.monsters.is_some() && matches!(kind, Some(AttackKind::Wave | AttackKind::HeavyWave)) {
                self.wave(index, packet, kind == Some(AttackKind::HeavyWave), now_ms);
            }
            return;
        }
        if kind != Some(AttackKind::Slash) {
            return;
        }

        // PvP 근접 공격. 시작은 서버 도착 시각 기준이다. 공격 준비 시간이 공격자 쪽 지연을 흡수하므로 되감지 않는다.
        let member = &mut self.members[index];
        if member.has_attacked && now_ms.wrapping_sub(member.last_attack_ms) < SLASH_COOLDOWN_MS {
            return;
        }
        member.has_attacked = true;
        member.last_attack_ms = now_ms;

        let radians = yaw * std::f32::consts::PI / 180.0;
        let origin = member.position;
        let center = Vec3 { x: origin.x + SLASH_REACH * radians.cos(), y: origin.y + SLASH_REACH * radians.sin(), z: origin.z };
        let mut attack = ActiveAttack::new(AttackKind::Slash, now_ms + self.rules.attack_windup_ms as u32, origin, center, SLASH_RADIUS, SLASH_DAMAGE);
        attack.attacker_id = player_id;
        self.start_attack(attack, now_ms, true);
    }

    fn wave(&mut self, index: usize, packet: &CAttack, heavy: bool, now_ms: u32) {
        let member = &mut self.members[index];
        // 검기 사이 간격. 클라이언트 공격 동작(0.72초)보다 빨리 쏠 수 없다.
        let interval = if member.has_attacked { now_ms.wrapping_sub(member.last_attack_ms) } else { u32::MAX };
        if interval < WAVE_COOLDOWN_MS {
            return;
        }

        let (yaw, requested) = (packet.yaw, packet.charge);
        let mut charge = 0.0f32;
        if heavy {
            // 충전은 직전 공격이 끝난 뒤에야 시작할 수 있으므로, 지난 시간으로 모을 수 있었던 양까지만 인정한다.
            let possible = if interval == u32::MAX { 1.0 } else { ((interval - WAVE_COOLDOWN_MS) as f32 / FULL_CHARGE_MS as f32).min(1.0) };
            charge = (if requested.is_finite() { requested.clamp(0.0, 1.0) } else { 0.0 }).min(possible);
            if charge < MIN_CHARGE - 0.02 {
                log!("방 {} 충전 검기 거부: {} (요청 {:.2}, 가능 {:.2})", self.team_code, member.name, requested, possible);
                return;
            }
        }
        member.has_attacked = true;
        member.last_attack_ms = now_ms;

        let radians = yaw * std::f32::consts::PI / 180.0;
        let (x, y) = (member.position.x + WAVE_MUZZLE * radians.cos(), member.position.y + WAVE_MUZZLE * radians.sin());
        let damage = if heavy { HEAVY_DAMAGE * (1.0 + charge * 2.0) } else { WAVE_DAMAGE };
        let radius = if heavy { HEAVY_RADIUS + charge * HEAVY_RADIUS_PER_CHARGE } else { WAVE_RADIUS };
        let pierces = if heavy { HEAVY_PIERCES + (charge * HEAVY_PIERCES_PER_CHARGE).round() as i32 } else { 1 };
        let life_ms = if heavy { WAVE_LIFE_MS + (charge * HEAVY_LIFE_PER_CHARGE_MS) as u32 } else { WAVE_LIFE_MS };
        let owner_id = member.player_id;

        let Some(monsters) = &mut self.monsters else { return };
        let mut spawn = make::<SWaveSpawn>();
        spawn.wave_id = monsters.spawn_wave(owner_id, x, y, yaw, damage, radius, pierces, life_ms, now_ms);
        spawn.owner_id = owner_id;
        spawn.x = x;
        spawn.y = y;
        spawn.yaw = yaw;
        spawn.radius = radius;
        spawn.speed = tuning::WAVE_SPEED;
        spawn.life_ms = life_ms as u16;
        spawn.charge = charge;
        self.broadcast(buffer(&spawn));
    }

    fn respawn(&mut self, player_id: u32) {
        let Some(index) = self.find(player_id) else { return };
        let mode = self.mode;
        let member = &mut self.members[index];
        if !member.dead {
            return;
        }
        let now_ms = server_time_ms();
        member.dead = false;
        member.hp = MAX_HP;
        member.protected_until_ms = now_ms + RESPAWN_PROTECTION_MS;
        member.position = spawn_position(mode, member.slot);
        member.velocity = Vec3::zeroed();
        member.sim = MoveSimState::zeroed();
        member.sim.x = member.position.x;
        member.sim.y = member.position.y;
        member.dodges.clear();
        member.history.clear();
        member.history.push_back(PositionSample { server_ms: now_ms, position: member.position });
        // 본인은 S_MoveAck의 새 상태로 재조정된다
        member.dirty = true;
        member.ack_dirty = true;

        let mut packet = make::<SRespawn>();
        packet.player_id = player_id;
        packet.position = member.position;
        packet.hp = member.hp;
        log!("방 {} 부활: {}", self.team_code, member.name);
        self.broadcast(buffer(&packet));
    }

    /// 공격을 등록하고(`announce`면 S_AttackStart로 알리고) 공격 id를 돌려준다. `hit_ms`에 판정한다.
    fn start_attack(&mut self, mut attack: ActiveAttack, start_ms: u32, announce: bool) -> u32 {
        attack.attack_id = self.next_attack_id;
        self.next_attack_id = self.next_attack_id.wrapping_add(1);
        if announce {
            let mut start = make::<SAttackStart>();
            start.attack_id = attack.attack_id;
            start.attacker_id = attack.attacker_id;
            start.kind = attack.kind.raw();
            start.start_server_ms = start_ms;
            start.hit_server_ms = attack.hit_ms;
            start.center = attack.center;
            // 부채꼴은 중심 사이 거리 기준 사거리를 알린다(판정은 radius + 몸 반지름으로 한다).
            start.radius = if attack.half_angle > 0.0 { attack.radius + BODY_RADIUS } else { attack.radius };
            start.damage = attack.damage;
            start.monster_id = attack.monster_id;
            start.yaw = attack.yaw;
            start.half_angle = attack.half_angle;
            self.broadcast(buffer(&start));
        }
        let id = attack.attack_id;
        self.attacks.push(attack);
        id
    }

    // ---- 몬스터 ----

    fn update_monsters(&mut self, now_ms: u32, dt: f32) {
        let Some(monsters) = &mut self.monsters else { return };
        let players: Vec<MonsterPlayer> =
            self.members.iter().map(|m| MonsterPlayer { id: m.player_id, x: m.position.x, y: m.position.y, alive: !m.dead }).collect();

        let mut events = std::mem::take(&mut self.monster_events);
        events.clear();
        let sim_start = Instant::now();
        monsters.update(now_ms, dt, &players, &mut events);
        let sim_ms = sim_start.elapsed().as_secs_f64() * 1000.0;
        self.monster_sim_sum_ms += sim_ms;
        self.monster_sim_max_ms = self.monster_sim_max_ms.max(sim_ms);
        self.apply_monster_events(&events, now_ms);
        self.monster_events = events; // 할당한 버퍼를 다음 틱에 다시 쓴다

        // 살아 있는 몬스터 전부를 보낸다. 0마리가 되면 빈 목록을 한 번 보내 클라이언트가 지우게 한다.
        let Some(monsters) = &self.monsters else { return };
        let monsters = monsters.monsters();
        if monsters.is_empty() && !self.sent_monsters {
            return;
        }
        let entries: Vec<MonsterEntry> = monsters
            .iter()
            .map(|monster| MonsterEntry {
                id: monster.id,
                kind: monster.kind,
                state: monster.shown_state(now_ms).raw(),
                x: monster.x.round() as i16,
                y: monster.y.round() as i16,
                yaw: ((monster.yaw / 360.0 * 256.0).round() as i32 & 0xFF) as u8,
                hp: monster.hp.ceil() as u16,
            })
            .collect();
        let mut batch = make::<SMonsterBatch>();
        batch.server_tick = self.tick;
        self.sent_monsters = !entries.is_empty();
        self.broadcast(encode_with_entries(batch, &entries).into());
    }

    fn apply_monster_events(&mut self, events: &MonsterEvents, now_ms: u32) {
        // 예고 시작·취소는 일어난 순서대로 처리한다.
        for tell in &events.tells {
            if tell.cancel {
                let Some(attack_id) = self.monster_attacks.remove(&tell.monster_id) else { continue };
                // 명중 시각이 이미 지나 판정을 기다리는 공격은 그대로 둔다(이미 휘두른 공격이다).
                if self.attacks.iter().any(|a| a.attack_id == attack_id && a.hit_ms <= now_ms) {
                    continue;
                }
                self.attacks.retain(|a| a.attack_id != attack_id);
                let mut cancel = make::<SAttackCancel>();
                cancel.attack_id = attack_id;
                self.broadcast(buffer(&cancel));
                continue;
            }
            let at = Vec3 { x: tell.x, y: tell.y, z: GROUND_Z };
            if tell.kind == 1 {
                // 돌진: 예고만 알린다. 피해는 돌진 중 실제로 닿았을 때(DashTouch) 판정한다.
                let attack_id = self.next_attack_id;
                self.next_attack_id = self.next_attack_id.wrapping_add(1);
                let mut start = make::<SAttackStart>();
                start.attack_id = attack_id;
                start.kind = AttackKind::MonsterCharge.raw();
                start.start_server_ms = tell.start_ms;
                start.hit_server_ms = tell.hit_ms; // 출발 시각
                start.center = at;
                start.radius = tuning::DASH_SPEED * tuning::DASH_MS as f32 / 1000.0; // 돌진 거리
                start.damage = tuning::DASH_DAMAGE;
                start.monster_id = tell.monster_id;
                start.yaw = tell.yaw;
                self.broadcast(buffer(&start));
                self.monster_attacks.insert(tell.monster_id, attack_id);
            } else {
                let mut attack =
                    ActiveAttack::new(AttackKind::MonsterArc, tell.hit_ms, at, at, tuning::ARC_RANGE - BODY_RADIUS, tuning::ARC_DAMAGE[tell.kind as usize]);
                attack.yaw = tell.yaw;
                attack.half_angle = tuning::ARC_HALF_ANGLE;
                attack.monster_id = tell.monster_id;
                let attack_id = self.start_attack(attack, tell.start_ms, true);
                self.monster_attacks.insert(tell.monster_id, attack_id);
            }
        }
        // 돌진 접촉: 닿은 순간이 명중 시각이다. 그 플레이어만, 회피 무적·지연 보정을 거쳐 판정한다.
        for touch in &events.dash_touches {
            let at = Vec3 { x: touch.x, y: touch.y, z: GROUND_Z };
            let mut attack = ActiveAttack::new(AttackKind::MonsterCharge, now_ms, at, at, tuning::DASH_HIT_DISTANCE - BODY_RADIUS, tuning::DASH_DAMAGE);
            attack.monster_id = touch.monster_id;
            attack.only_target_id = touch.player_id;
            self.start_attack(attack, now_ms, false);
        }
        for hit in &events.hits {
            let mut packet = make::<SMonsterHit>();
            packet.monster_id = hit.monster_id;
            packet.attacker_id = hit.attacker_id;
            packet.damage = hit.damage;
            packet.hp_after = hit.hp_after.ceil() as u16;
            packet.x = hit.x;
            packet.y = hit.y;
            self.broadcast(buffer(&packet));
            if hit.hp_after <= 0.0 {
                self.monster_attacks.remove(&hit.monster_id);
            }
        }
        for end in &events.wave_ends {
            let mut packet = make::<SWaveEnd>();
            packet.wave_id = end.wave_id;
            packet.x = end.x;
            packet.y = end.y;
            self.broadcast(buffer(&packet));
        }
    }

    fn update_combat(&mut self, now_ms: u32) {
        // 서버 내려찍기 시험 패턴: 플레이어를 번갈아 노린다.
        if self.options.pve_slam_interval_ms > 0 && self.mode == RoomMode::PvE && !self.members.is_empty() && now_ms >= self.next_slam_ms {
            self.next_slam_ms = now_ms + self.options.pve_slam_interval_ms;
            for _ in 0..self.members.len() {
                let target = &self.members[self.slam_target % self.members.len()];
                self.slam_target = self.slam_target.wrapping_add(1);
                if target.dead {
                    continue;
                }
                let at = target.position;
                self.start_attack(ActiveAttack::new(AttackKind::Slam, now_ms + SLAM_WINDUP_MS, at, at, SLAM_RADIUS, SLAM_DAMAGE), now_ms, true);
                break;
            }
        }

        // 판정 유예: 플레이어마다 "명중 시각 + 허용 되감기"가 지나야 확정한다.
        // 그때까지 도착한 회피는 누른 시각이 명중 전이면 모두 반영된다.
        let mut attacks = std::mem::take(&mut self.attacks);
        for attack in &mut attacks {
            for index in 0..self.members.len() {
                let member = &self.members[index];
                let player_id = member.player_id;
                if player_id == attack.attacker_id || attack.resolved.contains(&player_id) {
                    continue;
                }
                if attack.only_target_id != 0 && attack.only_target_id != player_id {
                    continue;
                }
                let allowed = allowed_rewind_ms(&self.rules, member.rtt_ms);
                if now_ms < attack.hit_ms + allowed {
                    continue;
                }
                attack.resolved.push(player_id);
                if !member.dead {
                    self.resolve(attack, index, allowed);
                }
            }
        }
        // 허용 되감기는 상한을 넘지 않으므로 이 시점이면 모든 플레이어 판정이 끝났다.
        let expire_after = self.rules.rewind_cap_ms as u32 + 200;
        attacks.retain(|attack| now_ms <= attack.hit_ms + expire_after);
        self.attacks = attacks;
    }

    fn resolve(&mut self, attack: &ActiveAttack, index: usize, allowed_rewind_ms: u32) {
        let rules = self.rules;
        let team_code = self.team_code;
        let member = &mut self.members[index];
        // 위치 이력은 도착 시각으로 쌓이므로, 클라이언트가 명중 순간에 있던 위치는 한쪽 지연만큼 뒤에 도착한 표본이다.
        let one_way_ms = (member.rtt_ms / 2).min(allowed_rewind_ms);
        let position = position_at(member, attack.hit_ms + one_way_ms);
        let (dx, dy) = (position.x - attack.center.x, position.y - attack.center.y);
        let distance = dx.hypot(dy);
        if distance > attack.radius + BODY_RADIUS {
            return;
        }
        if attack.hit_ms < member.protected_until_ms {
            return; // 부활 직후 보호
        }
        if attack.half_angle > 0.0 && distance > 1.0 {
            // 부채꼴: 공격 방향과 대상 방향 사이 각이 반각 안이어야 한다.
            let radians = attack.yaw * std::f32::consts::PI / 180.0;
            let dot = (dx * radians.cos() + dy * radians.sin()) / distance;
            if dot < (attack.half_angle * std::f32::consts::PI / 180.0).cos() {
                return;
            }
        }
        // 엄폐: 공격 시작점과 대상 사이를 기둥·벽이 가리면 맞지 않는다(클라이언트 1인 모드의 시야 검사와 같은 원칙).
        if self.options.map.as_ref().is_some_and(|map| map.line_blocked(attack.origin, position)) {
            log!("방 {} 판정: 공격 {} → {} 엄폐로 빗나감", team_code, attack.attack_id, member.name);
            return;
        }

        let dodge = member.dodges.iter().find(|d| in_iframe(&rules, d.server_ms, attack.hit_ms)).copied();

        let mut result = make::<SHitResult>();
        result.attack_id = attack.attack_id;
        result.target_id = member.player_id;
        result.hit_server_ms = attack.hit_ms;
        if let Some(dodge) = dodge {
            result.result = HitResult::Dodged.raw();
            result.damage = 0.0;
            result.dodge_server_ms = dodge.server_ms;
            result.rewind_ms = dodge.rewind_ms as u16;
            log!(
                "방 {} 판정: 공격 {} → {} 회피 (명중 {}ms보다 {}ms 전 회피, 되감기 {}ms)",
                team_code,
                attack.attack_id,
                member.name,
                attack.hit_ms,
                attack.hit_ms - dodge.server_ms,
                dodge.rewind_ms
            );
        } else {
            result.result = HitResult::Hit.raw();
            result.damage = attack.damage;
            member.hp = (member.hp - attack.damage).max(0.0);
            log!("방 {} 판정: 공격 {} → {} 피격 {:.0} (HP {:.0})", team_code, attack.attack_id, member.name, attack.damage, member.hp);
        }
        result.hp_after = member.hp;

        // 사망: C_Respawn이 올 때까지 몬스터가 노리지 않고 공격·회피를 받지 않는다.
        if member.hp <= 0.0 {
            member.dead = true;
            log!("방 {} 사망: {}", team_code, member.name);
        }
        self.broadcast(buffer(&result));
    }

    /// 틱 하나: 몬스터·검기 → 공격 판정 → 이동 확정 상태·묶음 전송.
    fn update(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        // 실제로 지난 시간만큼 진행한다(타이머가 늦게 깨어나도 몬스터 속도가 변하지 않는다).
        let tick_start = Instant::now();
        let dt = self.last_tick_time.map_or(0.05, |last| (tick_start - last).as_secs_f32().clamp(0.001, 0.1));
        self.last_tick_time = Some(tick_start);
        let now_ms = server_time_ms();
        self.update_monsters(now_ms, dt); // 몬스터가 새 공격을 예고한 뒤
        self.update_combat(now_ms); // 명중 시각이 지난 공격을 판정한다

        // 입력을 보낸 본인에게 마지막 처리 번호와 확정 상태를 알린다(클라이언트 재조정 기준).
        for member in &mut self.members {
            if !member.ack_dirty {
                continue;
            }
            member.ack_dirty = false;
            let mut ack = make::<SMoveAck>();
            ack.sequence = member.last_input_sequence;
            ack.state = member.sim;
            member.session.send_packet(&ack);
        }

        let mut entries: Vec<MoveEntry> = Vec::new();
        for member in &mut self.members {
            if !member.dirty {
                continue;
            }
            member.dirty = false;
            entries.push(MoveEntry { player_id: member.player_id, position: member.position, velocity: member.velocity, yaw: member.yaw, state: member.state });
        }
        if !entries.is_empty() {
            // 모두에게 같은 버퍼를 보낸다. 클라이언트는 자기 playerId 항목을 무시한다.
            let mut batch = make::<SMoveBatch>();
            batch.server_tick = self.tick;
            self.broadcast(encode_with_entries(batch, &entries).into());
        }

        // 부하 기록: 몬스터가 있는 방의 틱 처리 시간을 10초마다 남긴다.
        let cost_ms = tick_start.elapsed().as_secs_f64() * 1000.0;
        self.tick_cost_sum_ms += cost_ms;
        self.tick_cost_max_ms = self.tick_cost_max_ms.max(cost_ms);
        self.tick_cost_samples += 1;
        if self.tick_cost_samples >= 200 {
            if let (Some(monsters), false) = (&self.monsters, self.members.is_empty()) {
                let samples = self.tick_cost_samples as f64;
                log!(
                    "방 {} 부하: 플레이어 {}명, 몬스터 {}마리(누적 생성 {}, 처치 {}), 틱 평균 {:.3}ms 최대 {:.3}ms (그중 몬스터 계산 평균 {:.3}ms 최대 {:.3}ms)",
                    self.team_code,
                    self.members.len(),
                    monsters.monsters().len(),
                    monsters.total_spawned(),
                    monsters.total_killed(),
                    self.tick_cost_sum_ms / samples,
                    self.tick_cost_max_ms,
                    self.monster_sim_sum_ms / samples,
                    self.monster_sim_max_ms
                );
            }
            self.tick_cost_sum_ms = 0.0;
            self.tick_cost_max_ms = 0.0;
            self.monster_sim_sum_ms = 0.0;
            self.monster_sim_max_ms = 0.0;
            self.tick_cost_samples = 0;
        }
    }
}
