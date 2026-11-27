//! 균열(기획안 "균열"): 2라운드 시작 2분 뒤 중앙 광장에 입구 2개가 열리는 팀 대결.
//!
//! - 입구에 1초 서 있으면 필드 북쪽의 막힌 전투장으로 옮겨진다. 입구 하나에 한 팀, 팀마다 입구 하나, 한 판에 한 번.
//! - 한 팀이 들어가면 1분 동안 상대를 기다린다. 상대 팀이 다른 입구로 들어오면 대결(3분), 아무도 안 오면 보스전(4분).
//! - 대결: 상대 팀 전멸로 승리. 1분 30초부터 전투장 자기장이 중앙으로 좁아져 3분에 닫힌다(밖 피해 5%, 마지막 30초 15%).
//!   두 팀이 함께 전멸하면 마지막에 죽은 사람의 팀이 이기고, 같은 순간이면 둘 다 보상 없음.
//! - 균열 안에서 죽으면 탈락 없이 20초 뒤 밖에서 부활한다(다시 들어갈 수 없다).
//! - 보상: 이긴 팀(보스를 잡은 팀) 모두 경험치와 스탯 포인트 +3. 남아 있던 사람은 들어간 입구로 나온다
//!   (그곳이 자기장 밖이면 안쪽 안전한 곳으로).
//! - 2라운드가 끝날 때까지 아무도 안 들어가면 닫힌다. 판이 끝나거나 새 판이 시작되면 처음 상태로 돌아간다.

use super::{Room, MAX_HP};
use crate::log;
use asc_game::match_rules::{self, RIFT_REWARD_STATS, RIFT_REWARD_XP, RIFT_ZONE_DAMAGE};
use asc_movement::GROUND_Z;
use asc_protocol::*;

const RIFT_STATE_INTERVAL_MS: u32 = 2000;
/// 균열 보스(임시 수치): 망령 지휘관 모습, 체력 1200, 공격 피해 2.5배
pub(super) const BOSS_HP: f32 = 1200.0;
pub(super) const BOSS_POWER: f32 = 2.5;
/// 전투장 안 시작 위치: 입구 0은 중심 서쪽, 1은 동쪽에서 중심을 본다
const ARENA_SPAWN_OFFSET: f32 = 600.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum RiftStage {
    #[default]
    Closed,
    Open {
        start_ms: u32,
    },
    Waiting {
        start_ms: u32,
        end_ms: u32,
    },
    Pvp {
        start_ms: u32,
    },
    Boss {
        start_ms: u32,
        end_ms: u32,
        boss_id: u16,
    },
    Finished {
        start_ms: u32,
        end_ms: u32,
        winner: u8,
    },
}

#[derive(Default)]
pub(super) struct Rift {
    pub(super) stage: RiftStage,
    /// 이 판에서 이미 열렸다(다시 열리지 않는다)
    opened: bool,
    /// 입구를 차지한 팀(0 = 비어 있음)
    pub(super) gate_team: [u8; 2],
    /// 균열 안에서 마지막으로 죽은 (팀, 시각)
    last_death: Option<(u8, u32)>,
    /// 서로 다른 팀이 같은 순간에 죽었다(그 시각)
    draw_ms: Option<u32>,
    sent_key: Option<(u8, [u8; 2])>,
    next_state_ms: u32,
}

impl Room {
    fn rift_phase(&self) -> RiftPhase {
        match self.rift.stage {
            RiftStage::Closed => RiftPhase::Closed,
            RiftStage::Open { .. } => RiftPhase::Open,
            RiftStage::Waiting { .. } => RiftPhase::Waiting,
            RiftStage::Pvp { .. } => RiftPhase::Pvp,
            RiftStage::Boss { .. } => RiftPhase::Boss,
            RiftStage::Finished { .. } => RiftPhase::Finished,
        }
    }

