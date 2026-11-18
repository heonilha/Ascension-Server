// PvE 방 하나의 몬스터와 플레이어 검기(투사체) 시뮬레이션.
//
// 1인 데모(클라이언트 CombatLab.cpp의 ACLEnemy·ACLBladeWave·TickSurvival)의 규칙을 서버로 옮긴 것이다.
// - 종류: 0 망령 검병(부채꼴 베기), 1 폭주 망령(직선 돌진), 2 망령 지휘관(2연속 베기, 경직에 끊기지 않음)
// - 추적: 가장 가까운 살아 있는 플레이어. 정면이 막히면 ±45·±90·±135도로 돌아간다.
// - 피격: 경직 중 넉백(지형에 막히면 멈춤), 0.35초 동안은 다시 경직되지 않는다. 지휘관 외에는 예고가 끊긴다.
// - 증원: 주기적 증원 + 목표 수 보충 + 60초마다 지휘관, 층당 최대 48마리.
// - 검기: 가까운 순서로 관통하고 같은 몬스터를 두 번 맞히지 않는다. 벽에 막히고, 기둥 뒤 몬스터는 맞지 않는다.
//
// 네트워크를 모른다. Room이 방 strand에서 Update를 부르고, 생긴 일(Events)을 패킷으로 바꿔 보낸다.
// 플레이어에게 가는 피해는 여기서 정하지 않는다: 예고(Tell)와 돌진 접촉(DashTouch)을 알리면
// Room이 회피 무적·지연 보정 판정을 거쳐 확정한다.
#pragma once

#include "AscensionProtocol.h"
#include "game/CollisionMap.h"

#include <cstdint>
#include <random>
#include <span>
#include <vector>

namespace asc::game
{
    struct MonsterTuning
    {
        // 종류별 [검병, 폭주, 지휘관]
        static constexpr float kMaxHp[3] = {50.f, 70.f, 150.f};
        static constexpr float kRadius = 35.f;            // 몬스터 캡슐 반지름
        static constexpr float kChaseSpeed = 220.f;
        static constexpr float kSteerProbe = 110.f;       // 이 거리 앞까지 막혔는지 본다
        static constexpr float kSteerRadius = 30.f;
        // 베기(검병·지휘관)
        static constexpr float kArcTrigger = 155.f;       // 이 거리 안이면 예고 시작
        static constexpr float kArcRange = 180.f;
        static constexpr float kArcHalfAngle = 55.f;
        static constexpr std::uint32_t kArcTellMs = 600, kArcSecondTellMs = 450;
        static constexpr float kArcDamage[3] = {10.f, 0.f, 16.f};
        static constexpr std::uint32_t kArcRecoverMs[3] = {800, 0, 1500};
        // 돌진(폭주 망령)
        static constexpr float kChargeTrigger = 360.f;
        static constexpr std::uint32_t kChargeTellMs = 900, kDashMs = 500, kDashRecoverMs = 1200;
        static constexpr float kDashSpeed = 1200.f;
        static constexpr float kDashHitDistance = 105.f;  // 돌진 중 이 거리 안의 플레이어가 맞는다
        static constexpr float kDashDamage = 15.f;
        // 피격 반응
        static constexpr std::uint32_t kStaggerMs = 160, kEliteStaggerMs = 80, kReactionCooldownMs = 350;
        static constexpr float kEliteKnockback = 15.f, kKnockbackPerDamage = 1.5f, kKnockbackMin = 25.f, kKnockbackMax = 65.f;
        // 증원(생존 모드 규칙)
        static constexpr float kSpawnMinDistance = 650.f, kSpawnMaxDistance = 1250.f;
        static constexpr float kSpawnSeparation = 160.f;  // 다른 몬스터와 이 이상 떨어져 생긴다
        static constexpr float kSpawnClearRadius = 45.f;
        static constexpr float kSpawnPlayerDistance = 400.f; // 다른 플레이어 바로 옆에는 생기지 않는다
        // 검기
        static constexpr float kWaveSpeed = 1400.f;
        static constexpr float kWaveHitPadding = 32.f;    // 검기 반지름에 더하는 몬스터 몸 크기
        static constexpr float kWaveWallRadius = 15.f;
    };

    struct Monster
    {
        std::uint16_t id = 0;
        std::uint8_t kind = 0;
        proto::MonsterState phase = proto::MonsterState::Chase; // Idle·Chase·Tell·Dash·Recover (경직은 따로 겹친다)
        float x = 0.f, y = 0.f, yaw = 0.f;                      // yaw: 도
        float hp = 0.f;
        std::uint32_t deadlineMs = 0;        // 지금 단계가 끝나는 시각
        float dirX = 1.f, dirY = 0.f;        // 예고·돌진 방향
        int strike = 0;                      // 지휘관의 몇 번째 베기인가
        std::uint32_t staggerUntilMs = 0, reactionReadyMs = 0;
        float knockX = 0.f, knockY = 0.f, knockRemaining = 0.f;
        std::vector<std::uint32_t> dashHits; // 이번 돌진에 이미 맞은 플레이어

        proto::MonsterState ShownState(std::uint32_t nowMs) const
        {
            return nowMs < staggerUntilMs ? proto::MonsterState::Stagger : phase;
        }
    };

