//! 풀숲: 안에 들어간 플레이어는 밖에 있는 적에게 보이지 않는다(기획안 "필드 > 지형").
//!
//! - 같은 팀, 같은 풀숲 안, 아주 가까운(3m) 적에게는 보인다.
//! - 공격(근접·검기)하면 1.5초 동안 드러난다.
//! - 숨은 사람의 이동은 받는 사람마다 따로 거른다. 보이다가 숨으면 `MoveState::Hidden` 항목을 한 번 보내고,
//!   다시 보이면 보통 항목을 바로 보낸다. 숨어 있는 동안에는 위치를 보내지 않는다(화면 밖 정보 노출 방지).
//!
//! 세부 수치(드러나는 거리, 공격 후 시간)는 기획안에 없어 임시값이다.

use super::{Member, Room};
use asc_protocol::*;
use bytemuck::Zeroable;

/// 이 거리 안의 적에게는 풀숲 안에서도 보인다
pub(super) const BUSH_REVEAL_DISTANCE: f32 = 300.0;
/// 공격한 뒤 풀숲 안에서도 보이는 시간
pub(super) const ATTACK_REVEAL_MS: u32 = 1500;

impl Room {
    /// `viewer`에게 `target`이 보이는가
    pub(super) fn visible_to(&self, viewer: &Member, target: &Member, now_ms: u32) -> bool {
        if self.mode != RoomMode::Match || viewer.team == target.team {
            return true;
        }
        let Some(bush) = target.bush else { return true };
        if now_ms < target.revealed_until_ms || viewer.bush == Some(bush) {
            return true;
        }
        let (dx, dy) = (viewer.position.x - target.position.x, viewer.position.y - target.position.y);
        dx.hypot(dy) < BUSH_REVEAL_DISTANCE
    }

    fn update_bushes(&mut self) {
        for index in 0..self.members.len() {
            let member = &self.members[index];
            let bush = if member.dead || member.rift_gate.is_some() { None } else { self.layout.bush_at(member.position.x, member.position.y) };
            self.members[index].bush = bush;
        }
    }

    /// 받는 사람마다 이동 묶음을 만들어 보낸다. 자기 항목은 넣지 않는다(본인은 S_MoveAck로 안다).
    pub(super) fn send_move_batches(&mut self, now_ms: u32) {
        self.update_bushes();
        let entry = |m: &Member| MoveEntry { player_id: m.player_id, position: m.position, velocity: m.velocity, yaw: m.yaw, state: m.state };
        let mut entries: Vec<MoveEntry> = Vec::new();
        let mut changes: Vec<((u32, u32), bool)> = Vec::new();
        for viewer in &self.members {
            entries.clear();
            for target in &self.members {
                if target.player_id == viewer.player_id {
                    continue;
                }
                let key = (viewer.player_id, target.player_id);
                let was_hidden = self.hidden_pairs.contains(&key);
                if self.visible_to(viewer, target, now_ms) {
                    if was_hidden {
                        changes.push((key, false));
                        entries.push(entry(target)); // 다시 보인다: 바로 지금 위치
                    } else if target.dirty {
                        entries.push(entry(target));
                    }
                } else if !was_hidden {
                    changes.push((key, true));
                    entries.push(MoveEntry {
                        player_id: target.player_id,
                        position: Vec3::zeroed(),
                        velocity: Vec3::zeroed(),
                        yaw: 0.0,
                        state: MoveState::Hidden.raw(),
                    });
                }
            }
            if !entries.is_empty() {
                let mut batch = make::<SMoveBatch>();
                batch.server_tick = self.tick;
                viewer.session.send(encode_with_entries(batch, &entries).into());
            }
        }
        for (key, hidden) in changes {
            if hidden {
                self.hidden_pairs.insert(key);
            } else {
                self.hidden_pairs.remove(&key);
            }
        }
        for member in &mut self.members {
            member.dirty = false;
        }
    }
}