    pub(super) fn rift_state(&self, _now_ms: u32) -> SRiftState {
        let mut state = make::<SRiftState>();
        state.phase = self.rift_phase().raw();
        state.gate_team = self.rift.gate_team;
        for (i, gate) in self.layout.rift_gates.iter().take(2).enumerate() {
            state.gate_x[i] = gate.x;
            state.gate_y[i] = gate.y;
            state.gate_radius = gate.radius;
        }
        if let Some(arena) = self.layout.rift_arena {
            state.arena_x = arena.x;
            state.arena_y = arena.y;
            state.arena_radius = arena.radius;
        }
        state.reward_xp = RIFT_REWARD_XP;
        state.reward_stats = RIFT_REWARD_STATS as u8;
        let schedule = self.options.schedule.rift;
        match self.rift.stage {
            RiftStage::Closed => {}
            RiftStage::Open { start_ms } => state.phase_start_server_ms = start_ms,
            RiftStage::Waiting { start_ms, end_ms } => {
                state.phase_start_server_ms = start_ms;
                state.phase_end_server_ms = end_ms;
            }
            RiftStage::Pvp { start_ms } => {
                state.phase_start_server_ms = start_ms;
                state.phase_end_server_ms = start_ms + schedule.pvp_ms;
                state.zone_shrink_start_server_ms = start_ms + schedule.pvp_shrink_ms;
                state.zone_shrink_end_server_ms = start_ms + schedule.pvp_ms;
                state.zone_late_server_ms = start_ms + schedule.pvp_late_ms;
                state.zone_damage_early = RIFT_ZONE_DAMAGE[0];
                state.zone_damage_late = RIFT_ZONE_DAMAGE[1];
            }
            RiftStage::Boss { start_ms, end_ms, boss_id } => {
                state.phase_start_server_ms = start_ms;
                state.phase_end_server_ms = end_ms;
                state.boss_id = boss_id;
            }
            RiftStage::Finished { start_ms, end_ms, winner } => {
                state.phase_start_server_ms = start_ms;
                state.phase_end_server_ms = end_ms;
                state.winner_team = winner;
            }
        }
        state
    }

    pub(super) fn maybe_broadcast_rift_state(&mut self, now_ms: u32) {
        if !self.layout.has_rift() {
            return;
        }
        let key = (self.rift_phase().raw(), self.rift.gate_team);
        if self.rift.sent_key != Some(key) || now_ms >= self.rift.next_state_ms {
            self.rift.sent_key = Some(key);
            self.rift.next_state_ms = now_ms + RIFT_STATE_INTERVAL_MS;
            self.broadcast(buffer(&self.rift_state(now_ms)));
        }
    }

    /// 새 판·판 초기화: 균열을 처음 상태로
    pub(super) fn reset_rift(&mut self, now_ms: u32) {
        self.remove_boss();
        for member in &mut self.members {
            member.rift_gate = None;
            member.rift_used = false;
            member.gate_hold = None;
        }
        self.rift = Rift::default();
        self.maybe_broadcast_rift_state(now_ms);
    }

    fn remove_boss(&mut self) {
        if let RiftStage::Boss { boss_id, .. } = self.rift.stage {
            if let Some(monsters) = &mut self.monsters {
                let mut events = std::mem::take(&mut self.monster_events);
                events.clear();
                monsters.remove(boss_id, &mut events);
                self.apply_monster_events(&events, crate::clock::server_time_ms());
                self.monster_events = events;
            }
        }
    }

    /// 전투장 자기장 반지름(대결 중에만 준다)
    fn rift_zone_radius(&self, now_ms: u32) -> Option<f32> {
        let RiftStage::Pvp { start_ms } = self.rift.stage else { return None };
        let arena = self.layout.rift_arena?;
        let schedule = self.options.schedule.rift;
        let (from, to) = (start_ms + schedule.pvp_shrink_ms, start_ms + schedule.pvp_ms);
        if now_ms <= from {
            return Some(arena.radius);
        }
        let t = ((now_ms - from) as f32 / (to - from).max(1) as f32).clamp(0.0, 1.0);
        Some(arena.radius * (1.0 - t))
    }

