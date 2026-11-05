// AscensionMovement.h
// 서버(GCC)·봇·언리얼 클라이언트(MSVC)가 함께 쓰는 이동 계산. 같은 입력이면 같은 결과가 나와야 한다.
//
// - 서버는 클라이언트가 보낸 입력(C_Input)으로 이 코드를 돌려 위치를 정한다(서버 권한).
// - 클라이언트는 같은 코드로 미리 움직이고(예측), 서버 확정 상태(S_MoveAck)를 받으면 그 뒤 입력을 다시 돌려 맞춘다(재조정).
// - 60Hz 고정 틱. 프레임 길이와 무관하게 입력 하나 = 1틱이다.
// - 지형은 서버용 충돌 파일(data/CombatLab.txt)을 읽은 2D 볼록 다각형이다. 캐릭터는 반지름 32cm 원.
//
// 규칙: 프로토콜 파일과 같이 C++20 범위만 쓰고, 서버 원본을 고치면 클라이언트 복사본도 같이 갱신한다.
#pragma once

#include "AscensionProtocol.h"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <sstream>
#include <string>
#include <vector>

namespace asc::move
{
    inline constexpr int kTickRate = 60;
    inline constexpr float kTick = 1.f / kTickRate;

    // 1인 데모(ACLHero)와 같은 값: 달리기 450cm/s, UE 기본 가속·제동 2048cm/s²
    inline constexpr float kRunSpeed = 450.f;
    inline constexpr float kAcceleration = 2048.f;
    inline constexpr float kBraking = 2048.f;
    inline constexpr float kRadius = 32.f;     // 주인공 캡슐 반지름
    inline constexpr float kHalfHeight = 88.f; // 캡슐 반높이
    inline constexpr float kGroundZ = 90.f;    // z=0 바닥에 선 캡슐 중심 높이

    // 회피: 0.65초(39틱) 동안 300cm. 거리 곡선은 1인 데모와 같다(천천히 시작·끝).
    inline constexpr int kDodgeTicks = 39;
    inline constexpr float kDodgeDistance = 300.f;
    // 재사용 대기 1초. 클라이언트 실제 시간 검사(1.0초)가 항상 더 엄격하도록 2틱 짧게 둔다.
    inline constexpr int kDodgeCooldownTicks = kTickRate - 2;

    inline constexpr std::uint8_t kInputDodge = 1;

    struct Vec2
    {
        float x = 0.f, y = 0.f;
    };

    struct Obstacle
    {
        std::string name;
        bool blocking = false; // 윗면이 계단 높이(45cm)보다 높다. 공격 엄폐 판정용
        float zMin = 0.f, zMax = 0.f;
        std::vector<Vec2> polygon; // 반시계 볼록 다각형(cm)
    };

    struct World
    {
        std::vector<Obstacle> obstacles;
    };

    struct Input
    {
        float moveX = 0.f, moveY = 0.f; // 월드 좌표 이동 방향, 길이 1 이하
        std::uint8_t flags = 0;
        float dodgeX = 0.f, dodgeY = 0.f; // 회피 방향(flags에 kInputDodge일 때)
    };

    struct State
    {
        float x = 0.f, y = 0.f, vx = 0.f, vy = 0.f;
        std::uint16_t dodgeTick = 0; // 0 = 회피 아님, 1~kDodgeTicks = 다음에 진행할 회피 틱
        std::uint16_t cooldown = 0;  // 다음 회피까지 남은 틱
        float dodgeX = 0.f, dodgeY = 0.f;
    };

    inline proto::MoveSimState ToProto(const State& s)
    {
        return {s.x, s.y, s.vx, s.vy, s.dodgeTick, s.cooldown, s.dodgeX, s.dodgeY};
    }

    inline State FromProto(const proto::MoveSimState& p)
    {
        return {p.x, p.y, p.vx, p.vy, p.dodgeTick, p.cooldown, p.dodgeX, p.dodgeY};
    }

    // ---- 지형 ----

    // 서버용 충돌 파일 내용을 읽는다. 실패하면 false와 이유.
    inline bool ParseWorld(const std::string& text, World& out, std::string& error)
    {
        out.obstacles.clear();
        std::istringstream lines(text);
        std::string line;
        int version = 0, lineNumber = 0;
        while (std::getline(lines, line))
        {
            ++lineNumber;
            if (line.empty() || line[0] == '#') continue;
            std::istringstream in(line);
            std::string kind;
            in >> kind;
            if (kind == "version") in >> version;
            if (kind != "obstacle") continue;
            Obstacle o;
            int blocking = 0;
            std::size_t count = 0;
            in >> o.name >> blocking >> o.zMin >> o.zMax >> count;
            o.blocking = blocking != 0;
            if (!in || count < 3 || count > 64)
            {
                error = "obstacle line " + std::to_string(lineNumber);
                return false;
            }
            o.polygon.resize(count);
            for (Vec2& v : o.polygon) in >> v.x >> v.y;
            if (!in)
            {
                error = "obstacle vertices line " + std::to_string(lineNumber);
                return false;
            }
            out.obstacles.push_back(std::move(o));
        }
        if (version != 1 || out.obstacles.empty())
        {
            error = "version " + std::to_string(version) + " or no obstacles";
            return false;
        }
        return true;
    }

