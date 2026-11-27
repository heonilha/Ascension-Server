//! 배틀로얄 매치의 시간표: 라운드·자기장 축소·자기장 피해·부활 대기. 네트워크를 모르는 순수 계산이다.
//!
//! 기준: Ascension-Team `Docs/11.26기획안.md` "라운드와 자기장".
//! 한 판 = 1라운드 15분 → 축소 1분 → 2라운드 10분 → 축소 1분 → 3라운드 5분 → 축소 1분 → 4라운드 3분(중앙 한 점까지 계속 축소).
//! 자기장 밖 피해는 초당 최대 체력의 1% / 2% / 4% / 8%. 부활 대기 10 / 20 / 30초, 4라운드는 사망 = 탈락.
//! 데모·시험에서는 `MatchSchedule::scaled`로 모든 시간을 같은 비율로 줄인다.

use asc_protocol::MatchPhase;

/// 라운드 n(1~4)이 시작할 때의 자기장 반지름(cm). 1라운드는 맵 전체(2800 × 2400 직사각형의 대각선 절반 ≈ 3690)를 덮는다.
/// 라운드 n이 끝나면 축소 단계 동안 다음 값으로 줄고, 4라운드는 진행 내내 0까지 준다.
pub const ZONE_RADIUS: [f32; 4] = [3700.0, 2400.0, 1500.0, 800.0];
/// 자기장 중심. 4라운드에 이 한 점으로 모인다(데모 맵 중앙).
pub const ZONE_CENTER: (f32, f32) = (0.0, 0.0);
/// 라운드 n의 자기장 밖 피해: 초당 최대 체력 대비 퍼센트. 라운드 n 뒤 축소 단계도 같은 값을 쓴다.
pub const ZONE_DAMAGE_PERCENT: [f32; 4] = [1.0, 2.0, 4.0, 8.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchSchedule {
    /// 라운드 1~4의 길이
    pub round_ms: [u32; 4],
    /// 라운드 사이 자기장 축소 시간
    pub shrink_ms: u32,
    /// 팀이 모인 뒤 1라운드까지
    pub countdown_ms: u32,
    /// 결과 표시 뒤 대기로 돌아갈 때까지
    pub ended_ms: u32,
    /// 라운드 1~3에서 죽었을 때 부활 대기
    pub respawn_ms: [u32; 3],
    /// 시작 전(대기·카운트다운) 부활 대기
    pub pregame_respawn_ms: u32,
    /// 이 수 이상의 팀이 들어오면 카운트다운을 시작한다
    pub start_teams: u8,
}

impl Default for MatchSchedule {
    fn default() -> Self {
        const MINUTE: u32 = 60_000;
        MatchSchedule {
            round_ms: [15 * MINUTE, 10 * MINUTE, 5 * MINUTE, 3 * MINUTE],
            shrink_ms: MINUTE,
            countdown_ms: 10_000,
            ended_ms: 15_000,
            respawn_ms: [10_000, 20_000, 30_000],
            pregame_respawn_ms: 3_000,
            start_teams: 2,
        }
    }
}

impl MatchSchedule {
    /// 기획 시간표의 모든 시간에 `scale`을 곱한다(0.1이면 36분 → 3.6분). 각 시간은 최소 200ms.
    pub fn scaled(scale: f32) -> MatchSchedule {
        let base = MatchSchedule::default();
        let s = |ms: u32| ((ms as f64 * scale.max(0.0) as f64).round() as u32).max(200);
        MatchSchedule {
            round_ms: base.round_ms.map(s),
            shrink_ms: s(base.shrink_ms),
            countdown_ms: s(base.countdown_ms),
            ended_ms: s(base.ended_ms),
            respawn_ms: base.respawn_ms.map(s),
            pregame_respawn_ms: s(base.pregame_respawn_ms),
            start_teams: base.start_teams,
        }
    }

    /// 1라운드 시작부터 4라운드 끝까지
    pub fn total_ms(&self) -> u32 {
        self.round_ms.iter().sum::<u32>() + 3 * self.shrink_ms
    }
}

/// 진행 중(1~4라운드) 한 시점의 상태
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunningPhase {
    pub phase: MatchPhase,
    /// 1~4
    pub round: u8,
    /// 1라운드 시작 기준 이 단계의 시작·끝(ms)
    pub start_offset_ms: u32,
    pub end_offset_ms: u32,
    pub radius_from: f32,
    pub radius_to: f32,
    pub damage_percent: f32,
    /// 지금 죽으면 이만큼 기다렸다 부활한다. `None` = 탈락
    pub respawn_wait_ms: Option<u32>,
}

impl RunningPhase {
    /// `elapsed_ms`(1라운드 시작 기준) 시점의 자기장 반지름
    pub fn radius_at(&self, elapsed_ms: u32) -> f32 {
        let length = self.end_offset_ms.saturating_sub(self.start_offset_ms);
        if length == 0 || self.radius_from == self.radius_to {
            return self.radius_to;
        }
        let t = (elapsed_ms.saturating_sub(self.start_offset_ms) as f32 / length as f32).clamp(0.0, 1.0);
        self.radius_from + (self.radius_to - self.radius_from) * t
    }
}

/// 1라운드 시작 뒤 `elapsed_ms` 시점의 단계. 4라운드가 끝난 뒤에도 4라운드(반지름 0)로 남는다.
pub fn running_phase(schedule: &MatchSchedule, elapsed_ms: u32) -> RunningPhase {
    let mut start = 0u32;
    for round in 0..4usize {
        let round_end = start + schedule.round_ms[round];
        let last = round == 3;
        if elapsed_ms < round_end || last {
            return RunningPhase {
                phase: MatchPhase::Round,
                round: round as u8 + 1,
                start_offset_ms: start,
                end_offset_ms: round_end,
                radius_from: ZONE_RADIUS[round],
                // 1~3라운드는 고정, 4라운드는 중앙 한 점까지 계속 준다
                radius_to: if last { 0.0 } else { ZONE_RADIUS[round] },
                damage_percent: ZONE_DAMAGE_PERCENT[round],
                respawn_wait_ms: (!last).then(|| schedule.respawn_ms[round]),
            };
        }
        let shrink_end = round_end + schedule.shrink_ms;
        if elapsed_ms < shrink_end {
            return RunningPhase {
                phase: MatchPhase::Shrink,
                round: round as u8 + 1,
                start_offset_ms: round_end,
                end_offset_ms: shrink_end,
                radius_from: ZONE_RADIUS[round],
                radius_to: ZONE_RADIUS[round + 1],
                damage_percent: ZONE_DAMAGE_PERCENT[round],
                // 3라운드 뒤 축소 중 사망도 3라운드 사망으로 친다(마지막 부활 기회)
                respawn_wait_ms: Some(schedule.respawn_ms[round]),
            };
        }
        start = shrink_end;
    }
    unreachable!("4라운드에서 반환한다")
}

/// 점이 자기장 밖인가
pub fn outside_zone(x: f32, y: f32, radius: f32) -> bool {
    (x - ZONE_CENTER.0).hypot(y - ZONE_CENTER.1) > radius
}

/// 최후의 1팀 판정. `alive_teams`는 탈락하지 않은 팀 목록(중복 없음), `last_death_team`은 가장 마지막에 죽은 사람의 팀.
/// 남은 팀이 1팀 이하가 되면 `Some(이긴 팀)`: 1팀이면 그 팀, 0팀(동시 전멸)이면 마지막에 죽은 사람의 팀(없으면 0).
pub fn winner(alive_teams: &[u8], last_death_team: Option<u8>) -> Option<u8> {
    match alive_teams {
        [] => Some(last_death_team.unwrap_or(0)),
        [only] => Some(*only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_schedule_is_36_minutes() {
        let s = MatchSchedule::default();
        assert_eq!(s.total_ms(), 36 * 60_000);
    }

    #[test]
    fn phases_follow_the_plan() {
        let s = MatchSchedule::default();
        let min = 60_000;
        let p = running_phase(&s, 0);
        assert_eq!(
            (p.phase, p.round, p.radius_from, p.radius_to, p.damage_percent, p.respawn_wait_ms),
            (MatchPhase::Round, 1, 3700.0, 3700.0, 1.0, Some(10_000))
        );
        let p = running_phase(&s, 15 * min);
        assert_eq!((p.phase, p.round, p.radius_from, p.radius_to), (MatchPhase::Shrink, 1, 3700.0, 2400.0));
        assert_eq!(p.radius_at(15 * min + 30_000), 3050.0); // 축소 절반
        let p = running_phase(&s, 16 * min);
        assert_eq!((p.phase, p.round, p.damage_percent, p.respawn_wait_ms), (MatchPhase::Round, 2, 2.0, Some(20_000)));
        let p = running_phase(&s, 27 * min + 1);
        assert_eq!((p.phase, p.round), (MatchPhase::Round, 3));
        let p = running_phase(&s, 32 * min + 1); // 3라운드 뒤 축소: 아직 부활 가능
        assert_eq!((p.phase, p.round, p.respawn_wait_ms), (MatchPhase::Shrink, 3, Some(30_000)));
        let p = running_phase(&s, 33 * min);
        assert_eq!((p.phase, p.round, p.radius_from, p.radius_to, p.damage_percent, p.respawn_wait_ms), (MatchPhase::Round, 4, 800.0, 0.0, 8.0, None));
        assert_eq!(p.radius_at(33 * min + 90_000), 400.0);
        // 4라운드가 끝난 뒤에도 4라운드, 반지름 0
        let p = running_phase(&s, 50 * min);
        assert_eq!((p.round, p.radius_at(50 * min)), (4, 0.0));
    }

    #[test]
    fn scaled_schedule_keeps_ratios() {
        let s = MatchSchedule::scaled(0.01);
        assert_eq!(s.round_ms, [9_000, 6_000, 3_000, 1_800]);
        assert_eq!((s.shrink_ms, s.respawn_ms), (600, [200, 200, 300]));
        assert_eq!(s.total_ms(), 21_600);
    }

    #[test]
    fn zone_and_winner() {
        assert!(!outside_zone(300.0, 400.0, 500.0));
        assert!(outside_zone(300.0, 400.1, 500.0));
        assert_eq!(winner(&[1, 3], Some(2)), None);
        assert_eq!(winner(&[3], Some(2)), Some(3));
        assert_eq!(winner(&[], Some(2)), Some(2)); // 동시 전멸: 마지막에 죽은 사람의 팀
        assert_eq!(winner(&[], None), Some(0));
    }
}