    /// 1초마다: 대결 중 전투장 자기장 밖 피해
    pub(super) fn apply_rift_zone_damage(&mut self, now_ms: u32) {
        let RiftStage::Pvp { start_ms } = self.rift.stage else { return };
        let schedule = self.options.schedule.rift;
        if now_ms < start_ms + schedule.pvp_shrink_ms {
            return;
        }
        let (Some(radius), Some(arena)) = (self.rift_zone_radius(now_ms), self.layout.rift_arena) else { return };
        let percent = if now_ms < start_ms + schedule.pvp_late_ms { RIFT_ZONE_DAMAGE[0] } else { RIFT_ZONE_DAMAGE[1] };
        let damage = MAX_HP * percent / 100.0;
        for index in 0..self.members.len() {
            let member = &mut self.members[index];
            if member.dead || member.rift_gate.is_none() || (member.position.x - arena.x).hypot(member.position.y - arena.y) <= radius {
                continue;
            }
            member.hp = (member.hp - damage).max(0.0);
            let mut result = make::<SHitResult>();
            result.attack_id = 0;
            result.target_id = member.player_id;
            result.result = HitResult::Hit.raw();
            result.damage = damage;
            result.hp_after = member.hp;
            result.hit_server_ms = now_ms;
            let died = member.hp <= 0.0;
            if died {
                member.dead = true;
                log!("방 {} 균열 자기장 사망: {}", self.team_code, member.name);
            }
            self.broadcast(buffer(&result));
            if died {
                self.on_death(index, DeathCause::Zone, 0, now_ms, true);
            }
        }
    }

    /// 균열 안에서 죽었다(on_death가 부른다): 균열에서 빠지고 다시 못 들어간다.
    pub(super) fn rift_member_died(&mut self, index: usize, now_ms: u32) {
        let team = self.members[index].team;
        self.members[index].rift_gate = None;
        self.members[index].rift_used = true;
        if let Some((last_team, last_ms)) = self.rift.last_death {
            if last_ms == now_ms && last_team != team {
                self.rift.draw_ms = Some(now_ms);
            }
        }
        self.rift.last_death = Some((team, now_ms));
    }

    /// 몬스터가 죽었다: 균열 보스면 보스전 승리
    pub(super) fn on_monster_killed(&mut self, monster_id: u16, now_ms: u32) {
        if let RiftStage::Boss { boss_id, .. } = self.rift.stage {
            if boss_id == monster_id {
                self.rift.stage = RiftStage::Closed; // 보스는 이미 없다(remove_boss가 건너뛴다)
                let team = self.rift.gate_team.iter().copied().find(|&t| t != 0).unwrap_or(0);
                self.finish_rift(team, now_ms);
            }
        }
    }

    fn team_alive_in_rift(&self, team: u8) -> bool {
        team != 0 && self.members.iter().any(|m| m.team == team && m.rift_gate.is_some() && !m.dead)
    }

