// 방 모드별 전투 판정 규칙과 지연 보정 계산.
//
// 지연 보정 방식(P1)
// - 공격은 서버가 명중 시각을 정한다(S_AttackStart). 클라이언트는 그 시각에 맞춰 예고를 재생하므로
//   플레이어가 보는 명중 순간이 서버 명중 시각과 같다. 남는 지연은 회피 입력이 서버에 가는 시간뿐이다.
// - 회피 입력에는 누른 시각(서버 시계 추정값)을 붙인다. 서버는 그 시각을
//   [도착 시각 - 허용 되감기, 도착 시각]으로 제한해서 인정한다.
// - 명중 판정은 "명중 시각 + 허용 되감기"까지 미뤘다가 확정한다(판정 유예).
//   그 안에 도착한 회피는 모두 반영되고, HP를 깎았다 되돌리는 일이 없다.
#pragma once

#include "AscensionProtocol.h"

#include <algorithm>
#include <cstdint>

namespace asc::game
{
    // PvE: 무적 0.15초는 1인 데모의 손맛 기준값 그대로다. 협동이라 지연이 큰 팀원에게 관대하게 200ms까지 되감는다.
    // PvP: 되감기 상한을 100ms로 줄여 시각 조작으로 얻는 이득을 제한한다. 대신 무적을 0.30초로 늘려
    //      왕복 지연 100ms가 무적 시간의 1/3을 넘지 않게 한다. 공격 준비 0.35초는 상대가 예고를 보고 반응할 시간이다.
    inline constexpr proto::ModeRules kPveRules{.iframeMs = 150, .rewindCapMs = 200, .dodgeCooldownMs = 1000, .attackWindupMs = 0};
    inline constexpr proto::ModeRules kPvpRules{.iframeMs = 300, .rewindCapMs = 100, .dodgeCooldownMs = 1000, .attackWindupMs = 350};

    // 왕복 시간의 절반에 더하는 여유(지연 흔들림).
    inline constexpr std::uint32_t kJitterMarginMs = 30;
    // 재사용 대기 검사 여유. 클라이언트 시계 추정 오차로 정상 입력이 거부되지 않게 한다.
    inline constexpr std::uint32_t kCooldownSlackMs = 50;

    inline const proto::ModeRules& RulesFor(proto::RoomMode mode)
    {
        return mode == proto::RoomMode::PvP ? kPvpRules : kPveRules;
    }

    inline std::uint8_t CapacityFor(proto::RoomMode mode)
    {
        return mode == proto::RoomMode::PvP ? proto::kMaxPvpPlayers : proto::kMaxRoomPlayers;
    }

    // 회피 입력을 도착 시각보다 얼마나 과거로 인정할지. 왕복 시간을 아직 모르면(접속 직후 1초) 상한을 쓴다.
    // 왕복 시간은 클라이언트가 알려 주지만, 부풀려도 상한을 넘을 수 없다.
    inline std::uint32_t AllowedRewindMs(const proto::ModeRules& rules, std::uint32_t rttMs)
    {
        if (rttMs == 0) return rules.rewindCapMs;
        return std::min<std::uint32_t>(rttMs / 2 + kJitterMarginMs, rules.rewindCapMs);
    }

    // 클라이언트가 보낸 누른 시각을 인정 가능한 범위로 자른다. 미래 시각과 너무 먼 과거를 모두 막는다.
    inline std::uint32_t ClampPressTime(std::uint32_t pressMs, std::uint32_t arrivalMs, std::uint32_t allowedRewindMs)
    {
        const std::uint32_t earliest = arrivalMs > allowedRewindMs ? arrivalMs - allowedRewindMs : 0;
        return std::clamp(pressMs, earliest, arrivalMs);
    }

    // 회피 시작 시각 dodgeMs의 무적 구간에 명중 시각이 들어가는가.
    inline bool InIframe(const proto::ModeRules& rules, std::uint32_t dodgeMs, std::uint32_t hitMs)
    {
        return dodgeMs <= hitMs && hitMs <= dodgeMs + rules.iframeMs;
    }
}
