//! 방 하나: 배틀로얄 매치(최대 4팀 × 3명) 또는 개발 시험용 연습 방(최대 3명).
//!
//! 매치: 몬스터 사냥 + 팀 간 전투. 같은 팀끼리는 공격이 맞지 않는다. 대기 → 카운트다운 → 1~4라운드(사이마다 자기장 축소)
//! → 최후의 1팀 → 결과 표시 뒤 다시 대기. 시간표·자기장·부활 대기는 `asc_game::match_rules`가 정한다.
//!
//! 매치 필드(`asc_game::field`): 캠프 몬스터(맞기 전에는 가만히 있다), 풀숲(밖의 적에게 숨는다, `visibility.rs`),
//! 2라운드의 균열(`rift.rs`). 스킬·스탯 포인트 쓰기는 `loadout.rs`.
//!
//! 방의 상태는 방 태스크 하나만 가진다. 세션은 `RoomHandle`로 명령을 보내고, 방 태스크가 받은 순서대로 처리한다.
//! 명령 처리 사이사이에 20Hz 틱이 돈다. 서로 다른 방은 다른 태스크라서 여러 스레드에서 동시에 돌아간다.
//! 방으로 가는 `RoomHandle`이 모두 사라지면(마지막 사람이 나가고 `RoomManager`가 지우면) 방 태스크가 끝난다.

use crate::clock::server_time_ms;
use crate::log;
use crate::session::{Buffer, SessionHandle};
mod loadout;
mod rift;
mod visibility;

use asc_game::combat_rules::{allowed_rewind_ms, capacity_for, clamp_press_time, in_iframe, rules_for, COOLDOWN_SLACK_MS};
use asc_game::field::FieldLayout;
use asc_game::loadout::Loadout;
use asc_game::match_rules::{self, running_phase, MatchSchedule, RunningPhase, ZONE_CENTER, ZONE_RADIUS};
use asc_game::monsters::{tuning, MonsterEvents, MonsterField, MonsterPlayer};
use asc_game::progression::{self, Progress, MONSTER_XP};
use asc_game::rng::Rng;
use asc_game::room_layout::{practice_spawn_position, team_spawn_position, ARENA_HALF_DEPTH, ARENA_HALF_WIDTH};
use asc_game::CollisionMap;
use asc_movement::{World, GROUND_Z, RADIUS, TICK_RATE};
use asc_protocol::*;
use bytemuck::Zeroable;
use std::collections::{HashMap, HashSet, VecDeque};
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

/// 매치 상태(S_MatchState)를 변화가 없어도 다시 보내는 주기
const MATCH_STATE_INTERVAL_MS: u32 = 2000;
/// 자기장 피해 주기(1초마다 초당 피해를 한 번에 준다)
const ZONE_DAMAGE_INTERVAL_MS: u32 = 1000;
/// 부활 위치 후보 수
const RESPAWN_CANDIDATES: usize = 24;

/// 방의 명령 대기열 크기. 가득 차면 보내는 세션의 읽기가 멈춘다(입력을 쏟아붓는 클라이언트가 메모리를 늘리지 못한다).
const COMMAND_QUEUE: usize = 1024;

#[derive(Clone, Default)]
pub struct RoomOptions {
    /// 연습 방에서 이 주기(ms)로 서버 내려찍기 시험 패턴을 낸다. 0이면 끈다(회피 판정을 정해진 박자로 시험하는 용도).
    pub practice_slam_interval_ms: u32,
    /// 연습 방에 서버 몬스터를 둔다(증원·추적·공격). 끄면 빈 방이다(회피 판정 시험용).
    pub practice_monsters: bool,
    /// 매치에 서버 몬스터를 둔다. 끄면 플레이어끼리만 싸운다(회피 판정 시험용).
    pub match_monsters: bool,
    /// 매치 시간표(라운드·축소·부활 대기). 기본은 기획안의 36분. 데모·시험은 `MatchSchedule::scaled`로 줄인다.
    pub schedule: MatchSchedule,
    /// 처치 경험치 배율(1 = 기본). 시간을 줄인 데모에서 레벨이 같은 속도로 오르게 키운다. 0이면 1로 본다.
    pub xp_scale: f32,
    /// 충돌 지형. 있으면 이동(장애물 안·통과)과 공격(엄폐)을 지형으로도 검사한다. 모든 방이 같은 것을 읽기만 한다.
    pub map: Option<Arc<CollisionMap>>,
}

pub enum RoomCommand {
    /// `team`: 매치 팀 번호 1~4(세션이 확인했다). 연습 방은 0
    Enter {
        session: SessionHandle,
        name: String,
        team: u8,
    },
    Leave {
        player_id: u32,
    },
    Input {
        player_id: u32,
        packet: CInput,
        commands: Vec<InputCommand>,
    },
    Dodge {
        player_id: u32,
        packet: CDodge,
    },
    Attack {
        player_id: u32,
        packet: CAttack,
    },
    ReportRtt {
        player_id: u32,
        rtt_ms: u32,
    },
    Respawn {
        player_id: u32,
    },
    Skill {
        player_id: u32,
        packet: CSkillCommand,
    },
    Stat {
        player_id: u32,
        packet: CStatCommand,
    },
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
    /// 매치 팀 번호 1~4. 연습 방은 0
    team: u8,
    /// 팀 안에서 몇 번째 자리(0~2). 팀 시작 위치를 정한다
    team_slot: usize,
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
    /// 매치: 이 시각에 서버가 부활시킨다
    respawn_at_ms: Option<u32>,
    /// 매치: 4라운드에 죽어 탈락했다(이 판에서는 부활하지 않는다)
    eliminated: bool,
    /// 레벨·경험치·포인트(한 판 동안 유지, 죽어도 잃지 않는다)
    progress: Progress,
    /// 0 = 아직 모름
    rtt_ms: u32,
    /// 최근 회피 몇 개(판정 유예 동안 필요한 만큼)
    dodges: VecDeque<DodgeRecord>,
    /// 마지막 검기(재사용 대기·충전량 상한 기준)
    last_attack_ms: u32,
    has_attacked: bool,
    /// 마지막 근접 공격. 검기와 따로 센다: 클라이언트는 공격 한 번에 근접 공격과 0.28초 뒤 검기를 함께 보낸다
    last_slash_ms: u32,
    has_slashed: bool,
    history: VecDeque<PositionSample>,

    /// 배운 스킬·분배한 스탯(한 판 동안 유지)
    loadout: Loadout,
    /// 마지막으로 피해를 주고받은 시각(스킬 지우기는 전투 중 불가)
    last_combat_ms: Option<u32>,
    /// 들어가 있는 풀숲
    bush: Option<usize>,
    /// 공격하면 이 시각까지 풀숲 안에서도 보인다
    revealed_until_ms: u32,
    /// 균열 안: 들어온 입구 번호
    rift_gate: Option<usize>,
    /// 이 판에서 균열에 들어갔었다(다시 못 들어간다)
    rift_used: bool,
    /// 균열 입구에 서 있기 시작한 (입구, 시각)
    gate_hold: Option<(usize, u32)>,
}

struct ActiveAttack {
    attack_id: u32,
    attacker_id: u32,
    /// 공격한 플레이어의 팀(매치). 같은 팀은 맞지 않는다. 0 = 몬스터·서버 공격
    attacker_team: u8,
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
            attacker_team: 0,
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

/// 매치 진행 단계(방 내부 상태). 진행 중 세부 단계(라운드·축소)는 시작 시각으로 계산한다.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Waiting,
    Countdown {
        start_ms: u32,
        end_ms: u32,
    },
    /// `start_ms` = 1라운드 시작
    Running {
        start_ms: u32,
    },
    Ended {
        start_ms: u32,
        end_ms: u32,
        winner: u8,
        round: u8,
    },
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