    /// 틱마다(진행 중): 열기 → 입장 → 단계 진행 → 결과
    pub(super) fn update_rift(&mut self, now_ms: u32) {
        if !self.layout.has_rift() {
            return;
        }
        let Some((phase, elapsed)) = self.running(now_ms) else { return };
        let schedule = self.options.schedule.rift;
        match self.rift.stage {
            RiftStage::Closed => {
                if !self.rift.opened && elapsed >= self.options.schedule.rift_open_offset_ms() {
                    self.rift.opened = true;
                    self.rift.stage = RiftStage::Open { start_ms: now_ms };
                    log!("방 {} 균열 열림", self.team_code);
                }
                return;
            }
            RiftStage::Open { .. } => {
                if !(phase.phase == MatchPhase::Round && phase.round == 2) {
                    self.rift.stage = RiftStage::Closed; // 2라운드 동안 아무도 안 들어갔다
                    log!("방 {} 균열 닫힘(아무도 들어가지 않음)", self.team_code);
                    return;
                }
            }
            RiftStage::Waiting { end_ms, .. } => {
                let team = self.rift.gate_team.iter().copied().find(|&t| t != 0).unwrap_or(0);
                if !self.team_alive_in_rift(team) {
                    self.finish_rift(0, now_ms); // 기다리던 팀이 다 죽었다
                    return;
                }
                if now_ms >= end_ms {
                    self.start_boss(now_ms);
                    return;
                }
            }
            RiftStage::Pvp { start_ms } => {
                let [a, b] = self.rift.gate_team;
                match (self.team_alive_in_rift(a), self.team_alive_in_rift(b)) {
                    (true, true) => {}
                    (true, false) => return self.finish_rift(a, now_ms),
                    (false, true) => return self.finish_rift(b, now_ms),
                    (false, false) => {
                        // 함께 전멸: 마지막에 죽은 사람의 팀. 같은 순간이면 둘 다 보상 없음
                        let winner = match (self.rift.last_death, self.rift.draw_ms) {
                            (Some((_, ms)), Some(draw)) if ms == draw => 0,
                            (Some((team, _)), _) => team,
                            (None, _) => 0,
                        };
                        return self.finish_rift(winner, now_ms);
                    }
                }
                if now_ms >= start_ms + schedule.pvp_ms {
                    return self.finish_rift(0, now_ms);
                }
            }
            RiftStage::Boss { end_ms, .. } => {
                let team = self.rift.gate_team.iter().copied().find(|&t| t != 0).unwrap_or(0);
                if now_ms >= end_ms || !self.team_alive_in_rift(team) {
                    return self.finish_rift(0, now_ms); // 시간 초과·전멸: 보상 없음
                }
                return;
            }
            RiftStage::Finished { end_ms, .. } => {
                if now_ms >= end_ms {
                    self.rift.stage = RiftStage::Closed;
                }
                return;
            }
        }
        self.update_gate_entries(now_ms);
    }

    /// 입구에 서 있는 사람을 확인해 들여보낸다.
    fn update_gate_entries(&mut self, now_ms: u32) {
        let schedule = self.options.schedule.rift;
        for index in 0..self.members.len() {
            let member = &self.members[index];
            let gate = self.layout.rift_gates.iter().position(|g| g.contains(member.position.x, member.position.y));
            let allowed = gate.is_some_and(|g| {
                if member.dead || member.eliminated || member.rift_used || member.rift_gate.is_some() {
                    return false;
                }
                let (owner, other) = (self.rift.gate_team[g], self.rift.gate_team[1 - g]);
                match self.rift.stage {
                    RiftStage::Open { .. } => owner == 0,
                    // 입구 하나에 한 팀, 팀마다 입구 하나
                    RiftStage::Waiting { .. } => owner == member.team || (owner == 0 && other != member.team),
                    // 대결이 시작된 뒤 잠시 같은 팀원이 자기 입구로 따라 들어올 수 있다
                    RiftStage::Pvp { start_ms } => owner == member.team && now_ms < start_ms + schedule.join_grace_ms,
                    _ => false,
                }
            });
            let Some(gate) = gate.filter(|_| allowed) else {
                self.members[index].gate_hold = None;
                continue;
            };
            match self.members[index].gate_hold {
                Some((held, since)) if held == gate => {
                    if now_ms.wrapping_sub(since) >= schedule.enter_hold_ms {
                        self.enter_rift(index, gate, now_ms);
                    }
                }
                _ => self.members[index].gate_hold = Some((gate, now_ms)),
            }
        }
    }

