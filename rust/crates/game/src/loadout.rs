//! 스킬 배우기·강화·각성·지우기와 스탯 분배 규칙. 네트워크를 모르는 순수 계산이다.
//!
//! 기준: Ascension-Team `Docs/11.26기획안.md` "레벨과 스킬 포인트", "스탯", "고급 스킬 각성".
//! - 스킬은 검·활 합쳐 4개까지. 배우기 1 + 강화 2 = 최대 3포인트.
//! - 해금: 초급 = 해당 스탯 5, 중급 = 15, 고급 = 25 + 12레벨.
//! - 각성: 최대 강화된 고급 스킬에 각성 포인트 1(스킬당 1번).
//! - 지우면 쓴 스킬·각성 포인트를 모두 돌려받는다(전투 중 불가: 방이 판단해 넘긴다).
//! - 스탯은 모두 5에서 시작, 받은 스탯 포인트를 분배한다(되돌릴 수 없다).
//!
//! 스킬 번호 = 무기 × 12 + 스탯 × 3 + 티어 + 1 (무기 0 검·1 활, 스탯 0 힘·1 민첩·2 지능·3 체력, 티어 0 초급·1 중급·2 고급).
//! 클라이언트 스킬 표(`FCLSkillDef`의 Weapon·Stat·Tier)에서 같은 식으로 번호를 만든다.

use crate::progression::Progress;
use asc_protocol::{LoadoutResult, SkillAction, SKILL_COUNT, SKILL_MAX_RANK, SKILL_SLOTS, START_STAT};

/// 티어별 해금 스탯
pub const TIER_STAT: [u16; 3] = [5, 15, 25];
/// 고급 스킬 해금 레벨
pub const ADVANCED_LEVEL: u8 = 12;

/// 스킬 번호에서 (무기, 스탯, 티어). 범위 밖이면 `None`.
pub fn skill_parts(number: u8) -> Option<(u8, u8, u8)> {
    if number == 0 || number > SKILL_COUNT {
        return None;
    }
    let n = number - 1;
    Some((n / 12, (n % 12) / 3, n % 3))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LearnedSkill {
    pub number: u8,
    pub rank: u8,
    pub awakened: bool,
}

/// 한 사람의 스킬·스탯 분배(한 판 동안 유지)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loadout {
    pub slots: [Option<LearnedSkill>; SKILL_SLOTS],
    /// 힘·민첩·지능·체력
    pub stats: [u16; 4],
}

impl Default for Loadout {
    fn default() -> Self {
        Loadout { slots: [None; SKILL_SLOTS], stats: [START_STAT; 4] }
    }
}

impl Loadout {
    pub fn skill_points_spent(&self) -> u8 {
        self.slots.iter().flatten().map(|s| s.rank).sum()
    }

    pub fn awakening_points_spent(&self) -> u8 {
        self.slots.iter().flatten().filter(|s| s.awakened).count() as u8
    }

    pub fn stat_points_spent(&self) -> u16 {
        self.stats.iter().map(|&s| s - START_STAT).sum()
    }

    pub fn skill_points_left(&self, progress: &Progress) -> u8 {
        progress.skill_points.saturating_sub(self.skill_points_spent())
    }

    pub fn awakening_points_left(&self, progress: &Progress) -> u8 {
        progress.awakening_points.saturating_sub(self.awakening_points_spent())
    }

    pub fn stat_points_left(&self, progress: &Progress) -> u16 {
        progress.stat_points.saturating_sub(self.stat_points_spent())
    }

    fn find(&self, number: u8) -> Option<usize> {
        self.slots.iter().position(|s| s.is_some_and(|s| s.number == number))
    }

    /// 지금 배울 수 있는가(포인트·빈 칸 제외, 해금 조건만)
    pub fn unlocked(&self, number: u8, progress: &Progress) -> bool {
        let Some((_, stat, tier)) = skill_parts(number) else { return false };
        self.stats[stat as usize] >= TIER_STAT[tier as usize] && (tier < 2 || progress.level >= ADVANCED_LEVEL)
    }

    /// 스킬 명령. `in_combat`이면 지우기를 거부한다.
    pub fn apply_skill(&mut self, action: SkillAction, number: u8, progress: &Progress, in_combat: bool) -> LoadoutResult {
        let Some((_, _, tier)) = skill_parts(number) else { return LoadoutResult::Invalid };
        let index = self.find(number);
        match action {
            SkillAction::Learn => {
                if index.is_some() {
                    return LoadoutResult::AlreadyLearned;
                }
                let Some(free) = self.slots.iter().position(|s| s.is_none()) else { return LoadoutResult::SlotsFull };
                if !self.unlocked(number, progress) {
                    return LoadoutResult::Locked;
                }
                if self.skill_points_left(progress) == 0 {
                    return LoadoutResult::NoPoints;
                }
                self.slots[free] = Some(LearnedSkill { number, rank: 1, awakened: false });
            }
            SkillAction::Upgrade => {
                let Some(index) = index else { return LoadoutResult::NotLearned };
                let skill = self.slots[index].as_mut().expect("찾은 칸");
                if skill.rank >= SKILL_MAX_RANK {
                    return LoadoutResult::MaxRank;
                }
                if progress.skill_points.saturating_sub(self.skill_points_spent()) == 0 {
                    return LoadoutResult::NoPoints;
                }
                self.slots[index].as_mut().expect("찾은 칸").rank += 1;
            }
            SkillAction::Awaken => {
                let Some(index) = index else { return LoadoutResult::NotLearned };
                let skill = self.slots[index].expect("찾은 칸");
                if tier != 2 || skill.rank < SKILL_MAX_RANK {
                    return LoadoutResult::Locked;
                }
                if skill.awakened {
                    return LoadoutResult::MaxRank;
                }
                if self.awakening_points_left(progress) == 0 {
                    return LoadoutResult::NoPoints;
                }
                self.slots[index].as_mut().expect("찾은 칸").awakened = true;
            }
            SkillAction::Remove => {
                let Some(index) = index else { return LoadoutResult::NotLearned };
                if in_combat {
                    return LoadoutResult::InCombat;
                }
                self.slots[index] = None;
            }
        }
        LoadoutResult::Ok
    }

