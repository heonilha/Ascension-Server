// 서버 충돌 지형: 클라이언트 맵의 막히는 구조물을 2D 볼록 다각형 + 높이 범위로 들고 있는다.
//
// 데이터는 클라이언트 저장소의 Scripts/ExportServerCollision.py가 실제 맵 액터에서 뽑은
// data/CombatLab.txt다(손으로 고치지 않는다). 클라이언트는 같은 파일을 게임 물리와 격자 비교해 검증한다.
// 파일 해석과 기하 계산은 클라이언트와 같은 코드(protocol/AscensionMovement.h)를 쓴다.
//
// 스레드 규칙: 서버 시작 때 한 번 읽고 이후에는 읽기만 하므로 여러 방(strand)에서 동시에 써도 된다.
#pragma once

#include "AscensionMovement.h"

#include <optional>
#include <string>
#include <vector>

namespace asc::game
{
    class CollisionMap
    {
    public:
        // 파일을 읽는다. 형식이 틀리면 nullopt와 함께 error에 이유를 남긴다.
        static std::optional<CollisionMap> Load(const std::string& path, std::string& error);

        // 캡슐 중심이 반지름 radius 원으로 오를 수 없는 장애물과 겹치는가(머리 위 구조물·낮은 받침 제외).
        bool Blocked(const proto::Vec3& center, float radius) const;
        // from → to로 반지름 radius 원을 밀었을 때 장애물에 걸리는가.
        bool SweepBlocked(const proto::Vec3& from, const proto::Vec3& to, float radius) const;
        // 두 점 사이 시야가 막혔는가(공격이 기둥 너머로 닿지 않게). 두 점 중 높은 쪽 높이 범위로 본다.
        bool LineBlocked(const proto::Vec3& from, const proto::Vec3& to) const;

        const move::World& World() const { return world_; }
        const std::vector<move::Obstacle>& Obstacles() const { return world_.obstacles; }

    private:
        bool Relevant(const move::Obstacle& obstacle, float centerZ) const;
        static bool SegmentHitsPolygon(move::Vec2 a, move::Vec2 b, const std::vector<move::Vec2>& polygon);

        move::World world_;
    };
}