    struct MonsterPlayer
    {
        std::uint32_t id = 0;
        float x = 0.f, y = 0.f;
        bool alive = false;
    };

    // Update·피해 처리 중에 생긴 일. Room이 읽어 패킷과 공격 판정으로 바꾼다.
    struct MonsterEvents
    {
        // 공격 예고 시작(hitMs에 베기 명중 또는 돌진 출발) 또는 취소(cancel). 일어난 순서대로 쌓인다:
        // 같은 틱에 예고를 시작하고 곧바로 맞아 끊길 수 있으므로 순서를 지켜 처리해야 한다.
        struct Tell
        {
            std::uint16_t monsterId;
            std::uint8_t kind;
            bool cancel;
            float x = 0.f, y = 0.f, yaw = 0.f;
            std::uint32_t startMs = 0, hitMs = 0;
        };
        struct DashTouch // 돌진하는 몬스터가 플레이어에 닿았다
        {
            std::uint16_t monsterId;
            std::uint32_t playerId;
            float x, y;
        };
        struct Hit // 몬스터가 피해를 입었다(hpAfter 0 = 사망)
        {
            std::uint16_t monsterId;
            std::uint32_t attackerId;
            float damage, hpAfter, x, y;
        };
        struct WaveEnd
        {
            std::uint32_t waveId;
            float x, y;
        };
        std::vector<Tell> tells;
        std::vector<DashTouch> dashTouches;
        std::vector<Hit> hits;
        std::vector<WaveEnd> waveEnds;

        void Clear()
        {
            tells.clear(), dashTouches.clear(), hits.clear(), waveEnds.clear();
        }
    };

    class MonsterField
    {
    public:
        // map이 없으면(지형 끔) 맵 범위만 지킨다.
        MonsterField(const CollisionMap* map, std::uint32_t seed, std::uint32_t nowMs);

        // 한 틱 진행: 증원 → 몬스터 AI·이동 → 검기. dt는 초.
        void Update(std::uint32_t nowMs, float dt, std::span<const MonsterPlayer> players, MonsterEvents& events);

        // 검기 발사. 만든 검기 id를 돌려준다.
        std::uint32_t SpawnWave(std::uint32_t ownerId, float x, float y, float yawDegrees, float damage, float radius, int pierces,
                                std::uint32_t lifeMs, std::uint32_t nowMs);

        // 몬스터 하나에 피해. 없는 id면 false.
        bool Damage(std::uint16_t monsterId, std::uint32_t attackerId, float damage, float pushX, float pushY, std::uint32_t nowMs,
                    MonsterEvents& events);

        const std::vector<Monster>& Monsters() const { return monsters_; }
        std::size_t WaveCount() const { return waves_.size(); }
        std::uint32_t TotalSpawned() const { return totalSpawned_; }
        std::uint32_t TotalKilled() const { return totalKilled_; }

        // 시험용: 원하는 자리에 몬스터를 놓는다(증원 규칙을 거치지 않는다).
        Monster& SpawnAt(std::uint8_t kind, float x, float y, std::uint32_t nowMs);
        void SetSpawning(bool enabled) { spawning_ = enabled; }

        static constexpr float kArenaHalfWidth = 2800.f, kArenaHalfDepth = 2400.f;

    private:
        struct Wave
        {
            std::uint32_t id = 0, ownerId = 0;
            float x = 0.f, y = 0.f, dirX = 1.f, dirY = 0.f;
            float damage = 0.f, radius = 0.f;
            int pierces = 1;
            std::uint32_t endMs = 0;
            std::vector<std::uint16_t> hitMonsters;
        };

        void UpdateSpawning(std::uint32_t nowMs, std::span<const MonsterPlayer> players);
        void SpawnBatch(int count, bool elite, std::uint32_t nowMs, std::span<const MonsterPlayer> players);
        void UpdateMonster(Monster& monster, std::uint32_t nowMs, float dt, std::span<const MonsterPlayer> players, MonsterEvents& events);
        void BeginTell(Monster& monster, const MonsterPlayer& target, std::uint32_t startMs, MonsterEvents& events);
        void Chase(Monster& monster, float toX, float toY, float dt);
        void Separate();
        void UpdateWaves(std::uint32_t nowMs, float dt, MonsterEvents& events);

        bool Overlaps(float x, float y, float radius) const;       // 단단한 장애물 또는 맵 밖
        bool SweepBlocked(float x0, float y0, float x1, float y1, float radius) const;
        void PushOut(float& x, float& y, float radius) const;
        bool LineOfSight(float x0, float y0, float x1, float y1) const;
        float Random(float low, float high);

        const CollisionMap* map_;
        std::mt19937 rng_;
        std::vector<Monster> monsters_;
        std::vector<Wave> waves_;
        std::uint16_t nextMonsterId_ = 1;
        std::uint32_t nextWaveId_ = 1;
        std::uint32_t startMs_ = 0, reinforcementReadyMs_ = 0, refillReadyMs_ = 0, eliteReadyMs_ = 0;
        std::uint32_t totalSpawned_ = 0, totalKilled_ = 0;
        bool spawning_ = true;
    };
}
