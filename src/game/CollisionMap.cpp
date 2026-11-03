#include "game/CollisionMap.h"

#include <algorithm>
#include <cmath>
#include <fstream>
#include <format>
#include <limits>
#include <sstream>

namespace asc::game
{
    namespace
    {
        Vec2 Flat(const proto::Vec3& v) { return {v.x, v.y}; }
        float Cross(Vec2 a, Vec2 b) { return a.x * b.y - a.y * b.x; }
        Vec2 Sub(Vec2 a, Vec2 b) { return {a.x - b.x, a.y - b.y}; }
    }

    std::optional<CollisionMap> CollisionMap::Load(const std::string& path, std::string& error)
    {
        std::ifstream file(path);
        if (!file)
        {
            error = std::format("{} 파일을 열 수 없음", path);
            return std::nullopt;
        }

        CollisionMap map;
        int version = 0;
        std::string line;
        for (int lineNumber = 1; std::getline(file, line); ++lineNumber)
        {
            if (line.empty() || line[0] == '#') continue;
            std::istringstream in(line);
            std::string kind;
            in >> kind;
            if (kind == "version") in >> version;
            else if (kind == "step_height") in >> map.stepHeight_;
            else if (kind == "obstacle")
            {
                Obstacle obstacle;
                int blocking = 0;
                std::size_t count = 0;
                in >> obstacle.name >> blocking >> obstacle.zMin >> obstacle.zMax >> count;
                obstacle.blocking = blocking != 0;
                obstacle.polygon.resize(count);
                for (Vec2& v : obstacle.polygon) in >> v.x >> v.y;
                // 반시계 볼록 다각형인지 확인한다(판정 코드가 이 가정을 쓴다).
                bool convex = count >= 3;
                for (std::size_t i = 0; convex && i < count; ++i)
                {
                    const Vec2 a = obstacle.polygon[i], b = obstacle.polygon[(i + 1) % count], c = obstacle.polygon[(i + 2) % count];
                    convex = Cross(Sub(b, a), Sub(c, b)) > -1e-3f;
                }
                if (!in || !convex)
                {
                    error = std::format("{}:{} 장애물 형식 오류 또는 반시계 볼록 다각형이 아님", path, lineNumber);
                    return std::nullopt;
                }
                map.obstacles_.push_back(std::move(obstacle));
            }
        }
        if (version != 1 || map.obstacles_.empty())
        {
            error = std::format("{}: 버전 {} 또는 장애물 없음", path, version);
            return std::nullopt;
        }
        return map;
    }

    bool CollisionMap::Relevant(const Obstacle& obstacle, float centerZ) const
    {
        // 오를 수 없고, 캡슐 높이 범위와 겹치는 장애물만 막는다(머리 위 상인방·제단 덮개는 통과).
        return obstacle.blocking && obstacle.zMin < centerZ + kCharacterHalfHeight && obstacle.zMax > centerZ - kCharacterHalfHeight;
    }

    float CollisionMap::SignedDistance(Vec2 p, const std::vector<Vec2>& polygon)
    {
        bool inside = true;
        float best = std::numeric_limits<float>::max();
        for (std::size_t i = 0; i < polygon.size(); ++i)
        {
            const Vec2 a = polygon[i], e = Sub(polygon[(i + 1) % polygon.size()], a), ap = Sub(p, a);
            if (Cross(e, ap) < 0.f) inside = false;
            const float t = std::clamp((ap.x * e.x + ap.y * e.y) / (e.x * e.x + e.y * e.y), 0.f, 1.f);
            best = std::min(best, std::hypot(ap.x - t * e.x, ap.y - t * e.y));
        }
        return inside ? -best : best;
    }

    bool CollisionMap::SegmentHitsPolygon(Vec2 a, Vec2 b, const std::vector<Vec2>& polygon)
    {
        if (SignedDistance(a, polygon) <= 0.f || SignedDistance(b, polygon) <= 0.f) return true;
        const Vec2 d = Sub(b, a);
        for (std::size_t i = 0; i < polygon.size(); ++i)
        {
            const Vec2 p = polygon[i], e = Sub(polygon[(i + 1) % polygon.size()], p);
            const float denominator = Cross(d, e);
            if (std::abs(denominator) < 1e-6f) continue; // 평행
            const Vec2 ap = Sub(p, a);
            const float t = Cross(ap, e) / denominator, u = Cross(ap, d) / denominator;
            if (t >= 0.f && t <= 1.f && u >= 0.f && u <= 1.f) return true;
        }
        return false;
    }

    bool CollisionMap::Blocked(const proto::Vec3& center, float radius) const
    {
        const Vec2 p = Flat(center);
        return std::ranges::any_of(obstacles_, [&](const Obstacle& o) { return Relevant(o, center.z) && SignedDistance(p, o.polygon) < radius; });
    }

    bool CollisionMap::SweepBlocked(const proto::Vec3& from, const proto::Vec3& to, float radius) const
    {
        // 10cm 간격으로 원을 놓아 본다. 반지름보다 촘촘하므로 얇은 벽도 건너뛰지 못한다.
        const float distance = std::hypot(to.x - from.x, to.y - from.y);
        const int steps = std::max(1, static_cast<int>(std::ceil(distance / 10.f)));
        for (int i = 1; i <= steps; ++i)
        {
            const float t = static_cast<float>(i) / static_cast<float>(steps);
            if (Blocked({from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t, from.z + (to.z - from.z) * t}, radius)) return true;
        }
        return false;
    }

    bool CollisionMap::LineBlocked(const proto::Vec3& from, const proto::Vec3& to) const
    {
        const float z = std::max(from.z, to.z);
        return std::ranges::any_of(obstacles_, [&](const Obstacle& o) { return Relevant(o, z) && SegmentHitsPolygon(Flat(from), Flat(to), o.polygon); });
    }
}
