// 방 배치·처리량 상수와 시작 위치. Rust 서버 asc-game의 room_layout.rs와 server/room.rs 상수를 C++ 도구(봇·지형 시험)용으로 옮긴 것이다.
// 값을 바꾸면 Rust 쪽도 같이 바꾼다. (C++ 서버 본체는 Rust 서버로 옮긴 뒤 지웠다)
#pragma once

#include "AscensionMovement.h"
#include "AscensionProtocol.h"

#include <algorithm>
#include <cmath>
#include <numbers>

namespace asc::game
{
    struct RoomLayout
    {
        // 필드(500m × 500m)의 이동 범위. 클라이언트 AAscensionMode::ArenaHalfWidth/Depth와 같다.
        static constexpr float kArenaHalfWidth = 25000.f;
        static constexpr float kArenaHalfDepth = 25000.f;
        // 지형 검사 반지름(도착 위치 / 이동 경로)
        static constexpr float kTerrainRadius = 24.f;
        static constexpr float kTerrainSweepRadius = 16.f;
        // 이동 입력 처리량 제한: 0.5초 분량까지 모아 두고, 실제 시간의 1.03배만큼 처리권이 쌓인다
        static constexpr double kMaxInputBacklog = move::kTickRate * 0.5;
        static constexpr double kInputRateTolerance = 1.03;
        // 매치 팀 기지: 팀 1~4가 대각선 네 방향(45·135·225·315도), 반지름 160m 원 위에 선다
        static constexpr float kTeamBaseRadius = 16000.f;
        static constexpr float kTeamMemberSpacing = 150.f;

        // 연습 방 slot번째: 클라이언트 데모의 시작 위치(-750, 0) 근처에 1.5m 간격
        static proto::Vec3 PracticeSpawnPosition(int slot)
        {
            return {-750.f, -150.f + 150.f * static_cast<float>(slot), move::kGroundZ};
        }

        // 매치 팀 team(1~4)의 member번째(0~2)
        static proto::Vec3 TeamSpawnPosition(int team, int member)
        {
            const float index = static_cast<float>(std::clamp(team, 1, static_cast<int>(proto::kMaxTeams)) - 1);
            const float angle = (45.f + 90.f * index) * std::numbers::pi_v<float> / 180.f;
            const float c = std::cos(angle), s = std::sin(angle);
            const float offset = (static_cast<float>(member) - 1.f) * kTeamMemberSpacing;
            return {kTeamBaseRadius * c - offset * s, kTeamBaseRadius * s + offset * c, move::kGroundZ};
        }
    };
}
