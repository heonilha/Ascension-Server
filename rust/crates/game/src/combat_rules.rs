//! 방 모드별 전투 판정 규칙과 지연 보정 계산.
//!
//! 지연 보정 방식(P1)
//! - 공격은 서버가 명중 시각을 정한다(`SAttackStart`). 클라이언트는 그 시각에 맞춰 예고를 재생하므로
//!   플레이어가 보는 명중 순간이 서버 명중 시각과 같다. 남는 지연은 회피 입력이 서버에 가는 시간뿐이다.
//! - 회피 입력에는 누른 시각(서버 시계 추정값)을 붙인다. 서버는 그 시각을
//!   `[도착 시각 - 허용 되감기, 도착 시각]`으로 제한해서 인정한다.
//! - 명중 판정은 "명중 시각 + 허용 되감기"까지 미뤘다가 확정한다(판정 유예).
//!   그 안에 도착한 회피는 모두 반영되고, HP를 깎았다 되돌리는 일이 없다.

use asc_protocol::{ModeRules, RoomMode, MAX_MATCH_PLAYERS, MAX_PRACTICE_PLAYERS};

/// 연습 방: 무적 0.15초는 1인 데모의 손맛 기준값 그대로다. 지연이 큰 사람에게 관대하게 200ms까지 되감는다.
pub const PRACTICE_RULES: ModeRules = ModeRules { iframe_ms: 150, rewind_cap_ms: 200, dodge_cooldown_ms: 1000, attack_windup_ms: 0 };
/// 매치(PvP): 되감기 상한을 100ms로 줄여 시각 조작으로 얻는 이득을 제한한다. 대신 무적을 0.30초로 늘려
/// 왕복 지연 100ms가 무적 시간의 1/3을 넘지 않게 한다. 공격 준비 0.35초는 상대가 예고를 보고 반응할 시간이다.
pub const MATCH_RULES: ModeRules = ModeRules { iframe_ms: 300, rewind_cap_ms: 100, dodge_cooldown_ms: 1000, attack_windup_ms: 350 };

/// 왕복 시간의 절반에 더하는 여유(지연 흔들림).
pub const JITTER_MARGIN_MS: u32 = 30;
/// 재사용 대기 검사 여유. 클라이언트 시계 추정 오차로 정상 입력이 거부되지 않게 한다.
pub const COOLDOWN_SLACK_MS: u32 = 50;

pub fn rules_for(mode: RoomMode) -> ModeRules {
    match mode {
        RoomMode::Match => MATCH_RULES,
        RoomMode::Practice => PRACTICE_RULES,
    }
}

pub fn capacity_for(mode: RoomMode) -> u8 {
    match mode {
        RoomMode::Match => MAX_MATCH_PLAYERS,
        RoomMode::Practice => MAX_PRACTICE_PLAYERS,
    }
}

/// 회피 입력을 도착 시각보다 얼마나 과거로 인정할지. 왕복 시간을 아직 모르면(접속 직후 1초) 상한을 쓴다.
/// 왕복 시간은 클라이언트가 알려 주지만, 부풀려도 상한을 넘을 수 없다.
pub fn allowed_rewind_ms(rules: &ModeRules, rtt_ms: u32) -> u32 {
    let cap = rules.rewind_cap_ms as u32;
    if rtt_ms == 0 {
        return cap;
    }
    (rtt_ms / 2 + JITTER_MARGIN_MS).min(cap)
}

/// 클라이언트가 보낸 누른 시각을 인정 가능한 범위로 자른다. 미래 시각과 너무 먼 과거를 모두 막는다.
pub fn clamp_press_time(press_ms: u32, arrival_ms: u32, allowed_rewind_ms: u32) -> u32 {
    press_ms.clamp(arrival_ms.saturating_sub(allowed_rewind_ms), arrival_ms)
}

/// 회피 시작 시각 `dodge_ms`의 무적 구간에 명중 시각이 들어가는가.
pub fn in_iframe(rules: &ModeRules, dodge_ms: u32, hit_ms: u32) -> bool {
    dodge_ms <= hit_ms && hit_ms <= dodge_ms + rules.iframe_ms as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_and_capacity_by_mode() {
        assert_eq!(rules_for(RoomMode::Practice), PRACTICE_RULES);
        assert_eq!(rules_for(RoomMode::Match), MATCH_RULES);
        assert_eq!(capacity_for(RoomMode::Practice), 3);
        assert_eq!(capacity_for(RoomMode::Match), 12);
    }

    #[test]
    fn rewind_is_half_rtt_plus_margin_up_to_cap() {
        assert_eq!(allowed_rewind_ms(&PRACTICE_RULES, 0), 200); // 아직 모름 → 상한
        assert_eq!(allowed_rewind_ms(&PRACTICE_RULES, 1), 30);
        assert_eq!(allowed_rewind_ms(&PRACTICE_RULES, 100), 80);
        assert_eq!(allowed_rewind_ms(&PRACTICE_RULES, 340), 200);
        assert_eq!(allowed_rewind_ms(&PRACTICE_RULES, 2000), 200); // 부풀려도 상한
        assert_eq!(allowed_rewind_ms(&MATCH_RULES, 100), 80);
        assert_eq!(allowed_rewind_ms(&MATCH_RULES, 2000), 100);
        assert_eq!(allowed_rewind_ms(&MATCH_RULES, u32::MAX), 100);
    }

    #[test]
    fn press_time_is_clamped_to_window() {
        assert_eq!(clamp_press_time(950, 1000, 80), 950); // 범위 안
        assert_eq!(clamp_press_time(500, 1000, 80), 920); // 너무 먼 과거
        assert_eq!(clamp_press_time(1500, 1000, 80), 1000); // 미래
        assert_eq!(clamp_press_time(0, 50, 80), 0); // 서버 시작 직후: 0 아래로 내려가지 않는다
    }

    #[test]
    fn iframe_window_is_inclusive() {
        assert!(in_iframe(&PRACTICE_RULES, 1000, 1000));
        assert!(in_iframe(&PRACTICE_RULES, 1000, 1150));
        assert!(!in_iframe(&PRACTICE_RULES, 1000, 1151));
        assert!(!in_iframe(&PRACTICE_RULES, 1000, 999)); // 명중 뒤에 누른 회피
        assert!(in_iframe(&MATCH_RULES, 1000, 1300));
    }
}