    /// 스탯 분배
    pub fn apply_stat(&mut self, stat: u8, amount: u8, progress: &Progress) -> LoadoutResult {
        if stat > 3 || amount == 0 {
            return LoadoutResult::Invalid;
        }
        if self.stat_points_left(progress) < amount as u16 {
            return LoadoutResult::NoPoints;
        }
        self.stats[stat as usize] += amount as u16;
        LoadoutResult::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progression::total_xp_for;

    fn at_level(level: u8) -> Progress {
        let mut p = Progress::default();
        p.gain(total_xp_for(level));
        p
    }

    const SMASH: u8 = 1; // 검 힘 초급(강타)
    const SPIN: u8 = 2; // 검 힘 중급(회전베기)
    const QUAKE: u8 = 3; // 검 힘 고급(지진참)

    #[test]
    fn numbers_follow_weapon_stat_tier() {
        assert_eq!(skill_parts(1), Some((0, 0, 0)));
        assert_eq!(skill_parts(9), Some((0, 2, 2))); // 엑스칼리버
        assert_eq!(skill_parts(14), Some((1, 0, 1))); // 꿰뚫기
        assert_eq!(skill_parts(24), Some((1, 3, 2)));
        assert_eq!((skill_parts(0), skill_parts(25)), (None, None));
    }

    #[test]
    fn learn_needs_points_slots_and_unlock() {
        let mut l = Loadout::default();
        let p = at_level(1); // 스킬 포인트 1
        assert_eq!(l.apply_skill(SkillAction::Learn, SPIN, &p, false), LoadoutResult::Locked); // 힘 15 필요
        assert_eq!(l.apply_skill(SkillAction::Learn, SMASH, &p, false), LoadoutResult::Ok);
        assert_eq!(l.apply_skill(SkillAction::Learn, SMASH, &p, false), LoadoutResult::AlreadyLearned);
        assert_eq!(l.apply_skill(SkillAction::Learn, 4, &p, false), LoadoutResult::NoPoints);
        let p = at_level(6); // 6포인트
        for n in [4, 7, 10] {
            assert_eq!(l.apply_skill(SkillAction::Learn, n, &p, false), LoadoutResult::Ok);
        }
        assert_eq!(l.apply_skill(SkillAction::Learn, 13, &p, false), LoadoutResult::SlotsFull);
        assert_eq!(l.skill_points_left(&p), 2);
    }

    #[test]
    fn upgrade_awaken_and_remove_refund() {
        let mut l = Loadout::default();
        let p = at_level(15); // 스킬 12, 각성 1, 스탯 42
        assert_eq!(l.apply_skill(SkillAction::Learn, QUAKE, &p, false), LoadoutResult::Locked); // 힘 25 필요
        assert_eq!(l.apply_stat(0, 20, &p), LoadoutResult::Ok);
        assert_eq!(l.stats, [25, 5, 5, 5]);
        assert_eq!(l.apply_skill(SkillAction::Learn, QUAKE, &p, false), LoadoutResult::Ok);
        assert_eq!(l.apply_skill(SkillAction::Awaken, QUAKE, &p, false), LoadoutResult::Locked); // 3단계 먼저
        assert_eq!(l.apply_skill(SkillAction::Upgrade, QUAKE, &p, false), LoadoutResult::Ok);
        assert_eq!(l.apply_skill(SkillAction::Upgrade, QUAKE, &p, false), LoadoutResult::Ok);
        assert_eq!(l.apply_skill(SkillAction::Upgrade, QUAKE, &p, false), LoadoutResult::MaxRank);
        assert_eq!(l.apply_skill(SkillAction::Awaken, QUAKE, &p, false), LoadoutResult::Ok);
        assert_eq!(l.apply_skill(SkillAction::Awaken, QUAKE, &p, false), LoadoutResult::MaxRank);
        assert_eq!((l.skill_points_left(&p), l.awakening_points_left(&p)), (9, 0));
        assert_eq!(l.apply_skill(SkillAction::Remove, QUAKE, &p, true), LoadoutResult::InCombat);
        assert_eq!(l.apply_skill(SkillAction::Remove, QUAKE, &p, false), LoadoutResult::Ok);
        assert_eq!((l.skill_points_left(&p), l.awakening_points_left(&p)), (12, 1));
    }

    #[test]
    fn advanced_needs_level_12_and_stats_cannot_overspend() {
        let mut l = Loadout::default();
        let p = at_level(11); // 스탯 30
        assert_eq!(l.apply_stat(0, 20, &p), LoadoutResult::Ok);
        assert_eq!(l.apply_skill(SkillAction::Learn, QUAKE, &p, false), LoadoutResult::Locked); // 12레벨 전
        assert!(l.unlocked(QUAKE, &at_level(12)));
        assert_eq!(l.apply_stat(1, 11, &p), LoadoutResult::NoPoints);
        assert_eq!(l.apply_stat(1, 10, &p), LoadoutResult::Ok);
        assert_eq!((l.apply_stat(4, 1, &p), l.apply_stat(1, 0, &p)), (LoadoutResult::Invalid, LoadoutResult::Invalid));
    }
}
