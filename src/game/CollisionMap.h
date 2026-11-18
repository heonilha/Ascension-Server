// 서버 충돌 지형: 클라이언트 맵의 막히는 구조물을 2D 볼록 다각형 + 높이 범위로 들고 있는다.
//
// 데이터는 클라이언트 저장소의 Scripts/ExportServerCollision.py가 실제 맵 액터에서 뽑은
// data/CombatLab.txt다(손으로 고치지 않는다). 클라이언트는 같은 파일을 게임 물리와 격자 비교해 검증한다.
//
// 스레드 규칙: 서버 시작 때 한 번 읽고 이후에는 읽기만 하므로 여러 방(strand)에서 동시에 써도 된다.
#pragma once

#include "AscensionProtocol.h"

#include <optional>
#include <string>
#include <vector>

namespace asc::game
{
    struct Vec2
    {
        float x = 0.f, y = 0.f;
    };

    class CollisionMap
    {
    public:
        struct Obstacle
        {
            std::string name;
            bool blocking = false; // 윗면이 계단 높이보다 높다(오를 수 없다)
            float zMin = 0.f, zMax = 0.f;
            std::vector<Vec2> polygon; // 반시계 방향 볼록 다각형(cm)
        };

        // 파일을 읽는다. 형식이 틀리면 nullopt와 함께 error에 이유를 남긴다.
        static std::optional<CollisionMap> Load(const std::string& path, std::string& error);

        // 캐릭터(발 위치 기준이 아니라 캡슐 중심 z)가 반지름 radius 원으로 장애물과 겹치는가.
        bool Blocked(const proto::Vec3& center, float radius) const;
        // from → to로 반지름 radius 원을 밀었을 때 장애물에 걸리는가(기둥 통과 막기).
        bool SweepBlocked(const proto::Vec3& from, const proto::Vec3& to, float radius) const;
        // 두 점 사이 시야가 막혔는가(공격이 기둥 너머로 닿지 않게). 두 점 중 높은 쪽 높이 범위로 본다.
        bool LineBlocked(const proto::Vec3& from, const proto::Vec3& to) const;

        const std::vector<Obstacle>& Obstacles() const { return obstacles_; }

        // 주인공 캡슐(클라이언트 ACLHero: 반지름 32, 반높이 88)
        static constexpr float kCharacterHalfHeight = 88.f;

    private:
        bool Relevant(const Obstacle& obstacle, float centerZ) const;
        static float SignedDistance(Vec2 point, const std::vector<Vec2>& polygon);
        static bool SegmentHitsPolygon(Vec2 a, Vec2 b, const std::vector<Vec2>& polygon);

        std::vector<Obstacle> obstacles_;
        float stepHeight_ = 45.f;
    };
}