    fn enter_rift(&mut self, index: usize, gate: usize, now_ms: u32) {
        let Some(arena) = self.layout.rift_arena else { return };
        let schedule = self.options.schedule.rift;
        let team = self.members[index].team;
        self.rift.gate_team[gate] = team;
        self.members[index].rift_gate = Some(gate);
        self.members[index].rift_used = true;
        let side = if gate == 0 { -1.0 } else { 1.0 };
        let spread = (self.members[index].team_slot as f32 - 1.0) * 150.0;
        let position = Vec3 { x: arena.x + side * ARENA_SPAWN_OFFSET, y: arena.y + spread, z: GROUND_Z };
        let yaw = if gate == 0 { 0.0 } else { 180.0 };
        log!("방 {} 균열 입장: {} (팀 {}, 입구 {})", self.team_code, self.members[index].name, team, gate);
        self.teleport(index, position, yaw, TeleportReason::RiftEnter, now_ms);
        match self.rift.stage {
            RiftStage::Open { .. } => self.rift.stage = RiftStage::Waiting { start_ms: now_ms, end_ms: now_ms + schedule.wait_ms },
            RiftStage::Waiting { .. } if self.rift.gate_team[1 - gate] != 0 && self.rift.gate_team[1 - gate] != team => {
                self.rift.stage = RiftStage::Pvp { start_ms: now_ms };
                log!("방 {} 균열 대결 시작: 팀 {} 대 팀 {}", self.team_code, self.rift.gate_team[0], self.rift.gate_team[1]);
            }
            _ => {}
        }
    }

    fn start_boss(&mut self, now_ms: u32) {
        let schedule = self.options.schedule.rift;
        let Some(arena) = self.layout.rift_arena else { return };
        let Some(monsters) = &mut self.monsters else {
            self.finish_rift(0, now_ms); // 몬스터를 끈 서버: 보스전 없음
            return;
        };
        let boss = monsters.spawn_at(2, arena.x, arena.y, now_ms);
        boss.max_hp = BOSS_HP;
        boss.hp = BOSS_HP;
        boss.power = BOSS_POWER;
        boss.zone = 1;
        let boss_id = boss.id;
        self.rift.stage = RiftStage::Boss { start_ms: now_ms, end_ms: now_ms + schedule.boss_ms, boss_id };
        log!("방 {} 균열 보스전 시작 (보스 {}, 제한 {}초)", self.team_code, boss_id, schedule.boss_ms / 1000);
    }

    /// 결과: 보상을 주고 남은 사람을 입구로 내보낸다.
    fn finish_rift(&mut self, winner: u8, now_ms: u32) {
        self.remove_boss();
        if winner != 0 {
            log!("방 {} 균열 승리: 팀 {} (경험치 {}, 스탯 +{})", self.team_code, winner, RIFT_REWARD_XP, RIFT_REWARD_STATS);
            let ids: Vec<u32> = self.members.iter().filter(|m| m.team == winner).map(|m| m.player_id).collect();
            for id in ids {
                if let Some(index) = self.find(id) {
                    self.members[index].progress.stat_points += RIFT_REWARD_STATS;
                }
                self.award_xp(id, RIFT_REWARD_XP, ProgressReason::Rift);
                if let Some(index) = self.find(id) {
                    self.send_loadout(index, LoadoutResult::Ok, 0, 0);
                }
            }
        } else {
            log!("방 {} 균열 끝: 보상 없음", self.team_code);
        }
        let zone = self.zone_radius(now_ms);
        for index in 0..self.members.len() {
            let Some(gate) = self.members[index].rift_gate else { continue };
            self.members[index].rift_gate = None;
            if self.members[index].dead {
                continue;
            }
            let at = self.layout.rift_gates[gate];
            let spread = (self.members[index].team_slot as f32 - 1.0) * 120.0;
            let mut position = Vec3 { x: at.x + spread, y: at.y - at.radius - 80.0, z: GROUND_Z };
            if match_rules::outside_zone(position.x, position.y, zone) {
                let team = self.members[index].team;
                position = self.safe_respawn_position(team, now_ms);
            }
            self.teleport(index, position, -90.0, TeleportReason::RiftExit, now_ms);
        }
        let result_ms = self.options.schedule.rift.result_ms;
        self.rift.stage = RiftStage::Finished { start_ms: now_ms, end_ms: now_ms + result_ms, winner };
    }
}

fn buffer<T: Packet>(packet: &T) -> crate::session::Buffer {
    super::buffer(packet)
}
