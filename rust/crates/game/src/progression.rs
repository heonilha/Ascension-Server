//! 레벨·경험치·포인트(스킬·각성·스탯). 네트워크를 모르는 순수 계산이다.
//!
//! 기준: Ascension-Team `Docs/11.26기획안.md` "레벨과 스킬 포인트", "스탯".
//! - 최대 레벨 20. 1레벨부터 레벨마다 스킬 포인트 1개(12레벨에 12개), 15·17레벨에 각성 포인트 1개.
//! - 레벨업마다 스탯 포인트 3개(12레벨 33 · 15레벨 42 · 17레벨 48 · 20레벨 57).
//! - 경험치는 몬스터 처치 + 플레이어 처치. 나보다 레벨 높은 플레이어를 잡으면 더 많이 받는다.
//!
//! 기획안에 아직 없는 값(임시, 플레이 시험으로 조정): 레벨별 필요 경험치(`xp_to_next`),
//! 몬스터 처치 경험치(`MONSTER_XP`, 1인 데모 경험치 구슬 값), 플레이어 처치 경험치(`player_kill_xp`).
//! 1라운드(15분) 끝에 12레벨이 되려면 1,045 경험치가 필요하다(몬스터 검병 기준 약 9초에 한 마리).

pub const MAX_LEVEL: u8 = 20;
/// 이 레벨까지 레벨마다 스킬 포인트를 1개 받는다
pub const SKILL_POINT_LAST_LEVEL: u8 = 12;
/// 각성 포인트를 받는 레벨
pub const AWAKENING_LEVELS: [u8; 2] = [15, 17];
/// 레벨업 한 번에 받는 스탯 포인트
pub const STAT_POINTS_PER_LEVEL: u16 = 3;
/// 몬스터 종류(0 망령 검병, 1 폭주 망령, 2 망령 지휘관)별 처치 경험치
pub const MONSTER_XP: [u32; 3] = [8, 12, 40];

/// `level`에서 다음 레벨까지 필요한 경험치. 최대 레벨이면 0.
pub fn xp_to_next(level: u8) -> u32 {
    if level >= MAX_LEVEL {
        return 0;
    }
    20 + 15 * (level.max(1) as u32 - 1)
}

/// 1레벨 0 경험치에서 `level`까지 필요한 경험치 합
pub fn total_xp_for(level: u8) -> u32 {
    (1..level.min(MAX_LEVEL)).map(xp_to_next).sum()
}

/// 레벨 `victim`인 플레이어를 레벨 `killer`가 쓰러뜨렸을 때의 경험치.
/// 기본 20 + 상대 레벨 × 10, 상대가 높으면 레벨 차 1마다 25%씩 더(최대 2배).
pub fn player_kill_xp(killer: u8, victim: u8) -> u32 {
    let base = 20.0 + 10.0 * victim as f32;
    let bonus = (1.0 + 0.25 * victim.saturating_sub(killer) as f32).min(2.0);
    (base * bonus).round() as u32
}

/// 플레이어 한 명의 성장 상태(한 판 동안 유지, 죽어도 잃지 않는다).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub level: u8,
    /// 현재 레벨 안에서 모은 경험치
    pub xp: u32,
    /// 받은 포인트 합(쓰는 것은 클라이언트의 스킬·스탯 화면이 맡는다)
    pub skill_points: u8,
    pub awakening_points: u8,
    pub stat_points: u16,
}

impl Default for Progress {
    fn default() -> Self {
        // 1레벨에 첫 스킬 포인트 1개
        Progress { level: 1, xp: 0, skill_points: 1, awakening_points: 0, stat_points: 0 }
    }
}

impl Progress {
    /// 경험치를 더하고 오른 레벨 수를 돌려준다. 최대 레벨에서는 경험치가 더 쌓이지 않는다.
    pub fn gain(&mut self, amount: u32) -> u8 {
        let mut gained = 0;
        if self.level >= MAX_LEVEL {
            return 0;
        }
        self.xp += amount;
        while self.level < MAX_LEVEL && self.xp >= xp_to_next(self.level) {
            self.xp -= xp_to_next(self.level);
            self.level += 1;
            gained += 1;
            self.stat_points += STAT_POINTS_PER_LEVEL;
            if self.level <= SKILL_POINT_LAST_LEVEL {
                self.skill_points += 1;
            }
            if AWAKENING_LEVELS.contains(&self.level) {
                self.awakening_points += 1;
            }
        }
        if self.level >= MAX_LEVEL {
            self.xp = 0;
        }
        gained
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_reaches_round_targets() {
        assert_eq!((xp_to_next(1), xp_to_next(11), xp_to_next(19), xp_to_next(20)), (20, 170, 290, 0));
        assert_eq!(total_xp_for(12), 1045);
        assert_eq!(total_xp_for(15), 1645);
        assert_eq!(total_xp_for(17), 2120);
        assert_eq!(total_xp_for(20), 2945);
    }

    #[test]
    fn points_follow_the_plan_table() {
        let at = |level: u8| {
            let mut p = Progress::default();
            p.gain(total_xp_for(level));
            assert_eq!((p.level, p.xp), (level, 0));
            (p.skill_points, p.awakening_points, p.stat_points)
        };
        assert_eq!(at(1), (1, 0, 0));
        assert_eq!(at(4), (4, 0, 9));
        assert_eq!(at(12), (12, 0, 33));
        assert_eq!(at(15), (12, 1, 42));
        assert_eq!(at(17), (12, 2, 48));
        assert_eq!(at(20), (12, 2, 57));
    }

    #[test]
    fn gain_carries_over_and_stops_at_max() {
        let mut p = Progress::default();
        assert_eq!(p.gain(19), 0);
        assert_eq!(p.gain(1 + 35 + 5), 2); // 20 → 2레벨, 35 → 3레벨, 남은 5
        assert_eq!((p.level, p.xp), (3, 5));
        assert!(p.gain(100_000) > 0);
        assert_eq!((p.level, p.xp), (MAX_LEVEL, 0));
        assert_eq!(p.gain(500), 0);
        assert_eq!(p.xp, 0);
    }

    #[test]
    fn higher_level_victims_give_more() {
        assert_eq!(player_kill_xp(5, 5), 70);
        assert_eq!(player_kill_xp(10, 5), 70); // 낮은 상대는 보너스 없음
        assert_eq!(player_kill_xp(5, 7), 135); // 90 × 1.5
        assert_eq!(player_kill_xp(1, 20), 440); // 최대 2배
    }
}
