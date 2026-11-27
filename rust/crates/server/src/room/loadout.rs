//! 스킬 배우기·강화·각성·지우기와 스탯 분배(규칙은 `asc_game::loadout`). 결과는 본인에게 `S_Loadout`으로 알린다.
//!
//! 스탯은 지금 서버 판정(HP·피해)에 쓰지 않는다. 클라이언트 스킬 피해 계산이 쓰고, 서버 스킬 판정을 붙일 때 함께 옮긴다.

use super::Room;
use crate::clock::server_time_ms;
use crate::log;
use asc_protocol::*;

/// 마지막으로 피해를 주고받은 뒤 이 시간 안이면 전투 중(스킬 지우기 불가)
pub(super) const COMBAT_MS: u32 = 5000;

impl Room {
    pub(super) fn mark_combat(&mut self, player_id: u32, now_ms: u32) {
        if let Some(index) = self.find(player_id) {
            self.members[index].last_combat_ms = Some(now_ms);
        }
    }

    pub(super) fn send_loadout(&self, index: usize, result: LoadoutResult, action: u8, skill_number: u8) {
        let member = &self.members[index];
        let mut packet = make::<SLoadout>();
        packet.player_id = member.player_id;
        packet.result = result.raw();
        packet.action = action;
        packet.skill_number = skill_number;
        for (slot, learned) in packet.slots.iter_mut().zip(member.loadout.slots.iter()) {
            if let Some(skill) = learned {
                *slot = SkillSlot { skill_number: skill.number, rank: skill.rank, awakened: skill.awakened as u8 };
            }
        }
        packet.stats = member.loadout.stats;
        packet.skill_points_left = member.loadout.skill_points_left(&member.progress);
        packet.awakening_points_left = member.loadout.awakening_points_left(&member.progress);
        packet.stat_points_left = member.loadout.stat_points_left(&member.progress);
        member.session.send_packet(&packet);
    }

    pub(super) fn skill_command(&mut self, player_id: u32, packet: &CSkillCommand) {
        let Some(index) = self.find(player_id) else { return };
        let now_ms = server_time_ms();
        let number = packet.skill_number;
        let Some(action) = SkillAction::from_raw(packet.action) else {
            self.send_loadout(index, LoadoutResult::Invalid, packet.action, number);
            return;
        };
        let member = &mut self.members[index];
        let in_combat = member.last_combat_ms.is_some_and(|at| now_ms.wrapping_sub(at) < COMBAT_MS);
        let result = if member.dead { LoadoutResult::NotNow } else { member.loadout.apply_skill(action, number, &member.progress, in_combat) };
        log!("방 {} 스킬 {:?} {}번: {} → {:?}", self.team_code, action, number, member.name, result);
        self.send_loadout(index, result, action.raw(), number);
    }

    pub(super) fn stat_command(&mut self, player_id: u32, packet: &CStatCommand) {
        let Some(index) = self.find(player_id) else { return };
        let member = &mut self.members[index];
        let result = member.loadout.apply_stat(packet.stat, packet.amount, &member.progress);
        log!("방 {} 스탯 {}번 +{}: {} → {:?}", self.team_code, packet.stat, packet.amount, member.name, result);
        self.send_loadout(index, result, 0x10 + packet.stat.min(0x0F), 0);
    }
}