    // 매치
    stage: Stage,
    /// 1라운드를 시작할 때 있던 팀 수. 2팀 이상으로 시작했으면 1팀이 남을 때 끝난다
    started_teams: usize,
    /// 가장 마지막에 죽은 사람의 팀(동시 전멸 판정)
    last_death_team: Option<u8>,
    /// 마지막으로 보낸 (단계, 라운드, 남은 팀 수). 바뀌면 바로 다시 보낸다
    sent_match_key: Option<(u8, u8, u8)>,
    next_match_state_ms: u32,
    next_zone_damage_ms: u32,
    rng: Rng,
    /// 필드 배치(캠프·풀숲·균열). 지형 파일의 표식에서 읽는다
    layout: FieldLayout,
    rift: rift::Rift,
    /// (받는 사람, 숨은 사람): 지금 숨김 상태로 알린 쌍
    hidden_pairs: HashSet<(u32, u32)>,
    // 틱 처리 시간 통계(10초마다 로그)
    tick_cost_sum_ms: f64,
    tick_cost_max_ms: f64,
    monster_sim_sum_ms: f64,
    monster_sim_max_ms: f64,
    tick_cost_samples: u32,
}

fn mode_name(mode: RoomMode) -> &'static str {
    match mode {
        RoomMode::Match => "매치",
        RoomMode::Practice => "연습",
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

fn progress_packet(member: &Member, gained_xp: u32, reason: ProgressReason, levels_gained: u8) -> SPlayerProgress {
    let p = member.progress;
    let mut packet = make::<SPlayerProgress>();
    packet.player_id = member.player_id;
    packet.level = p.level;
    packet.xp = p.xp;
    packet.xp_to_next = progression::xp_to_next(p.level);
    packet.skill_points = p.skill_points;
    packet.awakening_points = p.awakening_points;
    packet.stat_points = p.stat_points;
    packet.gained_xp = gained_xp;
    packet.reason = reason.raw();
    packet.levels_gained = levels_gained;
    packet
}

fn to_info(member: &Member) -> PlayerInfo {
    let mut info = PlayerInfo::zeroed();
    info.player_id = member.player_id;
    copy_name(&mut info.name, &member.name);
    info.position = member.position;
    info.yaw = member.yaw;
    info.team = member.team;
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
        let layout = if mode == RoomMode::Match { FieldLayout::from_map(options.map.as_deref()) } else { FieldLayout::default() };
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
            stage: Stage::Waiting,
            started_teams: 0,
            last_death_team: None,
            sent_match_key: None,
            next_match_state_ms: 0,
            next_zone_damage_ms: 0,
            rng: Rng::new(team_code.wrapping_mul(2246822519) ^ 0x5bd1_e995),
            layout,
            rift: rift::Rift::default(),
            hidden_pairs: HashSet::new(),
            tick_cost_sum_ms: 0.0,
            tick_cost_max_ms: 0.0,
            monster_sim_sum_ms: 0.0,
            monster_sim_max_ms: 0.0,
            tick_cost_samples: 0,
        }
    }

    async fn run(mut self, mut commands: mpsc::Receiver<RoomCommand>) {
        let now_ms = server_time_ms();
        self.next_slam_ms = now_ms.wrapping_add(self.options.practice_slam_interval_ms);
        let monsters = match self.mode {
            RoomMode::Practice => self.options.practice_monsters,
            RoomMode::Match => self.options.match_monsters,
        };
        if monsters {
            let mut field = MonsterField::new(self.options.map.clone(), self.team_code.wrapping_mul(2654435761) ^ now_ms, now_ms);
            if self.mode == RoomMode::Match {
                // 매치 필드: 증원 대신 캠프. 캠프 몬스터는 공격받기 전에는 쫓지 않는다
                field.set_spawning(false);
                field.set_camps(&self.layout.camps, now_ms);
            }
            self.monsters = Some(field);
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
            RoomCommand::Enter { session, name, team } => self.enter(session, name, team),
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
            RoomCommand::Skill { player_id, packet } => self.skill_command(player_id, &packet),
            RoomCommand::Stat { player_id, packet } => self.stat_command(player_id, &packet),
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

    fn enter(&mut self, session: SessionHandle, name: String, team: u8) {
        if self.find(session.id()).is_some() {
            return;
        }
        let Some(slot) = self.free_slot() else {
            // RoomManager가 정원을 먼저 확인하므로 여기 오면 버그다.
            log!("방 {} 빈 자리 없음 (세션 {})", self.team_code, session.id());
            return;
        };

        let team = if self.mode == RoomMode::Match { team } else { 0 };
        // 팀 안의 빈 자리(0~2). RoomManager가 팀 정원을 먼저 확인했다.
        let team_slot = (0..3).find(|s| self.members.iter().all(|m| m.team != team || m.team_slot != *s)).unwrap_or(0);
        let position = match self.mode {
            RoomMode::Match => team_spawn_position(team, team_slot),
            RoomMode::Practice => practice_spawn_position(slot),
        };
        let mut sim = MoveSimState::zeroed();
        sim.x = position.x;
        sim.y = position.y;
        let now_ms = server_time_ms();
        let member = Member {
            player_id: session.id(),
            session,
            name,
            slot,
            team,
            team_slot,
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
            respawn_at_ms: None,
            eliminated: false,
            progress: Progress::default(),
            rtt_ms: 0,
            dodges: VecDeque::new(),
            last_attack_ms: 0,
            has_attacked: false,
            last_slash_ms: 0,
            has_slashed: false,
            history: VecDeque::from([PositionSample { server_ms: now_ms, position }]),
            loadout: Loadout::default(),
            last_combat_ms: None,
            bush: None,
            revealed_until_ms: 0,
            rift_gate: None,
            rift_used: false,
            gate_hold: None,
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

        log!("방 {} 입장: {} (팀 {}, 슬롯 {}, 인원 {}/{})", self.team_code, member.name, team, slot, self.members.len() + 1, capacity_for(self.mode));
        let session = member.session.clone();
        self.members.push(member);
        if self.mode == RoomMode::Match {
            session.send(buffer(&self.match_state(now_ms)));
        }
        // 새로 온 사람에게 방 안 모두의 레벨, 다른 사람들에게 새로 온 사람의 레벨
        for other in &self.members {
            if other.player_id != session.id() {
                session.send(buffer(&progress_packet(other, 0, ProgressReason::None, 0)));
            }
        }
        let newcomer = self.members.last().expect("방금 넣었다");
        self.broadcast(buffer(&progress_packet(newcomer, 0, ProgressReason::None, 0)));
        self.send_loadout(self.members.len() - 1, LoadoutResult::Ok, 0, 0);
        if self.mode == RoomMode::Match {
            session.send(buffer(&self.rift_state(now_ms)));
        }
    }

    /// 경험치를 주고 모두에게 알린다(배율 적용, 최소 1).
    fn award_xp(&mut self, player_id: u32, base: u32, reason: ProgressReason) {
        let Some(index) = self.find(player_id) else { return };
        let scale = if self.options.xp_scale > 0.0 { self.options.xp_scale } else { 1.0 };
        let amount = ((base as f32 * scale).round() as u32).max(1);
        let team_code = self.team_code;
        let member = &mut self.members[index];
        let levels = member.progress.gain(amount);
        if levels > 0 {
            log!("방 {} 레벨업: {} → {}레벨", team_code, member.name, member.progress.level);
        }
        let packet = progress_packet(member, amount, reason, levels);
        self.broadcast(buffer(&packet));
        if levels > 0 {
            self.send_loadout(index, LoadoutResult::Ok, 0, 0); // 남은 포인트가 늘었다
        }
    }

    /// 한 판을 새로 시작할 때 모두 1레벨로
    fn reset_progress(&mut self) {
        for member in &mut self.members {
            member.progress = Progress::default();
            member.loadout = Loadout::default();
            member.last_combat_ms = None;
        }
        for member in &self.members {
            self.broadcast(buffer(&progress_packet(member, 0, ProgressReason::None, 0)));
        }
        for index in 0..self.members.len() {
            self.send_loadout(index, LoadoutResult::Ok, 0, 0);
        }
    }

    fn leave(&mut self, player_id: u32) {
        let Some(index) = self.find(player_id) else { return };
        let member = self.members.remove(index);
        log!("방 {} 퇴장: {}", self.team_code, member.name);

        self.hidden_pairs.retain(|&(viewer, target)| viewer != player_id && target != player_id);
        let mut left = make::<SPlayerLeft>();
        left.player_id = player_id;
        self.broadcast(buffer(&left));
        if self.mode == RoomMode::Match {
            self.check_winner(server_time_ms());
        }
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

        // 검기(투사체)는 몬스터를 친다(두 모드 공통).
        if matches!(kind, Some(AttackKind::Wave | AttackKind::HeavyWave)) {
            if self.monsters.is_some() {
                self.wave(index, packet, kind == Some(AttackKind::HeavyWave), now_ms);
            }
            return;
        }
        // 근접 공격은 매치에서 다른 팀 플레이어를 친다.
        if self.mode != RoomMode::Match || kind != Some(AttackKind::Slash) {
            return;
        }

        // 매치 근접 공격. 시작은 서버 도착 시각 기준이다. 공격 준비 시간이 공격자 쪽 지연을 흡수하므로 되감지 않는다.
        let member = &mut self.members[index];
        if member.has_slashed && now_ms.wrapping_sub(member.last_slash_ms) < SLASH_COOLDOWN_MS {
            return;
        }
        member.has_slashed = true;
        member.last_slash_ms = now_ms;
        member.revealed_until_ms = now_ms + visibility::ATTACK_REVEAL_MS;

        let radians = yaw * std::f32::consts::PI / 180.0;
        let origin = member.position;
        let center = Vec3 { x: origin.x + SLASH_REACH * radians.cos(), y: origin.y + SLASH_REACH * radians.sin(), z: origin.z };
        let mut attack = ActiveAttack::new(AttackKind::Slash, now_ms + self.rules.attack_windup_ms as u32, origin, center, SLASH_RADIUS, SLASH_DAMAGE);
        attack.attacker_id = player_id;
        attack.attacker_team = member.team;
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
        member.revealed_until_ms = now_ms + visibility::ATTACK_REVEAL_MS;

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

    /// C_Respawn: 연습 방만 받는다. 매치는 서버가 부활 대기 뒤에 스스로 부활시킨다.
    fn respawn(&mut self, player_id: u32) {
        let Some(index) = self.find(player_id) else { return };
        if self.mode != RoomMode::Practice || !self.members[index].dead {
            return;
        }
        let position = practice_spawn_position(self.members[index].slot);
        self.revive(index, position, server_time_ms());
    }

    /// 살려서 `position`으로 옮기고 HP를 채운 뒤 모두에게 알린다.
    fn revive(&mut self, index: usize, position: Vec3, now_ms: u32) {
        let player_id = self.members[index].player_id;
        let member = &mut self.members[index];
        member.dead = false;
        member.eliminated = false;
        member.respawn_at_ms = None;
        member.rift_gate = None;
        member.gate_hold = None;
        member.hp = MAX_HP;
        member.protected_until_ms = now_ms + RESPAWN_PROTECTION_MS;
        member.position = position;
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

    /// 위치만 옮긴다(HP 그대로, 균열 입장·퇴장). 본인은 S_MoveAck의 새 상태로 재조정된다.
    fn teleport(&mut self, index: usize, position: Vec3, yaw: f32, reason: TeleportReason, now_ms: u32) {
        let member = &mut self.members[index];
        member.position = position;
        member.velocity = Vec3::zeroed();
        member.yaw = yaw;
        member.sim = MoveSimState::zeroed();
        member.sim.x = position.x;
        member.sim.y = position.y;
        member.dodges.clear();
        member.history.clear();
        member.history.push_back(PositionSample { server_ms: now_ms, position });
        member.gate_hold = None;
        member.dirty = true;
        member.ack_dirty = true;
        let mut packet = make::<STeleport>();
        packet.player_id = member.player_id;
        packet.position = position;
        packet.yaw = yaw;
        packet.reason = reason.raw();
        let (x, y) = (position.x, position.y);
        log!("방 {} 순간 이동({:?}): {} → ({:.0}, {:.0})", self.team_code, reason, member.name, x, y);
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

    // ---- 매치 진행 ----

    /// 진행 중이면 지금의 세부 단계(라운드·축소)와 1라운드 시작 뒤 지난 시간
    fn running(&self, now_ms: u32) -> Option<(RunningPhase, u32)> {
        match self.stage {
            Stage::Running { start_ms } => {
                let elapsed = now_ms.wrapping_sub(start_ms);
                Some((running_phase(&self.options.schedule, elapsed), elapsed))
            }
            _ => None,
        }
    }

    /// 지금 자기장 반지름. 시작 전·결과 표시 중에는 1라운드 크기(맵 전체)
    fn zone_radius(&self, now_ms: u32) -> f32 {
        self.running(now_ms).map_or(ZONE_RADIUS[0], |(phase, elapsed)| phase.radius_at(elapsed))
    }

    /// 사람이 한 명이라도 있는 팀 수
    fn present_teams(&self) -> usize {
        let mut teams: Vec<u8> = self.members.iter().map(|m| m.team).collect();
        teams.sort_unstable();
        teams.dedup();
        teams.len()
    }

    /// 탈락하지 않은 사람이 있는 팀(오름차순, 중복 없음). 부활 대기 중인 사람도 살아 있는 팀으로 친다.
    fn alive_teams(&self) -> Vec<u8> {
        let mut teams: Vec<u8> = self.members.iter().filter(|m| !m.eliminated).map(|m| m.team).collect();
        teams.sort_unstable();
        teams.dedup();
        teams
    }

    fn match_state(&self, now_ms: u32) -> SMatchState {
        let mut state = make::<SMatchState>();
        state.zone_x = ZONE_CENTER.0;
        state.zone_y = ZONE_CENTER.1;
        state.zone_radius_from = ZONE_RADIUS[0];
        state.zone_radius_to = ZONE_RADIUS[0];
        state.teams_alive = self.alive_teams().len() as u8;
        state.respawn_allowed = 1;
        match self.stage {
            Stage::Waiting => state.phase = MatchPhase::Waiting.raw(),
            Stage::Countdown { start_ms, end_ms } => {
                state.phase = MatchPhase::Countdown.raw();
                state.phase_start_server_ms = start_ms;
                state.phase_end_server_ms = end_ms;
            }
            Stage::Running { start_ms } => {
                let phase = running_phase(&self.options.schedule, now_ms.wrapping_sub(start_ms));
                state.phase = phase.phase.raw();
                state.round = phase.round;
                state.phase_start_server_ms = start_ms + phase.start_offset_ms;
                state.phase_end_server_ms = start_ms + phase.end_offset_ms;
                state.zone_radius_from = phase.radius_from;
                state.zone_radius_to = phase.radius_to;
                state.zone_damage_percent = phase.damage_percent;
                state.respawn_allowed = phase.respawn_wait_ms.is_some() as u8;
            }
            Stage::Ended { start_ms, end_ms, winner, round } => {
                state.phase = MatchPhase::Ended.raw();
                state.round = round;
                state.phase_start_server_ms = start_ms;
                state.phase_end_server_ms = end_ms;
                state.winner_team = winner;
            }
        }
        state
    }

    fn broadcast_match_state(&mut self, now_ms: u32) {
        let state = self.match_state(now_ms);
        self.sent_match_key = Some((state.phase, state.round, state.teams_alive));
        self.next_match_state_ms = now_ms + MATCH_STATE_INTERVAL_MS;
        self.broadcast(buffer(&state));
    }

    /// 틱마다: 단계 진행 → 자기장 피해 → 부활 → 상태 알림.
    fn update_match(&mut self, now_ms: u32) {
        let schedule = self.options.schedule;
        match self.stage {
            Stage::Waiting => {
                if self.present_teams() >= schedule.start_teams as usize {
                    self.stage = Stage::Countdown { start_ms: now_ms, end_ms: now_ms + schedule.countdown_ms };
                    log!("방 {} 매치 카운트다운 ({}팀)", self.team_code, self.present_teams());
                }
            }
            Stage::Countdown { end_ms, .. } => {
                if self.present_teams() < schedule.start_teams as usize {
                    self.stage = Stage::Waiting;
                    log!("방 {} 매치 대기로 돌아감(팀 부족)", self.team_code);
                } else if now_ms >= end_ms {
                    self.start_round_one(now_ms);
                }
            }
            Stage::Running { .. } => {
                if now_ms >= self.next_zone_damage_ms {
                    self.next_zone_damage_ms = now_ms + ZONE_DAMAGE_INTERVAL_MS;
                    self.apply_zone_damage(now_ms);
                    self.apply_rift_zone_damage(now_ms);
                }
                self.update_rift(now_ms);
            }
            Stage::Ended { end_ms, .. } => {
                if now_ms >= end_ms {
                    self.reset_match(now_ms);
                }
            }
        }

        // 부활 대기가 끝난 사람
        for index in 0..self.members.len() {
            let member = &self.members[index];
            if member.dead && !member.eliminated && member.respawn_at_ms.is_some_and(|at| now_ms >= at) {
                let position = self.safe_respawn_position(member.team, now_ms);
                self.revive(index, position, now_ms);
            }
        }

        let key = {
            let state = self.match_state(now_ms);
            (state.phase, state.round, state.teams_alive)
        };
        if self.sent_match_key != Some(key) || now_ms >= self.next_match_state_ms {
            self.broadcast_match_state(now_ms);
        }
        self.maybe_broadcast_rift_state(now_ms);
    }

    /// 1라운드 시작: 모두 살려 팀 시작 위치로 옮긴다.
    fn start_round_one(&mut self, now_ms: u32) {
        self.stage = Stage::Running { start_ms: now_ms };
        self.started_teams = self.present_teams();
        self.last_death_team = None;
        self.next_zone_damage_ms = now_ms + ZONE_DAMAGE_INTERVAL_MS;
        log!("방 {} 매치 시작: {}팀 {}명, 총 {}초", self.team_code, self.started_teams, self.members.len(), self.options.schedule.total_ms() / 1000);
        self.reset_rift(now_ms);
        self.revive_everyone_at_bases(now_ms);
        self.reset_progress(); // 시작 전에 모은 경험치는 이어지지 않는다
        if let Some(monsters) = &mut self.monsters {
            monsters.set_camps(&self.layout.camps, now_ms); // 새 판: 캠프를 모두 새로 채운다
        }
    }

    /// 결과 표시가 끝났다: 다음 판을 위해 대기로 돌아간다.
    fn reset_match(&mut self, now_ms: u32) {
        self.stage = Stage::Waiting;
        self.started_teams = 0;
        log!("방 {} 매치 초기화 → 대기", self.team_code);
        self.reset_rift(now_ms);
        self.revive_everyone_at_bases(now_ms);
        self.reset_progress();
    }

    fn revive_everyone_at_bases(&mut self, now_ms: u32) {
        for index in 0..self.members.len() {
            let position = team_spawn_position(self.members[index].team, self.members[index].team_slot);
            self.revive(index, position, now_ms);
        }
    }

    /// 자기장 밖 플레이어에게 1초 분량 피해(초당 최대 체력의 n%).
    /// 같은 피해 틱에 죽은 사람은 동시에 죽은 것으로 본다: 여러 팀이 같이 죽어 아무도 안 남으면 무승부(승리 팀 0).
    fn apply_zone_damage(&mut self, now_ms: u32) {
        let Some((phase, elapsed)) = self.running(now_ms) else { return };
        let radius = phase.radius_at(elapsed);
        let damage = MAX_HP * phase.damage_percent / 100.0;
        if damage <= 0.0 {
            return;
        }
        let mut died_teams: Vec<u8> = Vec::new();
        for index in 0..self.members.len() {
            let member = &mut self.members[index];
            if member.dead || member.rift_gate.is_some() || !match_rules::outside_zone(member.position.x, member.position.y, radius) {
                continue; // 균열 안은 전투장 자기장(rift.rs)만 받는다
            }
            member.hp = (member.hp - damage).max(0.0);
            let mut result = make::<SHitResult>();
            result.attack_id = 0; // 자기장
            result.target_id = member.player_id;
            result.result = HitResult::Hit.raw();
            result.damage = damage;
            result.hp_after = member.hp;
            result.hit_server_ms = now_ms;
            let died = member.hp <= 0.0;
            if died {
                member.dead = true;
                log!("방 {} 자기장 사망: {}", self.team_code, member.name);
            }
            self.broadcast(buffer(&result));
            if died {
                died_teams.push(self.members[index].team);
                self.on_death(index, DeathCause::Zone, 0, now_ms, false);
            }
        }
        if !died_teams.is_empty() {
            died_teams.dedup();
            if died_teams.iter().any(|&t| t != died_teams[0]) {
                self.last_death_team = None; // 여러 팀이 동시에
            }
            self.check_winner(now_ms);
        }
    }

    /// 사망 처리(HP 0이 된 직후). 매치: 부활 시각을 정하거나 탈락시키고 알린 뒤 승패를 본다. 연습 방은 C_Respawn을 기다린다.
    /// `check`가 거짓이면 승패 확인을 부른 쪽이 한다(같은 틱에 여러 명이 죽을 때).
    fn on_death(&mut self, index: usize, cause: DeathCause, killer_id: u32, now_ms: u32, check: bool) {
        if self.mode != RoomMode::Match {
            return;
        }
        let in_rift = self.members[index].rift_gate.is_some();
        let wait = match self.stage {
            Stage::Waiting | Stage::Countdown { .. } => Some(self.options.schedule.pregame_respawn_ms),
            // 균열 안에서 죽으면 탈락 없이 밖에서 부활한다(20초)
            Stage::Running { .. } if in_rift => Some(self.options.schedule.rift.respawn_ms),
            Stage::Running { .. } => self.running(now_ms).and_then(|(phase, _)| phase.respawn_wait_ms),
            Stage::Ended { end_ms, .. } => Some(end_ms.saturating_sub(now_ms)), // 초기화 때 모두 살아난다
        };
        if in_rift {
            self.rift_member_died(index, now_ms);
        }
        let member = &mut self.members[index];
        member.respawn_at_ms = wait.map(|w| now_ms + w);
        member.eliminated = wait.is_none();
        self.last_death_team = Some(member.team);
        let mut died = make::<SPlayerDied>();
        died.player_id = member.player_id;
        died.killer_id = killer_id;
        died.cause = cause.raw();
        died.respawn_server_ms = member.respawn_at_ms.unwrap_or(0);
        log!("방 {} {}: {} (팀 {}, 원인 {:?})", self.team_code, if member.eliminated { "탈락" } else { "부활 대기" }, member.name, member.team, cause);
        self.broadcast(buffer(&died));
        if check {
            self.check_winner(now_ms);
        }
    }

    /// 진행 중에 남은 팀이 1팀 이하(2팀 이상으로 시작했을 때) 또는 0팀이면 끝낸다.
    fn check_winner(&mut self, now_ms: u32) {
        let Some((phase, _)) = self.running(now_ms) else { return };
        let alive = self.alive_teams();
        let over = if self.started_teams >= 2 { alive.len() <= 1 } else { alive.is_empty() };
        if !over {
            return;
        }
        let winner = match_rules::winner(&alive, self.last_death_team).unwrap_or(0);
        let end_ms = now_ms + self.options.schedule.ended_ms;
        self.stage = Stage::Ended { start_ms: now_ms, end_ms, winner, round: phase.round };
        if winner == 0 {
            log!("방 {} 매치 종료: 무승부(동시 전멸, {}라운드)", self.team_code, phase.round);
        } else {
            log!("방 {} 매치 종료: 팀 {} 승리 ({}라운드)", self.team_code, winner, phase.round);
        }
        self.broadcast_match_state(now_ms);
    }

    /// 자기장 안 안전한 부활 위치: 후보 중 장애물에 막히지 않고 자기장 안쪽에 있으면서 가장 가까운 적과 가장 먼 곳.
    /// 후보는 팀 시작 위치와 자기장 안 무작위 점들이다. 쓸 만한 곳이 없으면 자기장 중심 근처.
    fn safe_respawn_position(&mut self, team: u8, now_ms: u32) -> Vec3 {
        let radius = self.zone_radius(now_ms);
        let inner = (radius * 0.75 - 100.0).max(0.0);
        let enemies: Vec<(f32, f32)> = self.members.iter().filter(|m| m.team != team && !m.dead).map(|m| (m.position.x, m.position.y)).collect();
        let mut candidates = vec![team_spawn_position(team, 1)];
        for _ in 0..RESPAWN_CANDIDATES {
            let angle = self.rng.range(0.0, std::f32::consts::TAU);
            let distance = inner * self.rng.range(0.0, 1.0).sqrt();
            candidates.push(Vec3 { x: ZONE_CENTER.0 + distance * angle.cos(), y: ZONE_CENTER.1 + distance * angle.sin(), z: GROUND_Z });
        }
        let usable = |p: &Vec3| {
            let (x, y) = (p.x, p.y);
            x.abs() <= ARENA_HALF_WIDTH - 100.0
                && y.abs() <= ARENA_HALF_DEPTH - 100.0
                && !match_rules::outside_zone(x, y, inner.max(1.0))
                && !self.options.map.as_ref().is_some_and(|map| map.blocked(Vec3 { x, y, z: 92.0 }, 40.0))
        };
        let score = |p: &Vec3| enemies.iter().map(|&(ex, ey)| (p.x - ex).hypot(p.y - ey)).fold(f32::MAX, f32::min);
        candidates.into_iter().filter(|p| usable(p)).max_by(|a, b| score(a).total_cmp(&score(b))).unwrap_or(Vec3 {
            x: ZONE_CENTER.0 + 150.0,
            y: ZONE_CENTER.1,
            z: GROUND_Z,
        })
    }

    // ---- 몬스터 ----

    fn update_monsters(&mut self, now_ms: u32, dt: f32) {
        let Some(monsters) = &mut self.monsters else { return };
        let players: Vec<MonsterPlayer> = self
            .members
            .iter()
            .map(|m| MonsterPlayer { id: m.player_id, x: m.position.x, y: m.position.y, alive: !m.dead, zone: m.rift_gate.is_some() as u8 })
            .collect();

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
                start.damage = tuning::DASH_DAMAGE * tell.power;
                start.monster_id = tell.monster_id;
                start.yaw = tell.yaw;
                self.broadcast(buffer(&start));
                self.monster_attacks.insert(tell.monster_id, attack_id);
            } else {
                let mut attack = ActiveAttack::new(
                    AttackKind::MonsterArc,
                    tell.hit_ms,
                    at,
                    at,
                    tuning::ARC_RANGE - BODY_RADIUS,
                    tuning::ARC_DAMAGE[tell.kind as usize] * tell.power,
                );
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
            let mut attack =
                ActiveAttack::new(AttackKind::MonsterCharge, now_ms, at, at, tuning::DASH_HIT_DISTANCE - BODY_RADIUS, tuning::DASH_DAMAGE * touch.power);
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
            self.mark_combat(hit.attacker_id, now_ms);
            if hit.hp_after <= 0.0 {
                self.monster_attacks.remove(&hit.monster_id);
                // 처치한 사람(막타)에게 경험치
                self.award_xp(hit.attacker_id, MONSTER_XP[(hit.kind as usize).min(2)], ProgressReason::Monster);
                self.on_monster_killed(hit.monster_id, now_ms);
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
        if self.options.practice_slam_interval_ms > 0 && self.mode == RoomMode::Practice && !self.members.is_empty() && now_ms >= self.next_slam_ms {
            self.next_slam_ms = now_ms + self.options.practice_slam_interval_ms;
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
                if attack.attacker_team != 0 && attack.attacker_team == member.team {
                    continue; // 같은 팀
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
            member.last_combat_ms = Some(attack.hit_ms);
            log!("방 {} 판정: 공격 {} → {} 피격 {:.0} (HP {:.0})", team_code, attack.attack_id, member.name, attack.damage, member.hp);
        }
        result.hp_after = member.hp;

        // 사망: 부활할 때까지(연습 방은 C_Respawn, 매치는 부활 대기 뒤) 몬스터가 노리지 않고 공격·회피를 받지 않는다.
        let died = member.hp <= 0.0;
        if died {
            member.dead = true;
            log!("방 {} 사망: {}", team_code, member.name);
        }
        self.broadcast(buffer(&result));
        if result.result == HitResult::Hit.raw() {
            self.mark_combat(attack.attacker_id, attack.hit_ms);
        }
        if died {
            let (cause, killer) = match attack.kind {
                AttackKind::Slash => (DeathCause::Player, attack.attacker_id),
                _ => (DeathCause::Monster, 0),
            };
            if cause == DeathCause::Player {
                // 처치 경험치: 상대 레벨이 높을수록 많다
                let victim_level = self.members[index].progress.level;
                if let Some(killer_index) = self.find(killer) {
                    let xp = progression::player_kill_xp(self.members[killer_index].progress.level, victim_level);
                    self.award_xp(killer, xp, ProgressReason::Player);
                }
            }
            self.on_death(index, cause, killer, server_time_ms(), true);
        }
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
        if self.mode == RoomMode::Match {
            self.update_match(now_ms); // 단계 진행·자기장 피해·부활
        }

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

        self.send_move_batches(now_ms); // 풀숲에 숨은 사람은 받는 사람마다 다르게(visibility.rs)

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionHandle;

    /// 시험용 짧은 시간표: 1~4라운드 1000/1000/1000/1000ms, 축소 500ms, 부활 대기 300/400/500ms
    fn short_schedule() -> MatchSchedule {
        MatchSchedule {
            round_ms: [1000, 1000, 1000, 1000],
            shrink_ms: 500,
            countdown_ms: 200,
            ended_ms: 300,
            respawn_ms: [300, 400, 500],
            pregame_respawn_ms: 100,
            start_teams: 2,
            // 균열: 2라운드 시작 200ms 뒤 열림, 대기 300, 대결 1000(500부터 축소, 800부터 큰 피해), 합류 200, 보스 800, 부활 200
            rift: match_rules::RiftSchedule {
                open_after_round2_ms: 200,
                wait_ms: 300,
                pvp_ms: 1000,
                pvp_shrink_ms: 500,
                pvp_late_ms: 800,
                join_grace_ms: 200,
                boss_ms: 800,
                respawn_ms: 200,
                result_ms: 100,
                enter_hold_ms: 100,
            },
        }
    }

    fn match_room() -> Room {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/CombatLab.txt");
        let options = RoomOptions {
            match_monsters: false,
            schedule: short_schedule(),
            map: Some(Arc::new(CollisionMap::load(&path).unwrap())),
            ..RoomOptions::default()
        };
        Room::new(RoomMode::Match, 1, options)
    }

    /// 입장시키고 보낸 패킷 수신기를 돌려준다(버리면 송신이 조용히 실패할 뿐 방 동작에는 영향이 없다).
    fn join(room: &mut Room, id: u32, team: u8) -> mpsc::Receiver<Buffer> {
        let (session, receiver, _) = SessionHandle::detached(id, 4096);
        room.enter(session, format!("p{id}"), team);
        receiver
    }

    fn member(room: &Room, id: u32) -> &Member {
        &room.members[room.find(id).unwrap()]
    }

    fn place(room: &mut Room, id: u32, x: f32, y: f32) {
        let index = room.find(id).unwrap();
        let m = &mut room.members[index];
        m.position = Vec3 { x, y, z: GROUND_Z };
        m.history.clear();
        m.history.push_back(PositionSample { server_ms: 0, position: m.position });
    }

    /// 대기 → 카운트다운 → 1라운드까지 진행하고 1라운드 시작 시각을 돌려준다.
    fn start(room: &mut Room, now: u32) -> u32 {
        room.update_match(now);
        assert!(matches!(room.stage, Stage::Countdown { .. }));
        room.update_match(now + 200);
        assert_eq!(room.stage, Stage::Running { start_ms: now + 200 });
        now + 200
    }

    fn kill(room: &mut Room, id: u32, now: u32) {
        let index = room.find(id).unwrap();
        room.members[index].hp = 0.0;
        room.members[index].dead = true;
        room.on_death(index, DeathCause::Monster, 0, now, true);
    }

    fn received_ids(receiver: &mut mpsc::Receiver<Buffer>) -> Vec<u16> {
        let mut ids = Vec::new();
        while let Ok(buffer) = receiver.try_recv() {
            ids.push(u16::from_le_bytes([buffer[2], buffer[3]]));
        }
        ids
    }

    /// 받은 패킷 중 `T`만 꺼낸다(고정 부분과 뒤에 붙은 바이트)
    fn received<T: Packet>(receiver: &mut mpsc::Receiver<Buffer>) -> Vec<(T, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(buffer) = receiver.try_recv() {
            if u16::from_le_bytes([buffer[2], buffer[3]]) == T::ID.raw() {
                let (packet, tail) = read::<T>(&buffer).expect("packet");
                out.push((packet, tail.to_vec()));
            }
        }
        out
    }

    /// 받은 이동 묶음의 항목 전부
    fn move_entries(receiver: &mut mpsc::Receiver<Buffer>) -> Vec<MoveEntry> {
        let mut entries = Vec::new();
        for (_, mut tail) in received::<SMoveBatch>(receiver) {
            while let Some((entry, rest)) = read::<MoveEntry>(&tail) {
                entries.push(entry);
                tail = rest.to_vec();
            }
        }
        entries
    }

    #[test]
    fn bush_hides_players_from_enemies_outside() {
        let mut room = match_room();
        let mut enemy = join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        let mut ally = join(&mut room, 3, 2);
        // 풀숲 16: (1150, -550) 반지름 260
        place(&mut room, 2, 1150.0, -550.0);
        place(&mut room, 1, 1150.0, 200.0); // 7.5m 밖
        place(&mut room, 3, 1150.0, 300.0);
        move_entries(&mut enemy);
        move_entries(&mut ally);
        room.send_move_batches(1000);
        let seen = |entries: &[MoveEntry]| entries.iter().find(|e| { e.player_id } == 2).map(|e| e.state);
        assert_eq!(seen(&move_entries(&mut enemy)), Some(MoveState::Hidden.raw()), "enemy_gets_hidden_once");
        assert_eq!(seen(&move_entries(&mut ally)), None, "ally_sees_normally_no_hidden_entry");
        // 숨어 있는 동안 움직여도 적은 위치를 받지 않는다
        let index = room.find(2).unwrap();
        room.members[index].dirty = true;
        room.send_move_batches(1050);
        assert_eq!(seen(&move_entries(&mut enemy)), None, "no_updates_while_hidden");
        // 3m 안으로 다가오면 보인다
        place(&mut room, 1, 1150.0, -300.0);
        room.send_move_batches(1100);
        assert_eq!(seen(&move_entries(&mut enemy)), Some(MoveState::Idle.raw()), "close_enemy_sees");
        // 멀어지면 다시 숨고, 공격하면 1.5초 드러난다
        place(&mut room, 1, 1150.0, 200.0);
        room.send_move_batches(1150);
        assert_eq!(seen(&move_entries(&mut enemy)), Some(MoveState::Hidden.raw()));
        let index = room.find(2).unwrap();
        room.members[index].revealed_until_ms = 1200 + visibility::ATTACK_REVEAL_MS;
        room.send_move_batches(1200);
        assert_eq!(seen(&move_entries(&mut enemy)), Some(MoveState::Idle.raw()), "attack_reveals");
    }

    /// 균열 시각까지 진행: 2라운드 시작 + 200ms
    fn open_rift(room: &mut Room) -> u32 {
        let t0 = start(room, 0);
        let open = t0 + 1000 + 500 + 200;
        room.update_match(open);
        assert!(matches!(room.rift.stage, rift::RiftStage::Open { .. }));
        open
    }

    /// 입구에 세우고 입장에 필요한 시간만큼 진행한다
    fn walk_into_gate(room: &mut Room, id: u32, gate: usize, now: u32) -> u32 {
        let g = room.layout.rift_gates[gate];
        place(room, id, g.x, g.y);
        room.update_match(now);
        room.update_match(now + 100);
        now + 100
    }

    #[test]
    fn rift_duel_rewards_the_winning_team_and_returns_them() {
        let mut room = match_room();
        join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        let mut now = open_rift(&mut room);
        now = walk_into_gate(&mut room, 1, 0, now + 10);
        assert_eq!(member(&room, 1).rift_gate, Some(0));
        assert!(member(&room, 1).position.y > 5000.0, "teleported_to_arena");
        assert!(matches!(room.rift.stage, rift::RiftStage::Waiting { .. }));
        // 다른 팀은 이미 차지한 입구로 못 들어간다
        now = walk_into_gate(&mut room, 2, 0, now + 10);
        assert_eq!(member(&room, 2).rift_gate, None, "gate_taken_by_other_team");
        now = walk_into_gate(&mut room, 2, 1, now + 10);
        assert!(matches!(room.rift.stage, rift::RiftStage::Pvp { .. }), "second_team_starts_duel");
        let stats_before = member(&room, 1).progress.stat_points;
        kill(&mut room, 2, now + 10);
        assert!(member(&room, 2).rift_used && member(&room, 2).respawn_at_ms == Some(now + 10 + 200), "rift_death_respawns_outside_later");
        room.update_match(now + 20);
        assert!(matches!(room.rift.stage, rift::RiftStage::Finished { winner: 1, .. }));
        // 경험치 400으로 오른 레벨마다 3 + 균열 보상 3
        let progress = member(&room, 1).progress;
        assert!(progress.level > 1, "winner_gets_xp");
        assert_eq!(progress.stat_points, stats_before + 3 * (progress.level as u16 - 1) + 3, "winner_gets_3_bonus_stats");
        let p = member(&room, 1).position;
        assert!(member(&room, 1).rift_gate.is_none() && { p.y } < 1000.0, "winner_back_at_gate");
        // 패배 팀은 밖에서 부활하고 다시 들어갈 수 없다
        room.update_match(now + 300);
        assert!(!member(&room, 2).dead && { member(&room, 2).position.y } < 5000.0);
        walk_into_gate(&mut room, 2, 1, now + 400);
        assert_eq!(member(&room, 2).rift_gate, None);
    }

    #[test]
    fn rift_alone_ends_without_opponent_and_unused_rift_closes() {
        let mut room = match_room();
        join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        let mut now = open_rift(&mut room);
        now = walk_into_gate(&mut room, 1, 0, now + 10);
        // 대기 300ms가 지나면 보스전. 몬스터를 끈 방이라 보상 없이 끝나고 밖으로 나온다
        room.update_match(now + 310);
        assert!(matches!(room.rift.stage, rift::RiftStage::Finished { winner: 0, .. }));
        assert!(member(&room, 1).rift_gate.is_none() && { member(&room, 1).position.y } < 1000.0);

        // 아무도 안 들어가면 2라운드가 끝날 때 닫힌다
        let mut room = match_room();
        join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        let open = open_rift(&mut room);
        room.update_match(open + 900); // 2라운드(1500~2500) 끝 → 축소
        assert_eq!(room.rift.stage, rift::RiftStage::Closed);
        let g = room.layout.rift_gates[0];
        place(&mut room, 1, g.x, g.y);
        room.update_match(open + 950);
        room.update_match(open + 1100);
        assert_eq!(member(&room, 1).rift_gate, None, "closed_rift_cannot_be_entered");
    }

    #[test]
    fn skill_and_stat_commands_follow_the_rules() {
        let mut room = match_room();
        let mut rx = join(&mut room, 1, 1);
        let last = |rx: &mut mpsc::Receiver<Buffer>| received::<SLoadout>(rx).pop().expect("loadout").0;
        assert_eq!({ last(&mut rx).skill_points_left }, 1, "loadout_on_enter");
        let command = |action: SkillAction, number: u8| {
            let mut p = make::<CSkillCommand>();
            p.action = action.raw();
            p.skill_number = number;
            p
        };
        room.skill_command(1, &command(SkillAction::Learn, 2)); // 회전베기: 힘 15 필요
        assert_eq!({ last(&mut rx).result }, LoadoutResult::Locked.raw());
        room.skill_command(1, &command(SkillAction::Learn, 1)); // 강타
        let l = last(&mut rx);
        assert_eq!(({ l.result }, { l.slots[0].skill_number }, { l.slots[0].rank }, { l.skill_points_left }), (LoadoutResult::Ok.raw(), 1, 1, 0));
        let mut stat = make::<CStatCommand>();
        stat.stat = 0;
        stat.amount = 1;
        room.stat_command(1, &stat);
        assert_eq!({ last(&mut rx).result }, LoadoutResult::NoPoints.raw());
        room.award_xp(1, 20, ProgressReason::Monster); // 2레벨: 스킬 +1, 스탯 +3
        assert_eq!({ last(&mut rx).skill_points_left }, 1, "loadout_on_level_up");
        stat.amount = 3;
        room.stat_command(1, &stat);
        assert_eq!({ last(&mut rx).stats }, [8, 5, 5, 5]);
        // 전투 중에는 지울 수 없다
        room.mark_combat(1, server_time_ms());
        room.skill_command(1, &command(SkillAction::Remove, 1));
        assert_eq!({ last(&mut rx).result }, LoadoutResult::InCombat.raw());
        room.members[0].last_combat_ms = None;
        room.skill_command(1, &command(SkillAction::Remove, 1));
        let l = last(&mut rx);
        assert_eq!(({ l.result }, { l.slots[0].skill_number }, { l.skill_points_left }), (LoadoutResult::Ok.raw(), 0, 2));
    }

    #[test]
    fn waits_for_two_teams_then_counts_down_and_starts_at_bases() {
        let mut room = match_room();
        let mut a = join(&mut room, 1, 1);
        assert!(received_ids(&mut a).contains(&PacketId::SMatchState.raw())); // 입장하면 매치 상태를 받는다
        join(&mut room, 2, 1);
        room.update_match(1000);
        assert_eq!(room.stage, Stage::Waiting); // 한 팀뿐
        join(&mut room, 3, 3);
        place(&mut room, 1, 0.0, 0.0);
        let t0 = start(&mut room, 2000);
        // 1라운드 시작: 팀 시작 위치로
        let p = member(&room, 1).position;
        assert_eq!(({ p.x }, { p.y }), ({ team_spawn_position(1, 0).x }, { team_spawn_position(1, 0).y }));
        assert_eq!((member(&room, 2).team_slot, member(&room, 3).team), (1, 3));
        let state = room.match_state(t0 + 1);
        assert_eq!((state.phase, state.round, state.teams_alive, { state.zone_radius_to }), (MatchPhase::Round.raw(), 1, 2, 7100.0));
        // 1라운드 끝 → 축소
        let state = room.match_state(t0 + 1100);
        assert_eq!((state.phase, state.round, { state.zone_radius_from }, { state.zone_radius_to }), (MatchPhase::Shrink.raw(), 1, 7100.0, 4600.0));
    }

    #[test]
    fn countdown_returns_to_waiting_when_a_team_leaves() {
        let mut room = match_room();
        join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        room.update_match(0);
        assert!(matches!(room.stage, Stage::Countdown { .. }));
        room.leave(2);
        room.update_match(100);
        assert_eq!(room.stage, Stage::Waiting);
    }

    #[test]
    fn slash_skips_teammates_and_hits_enemies() {
        let mut room = match_room();
        join(&mut room, 1, 1);
        join(&mut room, 2, 1);
        join(&mut room, 3, 2);
        for (id, x) in [(1, 0.0), (2, 100.0), (3, 100.0)] {
            place(&mut room, id, x, if id == 3 { 60.0 } else { -60.0 });
        }
        let center = Vec3 { x: 100.0, y: 0.0, z: GROUND_Z };
        let mut attack = ActiveAttack::new(AttackKind::Slash, 0, Vec3 { x: 0.0, y: -60.0, z: GROUND_Z }, center, SLASH_RADIUS, SLASH_DAMAGE);
        attack.attacker_id = 1;
        attack.attacker_team = 1;
        room.attacks.push(attack);
        room.update_combat(10_000);
        assert_eq!(member(&room, 2).hp, MAX_HP); // 같은 팀
        assert_eq!(member(&room, 3).hp, MAX_HP - SLASH_DAMAGE); // 다른 팀
    }

    #[test]
    fn deaths_in_rounds_1_to_3_respawn_inside_the_zone_after_the_wait() {
        let mut room = match_room();
        join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        let t0 = start(&mut room, 0);
        kill(&mut room, 1, t0 + 100); // 1라운드: 300ms 대기
        assert_eq!((member(&room, 1).respawn_at_ms, member(&room, 1).eliminated), (Some(t0 + 400), false));
        assert!(matches!(room.stage, Stage::Running { .. })); // 부활할 수 있으니 끝나지 않는다
        room.update_match(t0 + 399);
        assert!(member(&room, 1).dead);
        room.update_match(t0 + 400);
        let m = member(&room, 1);
        assert!(!m.dead && m.hp == MAX_HP);
        assert!(!match_rules::outside_zone(m.position.x, m.position.y, ZONE_RADIUS[0]));
        assert!(!room.options.map.as_ref().unwrap().blocked(Vec3 { x: m.position.x, y: m.position.y, z: 92.0 }, 32.0));
        // 3라운드 뒤 축소(4000~4500ms) 중 사망도 마지막 부활 기회(500ms)
        kill(&mut room, 2, t0 + 4200);
        assert_eq!((member(&room, 2).respawn_at_ms, member(&room, 2).eliminated), (Some(t0 + 4700), false));
    }

    #[test]
    fn round_4_death_eliminates_and_last_team_wins_then_resets() {
        let mut room = match_room();
        let mut watcher = join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        join(&mut room, 3, 2);
        let t0 = start(&mut room, 0);
        let round4 = t0 + 3 * 1000 + 3 * 500 + 10;
        received_ids(&mut watcher);
        kill(&mut room, 2, round4);
        assert!(member(&room, 2).eliminated);
        assert!(matches!(room.stage, Stage::Running { .. })); // 팀 2에 한 명 남음
        kill(&mut room, 3, round4 + 50);
        assert_eq!(room.stage, Stage::Ended { start_ms: round4 + 50, end_ms: round4 + 350, winner: 1, round: 4 });
        let ids = received_ids(&mut watcher);
        assert!(ids.contains(&PacketId::SPlayerDied.raw()) && ids.contains(&PacketId::SMatchState.raw()));
        // 결과 표시가 끝나면 대기로 돌아가고 모두 살아난다
        room.update_match(round4 + 350);
        assert_eq!(room.stage, Stage::Waiting);
        assert!(room.members.iter().all(|m| !m.dead && !m.eliminated && m.hp == MAX_HP));
    }

    #[test]
    fn kills_give_xp_and_a_new_match_resets_levels() {
        let mut room = match_room();
        let mut watcher = join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        let t0 = start(&mut room, 0);
        received_ids(&mut watcher);
        // 몬스터: 지휘관 막타 40
        room.award_xp(1, MONSTER_XP[2], ProgressReason::Monster);
        assert_eq!((member(&room, 1).progress.level, member(&room, 1).progress.xp), (2, 20));
        assert!(received_ids(&mut watcher).contains(&PacketId::SPlayerProgress.raw()));
        // 플레이어 처치: 근접 공격으로 HP 0
        for (id, x) in [(1, 0.0), (2, 100.0)] {
            place(&mut room, id, x, 0.0);
        }
        let victim = room.find(2).unwrap();
        room.members[victim].hp = 10.0;
        room.members[victim].protected_until_ms = 0; // 시작 직후 보호 해제
        let mut attack =
            ActiveAttack::new(AttackKind::Slash, t0, Vec3 { x: 0.0, y: 0.0, z: GROUND_Z }, Vec3 { x: 100.0, y: 0.0, z: GROUND_Z }, SLASH_RADIUS, SLASH_DAMAGE);
        attack.attacker_id = 1;
        attack.attacker_team = 1;
        room.attacks.push(attack);
        room.update_combat(t0 + 1000);
        assert!(member(&room, 2).dead);
        // 2레벨이 1레벨을 잡으면 20 + 10 = 30 → 2레벨 20 + 30 = 50 ≥ 35 → 3레벨 15
        assert_eq!((member(&room, 1).progress.level, member(&room, 1).progress.xp), (3, 15));
        // 다음 판(결과 표시 뒤 초기화)에는 모두 1레벨
        room.stage = Stage::Ended { start_ms: t0, end_ms: t0 + 2000, winner: 1, round: 1 };
        room.update_match(t0 + 2000);
        assert!(room.members.iter().all(|m| m.progress == Progress::default()));
    }

    #[test]
    fn slash_and_wave_cooldowns_are_separate() {
        let mut room = match_room();
        room.options.match_monsters = true;
        room.monsters = Some(MonsterField::new(room.options.map.clone(), 1, 0));
        join(&mut room, 1, 1);
        let mut packet = make::<CAttack>();
        packet.kind = AttackKind::Slash.raw();
        room.attack(1, &packet);
        // 같은 공격 동작에서 0.28초 뒤 나가는 검기는 근접 공격의 재사용 대기에 걸리지 않는다
        packet.kind = AttackKind::Wave.raw();
        let before = room.monsters.as_ref().unwrap().wave_count();
        room.attack(1, &packet);
        assert_eq!(room.monsters.as_ref().unwrap().wave_count(), before + 1);
        // 검기끼리는 0.6초 간격
        room.attack(1, &packet);
        assert_eq!(room.monsters.as_ref().unwrap().wave_count(), before + 1);
    }

    #[test]
    fn xp_scale_multiplies_kill_xp() {
        let mut room = match_room();
        room.options.xp_scale = 3.0;
        join(&mut room, 1, 1);
        room.award_xp(1, 8, ProgressReason::Monster);
        assert_eq!((member(&room, 1).progress.level, member(&room, 1).progress.xp), (2, 4));
        // 24 - 20
    }

    #[test]
    fn zone_damage_hits_outside_players_and_simultaneous_wipe_is_a_draw() {
        let mut room = match_room();
        join(&mut room, 1, 1);
        join(&mut room, 2, 2);
        let t0 = start(&mut room, 0);
        // 1라운드: 반지름 7100 밖(필드 모서리 너머)에 세우면 초당 1%
        place(&mut room, 1, 6000.0, 5000.0);
        room.next_zone_damage_ms = 0;
        room.update_match(t0 + 100);
        assert_eq!((member(&room, 1).hp, member(&room, 2).hp), (MAX_HP - 1.0, MAX_HP));
        // 4라운드: 둘 다 밖, 같은 피해 틱에 동시에 죽으면 무승부
        let round4 = t0 + 3 * 1000 + 3 * 500 + 500;
        for id in [1, 2] {
            place(&mut room, id, 1500.0, 0.0);
            let index = room.find(id).unwrap();
            room.members[index].hp = 5.0;
        }
        room.next_zone_damage_ms = 0;
        room.update_match(round4);
        assert!(member(&room, 1).eliminated && member(&room, 2).eliminated);
        assert!(matches!(room.stage, Stage::Ended { winner: 0, round: 4, .. }));
    }
}