    // 점에서 볼록 다각형 경계까지 거리(안쪽이면 음수). outward가 있으면 밀어낼 방향(바깥쪽 단위 벡터).
    inline float SignedDistance(Vec2 p, const std::vector<Vec2>& polygon, Vec2* outward = nullptr)
    {
        bool inside = true;
        float best = 1e30f, insideBest = 1e30f;
        Vec2 bestPoint{}, insideNormal{};
        const std::size_t n = polygon.size();
        for (std::size_t i = 0; i < n; ++i)
        {
            const Vec2 a = polygon[i], b = polygon[(i + 1) % n];
            const float ex = b.x - a.x, ey = b.y - a.y, px = p.x - a.x, py = p.y - a.y;
            const float cross = ex * py - ey * px;
            const float length = std::sqrt(ex * ex + ey * ey);
            if (cross < 0.f) inside = false;
            // 안쪽에 있을 때 가장 가까운 변의 바깥 법선(반시계 다각형: (ey, -ex))
            const float edgeDistance = cross / length;
            if (edgeDistance < insideBest)
            {
                insideBest = edgeDistance;
                insideNormal = {ey / length, -ex / length};
            }
            const float t = std::clamp((px * ex + py * ey) / (length * length), 0.f, 1.f);
            const float qx = a.x + t * ex, qy = a.y + t * ey;
            const float d = std::sqrt((p.x - qx) * (p.x - qx) + (p.y - qy) * (p.y - qy));
            if (d < best)
            {
                best = d;
                bestPoint = {qx, qy};
            }
        }
        if (inside)
        {
            if (outward) *outward = insideNormal;
            return -insideBest;
        }
        if (outward && best > 1e-6f) *outward = {(p.x - bestPoint.x) / best, (p.y - bestPoint.y) / best};
        return best;
    }

    // 이동에서 단단한 장애물: 바닥보다 위로 솟았고 캡슐 높이 범위와 겹치는 것.
    // 2D 이동이라 계단 오르기가 없으므로 낮은 받침(40cm)도 막는다(온라인에서는 받침 둘레로 돈다).
    inline bool Solid(const Obstacle& o)
    {
        return o.zMax > 5.f && o.zMin < kGroundZ + kHalfHeight;
    }

    // 원(반지름 kRadius)을 장애물 밖으로 밀어내고, 벽을 향하는 속도 성분을 없앤다(벽 따라 미끄러짐).
    inline void ResolveCollisions(State& s, const World& world)
    {
        for (int iteration = 0; iteration < 4; ++iteration)
        {
            bool moved = false;
            for (const Obstacle& o : world.obstacles)
            {
                if (!Solid(o)) continue;
                Vec2 normal{};
                const float d = SignedDistance({s.x, s.y}, o.polygon, &normal);
                if (d >= kRadius) continue;
                const float push = kRadius - d;
                s.x += normal.x * push;
                s.y += normal.y * push;
                const float into = s.vx * normal.x + s.vy * normal.y;
                if (into < 0.f)
                {
                    s.vx -= normal.x * into;
                    s.vy -= normal.y * into;
                }
                moved = true;
            }
            if (!moved) break;
        }
    }

    inline float DodgeCurve(float a)
    {
        constexpr float twoPi = 6.28318530717958647692f;
        return kDodgeDistance * (a - std::sin(twoPi * a) / twoPi);
    }

    // 1틱 진행.
    inline void Step(State& s, const Input& input, const World& world)
    {
        if (s.cooldown > 0) --s.cooldown;

        if ((input.flags & kInputDodge) && s.dodgeTick == 0 && s.cooldown == 0)
        {
            const float length = std::sqrt(input.dodgeX * input.dodgeX + input.dodgeY * input.dodgeY);
            if (length > 1e-3f && std::isfinite(length))
            {
                s.dodgeX = input.dodgeX / length;
                s.dodgeY = input.dodgeY / length;
                s.dodgeTick = 1;
                s.cooldown = kDodgeCooldownTicks;
                s.vx = s.vy = 0.f; // 1인 데모처럼 회피 시작 때 달리기 속도를 버린다
            }
        }

        if (s.dodgeTick > 0)
        {
            const float a0 = static_cast<float>(s.dodgeTick - 1) / kDodgeTicks, a1 = static_cast<float>(s.dodgeTick) / kDodgeTicks;
            const float step = DodgeCurve(a1) - DodgeCurve(a0);
            s.x += s.dodgeX * step;
            s.y += s.dodgeY * step;
            s.dodgeTick = s.dodgeTick >= kDodgeTicks ? 0 : static_cast<std::uint16_t>(s.dodgeTick + 1);
        }
        else
        {
            float mx = std::isfinite(input.moveX) ? input.moveX : 0.f, my = std::isfinite(input.moveY) ? input.moveY : 0.f;
            const float length = std::sqrt(mx * mx + my * my);
            if (length > 1.f)
            {
                mx /= length;
                my /= length;
            }
            if (length > 1e-3f)
            {
                // 목표 속도로 가속(최대 kAcceleration)
                const float dx = mx * kRunSpeed - s.vx, dy = my * kRunSpeed - s.vy;
                const float dv = std::sqrt(dx * dx + dy * dy), maxDv = kAcceleration * kTick;
                const float scale = dv > maxDv ? maxDv / dv : 1.f;
                s.vx += dx * scale;
                s.vy += dy * scale;
            }
            else
            {
                // 입력이 없으면 제동
                const float speed = std::sqrt(s.vx * s.vx + s.vy * s.vy), slowed = std::max(0.f, speed - kBraking * kTick);
                const float scale = speed > 1e-6f ? slowed / speed : 0.f;
                s.vx *= scale;
                s.vy *= scale;
            }
            s.x += s.vx * kTick;
            s.y += s.vy * kTick;
        }
        ResolveCollisions(s, world);
    }
}
