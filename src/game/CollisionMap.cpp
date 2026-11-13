#include "game/CollisionMap.h"

#include <algorithm>
#include <cmath>
#include <format>
#include <fstream>
#include <sstream>

namespace asc::game
{
    namespace
    {
        using move::Vec2;

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
        std::stringstream text;
        text << file.rdbuf();

        CollisionMap map;
        std::string parseError;
        if (!move::ParseWorld(text.str(), map.world_, parseError))
        {
            error = std::format("{}: {}", path, parseError);
            return std::nullopt;
        }
        // 반시계 볼록 다각형인지 확인한다(판정 코드가 이 가정을 쓴다).
        for (const move::Obstacle& o : map.world_.obstacles)
        {
            const std::size_t n = o.polygon.size();
            for (std::size_t i = 0; i < n; ++i)
            {
                if (Cross(Sub(o.polygon[(i + 1) % n], o.polygon[i]), Sub(o.polygon[(i + 2) % n], o.polygon[(i + 1) % n])) < -1e-3f)
                {
                    error = std::format("{}: {}이 반시계 볼록 다각형이 아님", path, o.name);
                    return std::nullopt;
                }
            }
        }
        for (const move::Obstacle& o : map.world_.obstacles)
        {
            Bounds b{1e30f, 1e30f, -1e30f, -1e30f};
            for (const Vec2 v : o.polygon)
            {
                b.minX = std::min(b.minX, v.x), b.minY = std::min(b.minY, v.y);
                b.maxX = std::max(b.maxX, v.x), b.maxY = std::max(b.maxY, v.y);
            }
            map.bounds_.push_back(b);
        }
        return map;
    }

    bool CollisionMap::SolidOverlap(Vec2 p, float radius) const
    {
        for (std::size_t i = 0; i < world_.obstacles.size(); ++i)
        {
            const Bounds& b = bounds_[i];
            if (p.x < b.minX - radius || p.x > b.maxX + radius || p.y < b.minY - radius || p.y > b.maxY + radius) continue;
            if (move::Solid(world_.obstacles[i]) && move::SignedDistance(p, world_.obstacles[i].polygon) < radius) return true;
        }
        return false;
    }

    bool CollisionMap::PushOutSolid(Vec2& p, float radius) const
    {
        bool moved = false;
        for (int iteration = 0; iteration < 4; ++iteration)
        {
            bool pushed = false;
            for (std::size_t i = 0; i < world_.obstacles.size(); ++i)
            {
                const Bounds& b = bounds_[i];
                if (p.x < b.minX - radius || p.x > b.maxX + radius || p.y < b.minY - radius || p.y > b.maxY + radius) continue;
                if (!move::Solid(world_.obstacles[i])) continue;
                Vec2 normal{};
                const float d = move::SignedDistance(p, world_.obstacles[i].polygon, &normal);
                if (d >= radius) continue;
                p.x += normal.x * (radius - d);
                p.y += normal.y * (radius - d);
                pushed = moved = true;
            }
            if (!pushed) break;
        }
        return moved;
    }

    bool CollisionMap::Relevant(const move::Obstacle& obstacle, float centerZ) const
    {
        // 오를 수 없고, 캡슐 높이 범위와 겹치는 장애물만 막는다(머리 위 상인방·제단 덮개는 통과).
        return obstacle.blocking && obstacle.zMin < centerZ + move::kHalfHeight && obstacle.zMax > centerZ - move::kHalfHeight;
    }

    bool CollisionMap::SegmentHitsPolygon(Vec2 a, Vec2 b, const std::vector<Vec2>& polygon)
    {
        if (move::SignedDistance(a, polygon) <= 0.f || move::SignedDistance(b, polygon) <= 0.f) return true;
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
        return std::ranges::any_of(world_.obstacles,
                                   [&](const move::Obstacle& o) { return Relevant(o, center.z) && move::SignedDistance(p, o.polygon) < radius; });
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
        return std::ranges::any_of(world_.obstacles,
                                   [&](const move::Obstacle& o) { return Relevant(o, z) && SegmentHitsPolygon(Flat(from), Flat(to), o.polygon); });
    }
}
