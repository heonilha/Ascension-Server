// 방 하나: PvE 층 인스턴스(팀 하나, 최대 3명) 또는 PvP 스테이지(최대 16명).
//
// 스레드 규칙
// - 방의 상태(members_, attacks_)는 방 전용 strand에서만 읽고 쓴다.
// - Enter/Leave/Input/Dodge/Attack은 아무 스레드에서나 불러도 되고, 내부에서 strand로 넘긴다.
// - 서로 다른 방은 서로 다른 strand라서 여러 스레드에서 동시에 돌아간다.
#pragma once

#include "AscensionProtocol.h"
#include "game/CollisionMap.h"
#include "game/Monsters.h"
#include "net/Packet.h"

#include <asio.hpp>

#include <chrono>
#include <cstdint>
#include <deque>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

namespace asc::net
{
    class Session;
}

namespace asc::game
{
    struct RoomOptions
    {
        // PvE 방에서 이 주기(ms)로 서버 내려찍기 시험 패턴을 낸다. 0이면 끈다.
        // 회피 판정을 정해진 박자로 시험하는 용도다(수문장 패턴을 서버로 옮기면 그 패턴으로 바꾼다).
        std::uint32_t pveSlamIntervalMs = 0;
        // PvE 방에 서버 몬스터를 둔다(증원·추적·공격). 끄면 빈 방이다(회피 판정 시험용).
        bool pveMonsters = true;
        // 충돌 지형. 있으면 이동(장애물 안·통과)과 공격(엄폐)을 지형으로도 검사한다. 모든 방이 같은 것을 읽기만 한다.
        std::shared_ptr<const CollisionMap> map;
    };

    class Room : public std::enable_shared_from_this<Room>
    {
    public:
        Room(asio::io_context& io, proto::RoomMode mode, std::uint32_t teamCode, RoomOptions options);

        void Start();
        void Stop();

        void Enter(std::shared_ptr<net::Session> session);
        void Leave(std::uint32_t playerId);
        void Input(std::uint32_t playerId, const proto::C_Input& packet, std::vector<proto::InputCommand> commands);
        void Dodge(std::uint32_t playerId, const proto::C_Dodge& packet);
        void Attack(std::uint32_t playerId, const proto::C_Attack& packet);
        void ReportRtt(std::uint32_t playerId, std::uint32_t rttMs);
        void Respawn(std::uint32_t playerId);

        // 슬롯별 시작 위치(지형 시험에서도 쓴다)
        static proto::Vec3 SpawnPosition(proto::RoomMode mode, int slot);

        std::uint32_t TeamCode() const { return teamCode_; }
        proto::RoomMode Mode() const { return mode_; }

        // 서버 틱 주기. 이동 묶음을 이 주기로 보내고, 판정 유예가 끝난 공격을 확정한다.
        static constexpr auto kTickInterval = std::chrono::milliseconds(proto::kServerTickMs); // 20Hz

        // 이동 입력 처리량 제한(속도 조작 방지). 실제 시간 1초에 입력 60개(이동 틱 수)의 1.03배만큼 처리권이 쌓이고,
        // 입력 하나마다 하나를 쓴다. 네트워크 몰림을 위해 0.5초 분량까지 모아 두고, 넘치는 입력은 버린다.
        // 3% 여유: 0.5초 넘는 끊김으로 처리권이 바닥나도 다시 차오르게 한다(여유가 없으면 그 뒤 작은 흔들림마다 입력을 버린다).
        // 대가로 속도 조작의 상한이 달리기 속도의 103%가 된다.
        static constexpr double kMaxInputBacklog = move::kTickRate * 0.5;
        static constexpr double kInputRateTolerance = 1.03;
        // 현재 데모 맵(개방형 폐허 정원)의 이동 범위. 클라이언트 AAscensionMode::ArenaHalfWidth/Depth와 같다.
        static constexpr float kArenaHalfWidth = 2800.f;
        static constexpr float kArenaHalfDepth = 2400.f;

        // 전투 수치(P1 시험값)
        static constexpr float kMaxHp = 100.f;
        static constexpr std::uint32_t kRespawnProtectionMs = 2000; // 부활 지점에 몰린 몬스터에게 바로 다시 죽지 않게
        static constexpr float kBodyRadius = 40.f;            // 판정 범위에 더하는 몸 반지름
        static constexpr std::uint32_t kSlamWindupMs = 800;   // 내려찍기 예고
        static constexpr float kSlamRadius = 220.f;
        static constexpr float kSlamDamage = 25.f;
        static constexpr std::uint32_t kSlashCooldownMs = 600; // PvP 근접 공격 재사용
        static constexpr float kSlashReach = 120.f;           // 공격자 앞 이 거리에 판정 원의 중심
        static constexpr float kSlashRadius = 130.f;
        static constexpr float kSlashDamage = 20.f;
        static constexpr std::uint32_t kHistoryMs = 1000;     // 위치 이력 보관 시간

        // 플레이어 검기(PvE). 1인 데모의 기본값이다. 성장(무기·각성·레벨) 보정은 아직 서버에 없다.
        static constexpr std::uint32_t kWaveCooldownMs = 600; // 검기 사이 최소 간격(클라이언트 공격 동작 0.72초에 여유)
        static constexpr float kWaveMuzzle = 45.f;            // 캐릭터 앞 이 거리에서 나간다
        static constexpr float kWaveDamage = 20.f, kWaveRadius = 55.f;
        static constexpr std::uint32_t kWaveLifeMs = 1200;
        static constexpr float kHeavyDamage = 35.f, kHeavyRadius = 70.f, kHeavyRadiusPerCharge = 160.f;
        static constexpr int kHeavyPierces = 4, kHeavyPiercesPerCharge = 6;
        static constexpr std::uint32_t kHeavyLifePerChargeMs = 800;
        static constexpr float kMinCharge = 0.2f;             // 0.6초 / 최대 3초
        static constexpr std::uint32_t kFullChargeMs = 2700;  // 완충에 필요한 시간 3초에 10% 여유

        // 지형 검사 반지름. 클라이언트 캡슐(32)은 장애물에 파고들지 않으므로 여유를 두고 작게 본다.
        // 도착 위치는 24cm(8cm 여유), 이동 경로는 16cm로 쓸어 본다(기둥 모서리를 스치는 정상 이동은 통과).
        static constexpr float kTerrainRadius = 24.f;
        static constexpr float kTerrainSweepRadius = 16.f;

    private:
        using Clock = std::chrono::steady_clock;

        struct DodgeRecord
        {
            std::uint32_t serverMs = 0; // 인정한 회피 시작 시각
            std::uint32_t rewindMs = 0; // 도착 시각 - 인정 시각
        };

        struct PositionSample
        {
            std::uint32_t serverMs = 0;
            proto::Vec3 position{};
        };

        struct Member
        {
            std::shared_ptr<net::Session> session;
            std::uint32_t playerId = 0;
            std::string name;
            int slot = 0;
            proto::Vec3 position{};
            proto::Vec3 velocity{};
            float yaw = 0.f;
            proto::MoveState state = proto::MoveState::Idle;
            bool dirty = false;

            // 서버 권한 이동
            move::State sim;
            std::uint32_t lastInputSequence = 0; // 마지막으로 처리(또는 버림)한 입력 번호
            bool hasInput = false;
            bool ackDirty = false;
            double inputCredits = kMaxInputBacklog;
            Clock::time_point creditTime{};
            std::uint32_t droppedInputs = 0;
            Clock::time_point lastDropLog{};

            float hp = kMaxHp;
            bool dead = false;       // HP 0. C_Respawn까지 몬스터가 노리지 않고 공격·회피를 받지 않는다
            std::uint32_t protectedUntilMs = 0; // 부활 직후 보호: 이 시각 전에 명중하는 공격은 맞지 않는다
            std::uint32_t rttMs = 0; // 0 = 아직 모름
            std::deque<DodgeRecord> dodges; // 최근 회피 몇 개(판정 유예 동안 필요한 만큼)
            std::uint32_t lastAttackMs = 0;
            bool hasAttacked = false;
            std::deque<PositionSample> history;
        };

        struct ActiveAttack
        {
            std::uint32_t attackId = 0;
            std::uint32_t attackerId = 0;
            proto::AttackKind kind = proto::AttackKind::Slam;
            std::uint32_t hitMs = 0;
            proto::Vec3 origin{}; // 엄폐 검사 시작점: 공격자 위치(근접) 또는 범위 중심(내려찍기)
            proto::Vec3 center{};
            float radius = 0.f;
            float damage = 0.f;
            float yaw = 0.f, halfAngle = 0.f;   // halfAngle > 0이면 center에서 yaw 방향 부채꼴(몬스터 베기)
            std::uint16_t monsterId = 0;        // 몬스터 공격이면 그 몬스터
            std::uint32_t onlyTargetId = 0;     // 0이 아니면 이 플레이어만 판정한다(돌진 접촉)
            std::vector<std::uint32_t> resolved; // 판정을 끝낸 플레이어
        };

        void DoEnter(std::shared_ptr<net::Session> session);
        void DoLeave(std::uint32_t playerId);
        void DoInput(std::uint32_t playerId, const proto::C_Input& packet, const std::vector<proto::InputCommand>& commands);
        void DoDodge(std::uint32_t playerId, const proto::C_Dodge& packet);
        void DoAttack(std::uint32_t playerId, const proto::C_Attack& packet);
        void DoWave(Member& member, const proto::C_Attack& packet, std::uint32_t nowMs);
        void DoRespawn(std::uint32_t playerId);
        asio::awaitable<void> TickLoop(std::shared_ptr<Room> self);

        void UpdateCombat(std::uint32_t nowMs);
        // 공격을 등록하고(announce면 S_AttackStart로 알리고) 공격 id를 돌려준다. hitMs에 판정한다.
        std::uint32_t StartAttack(ActiveAttack attack, std::uint32_t startMs, bool announce = true);
        void UpdateMonsters(std::uint32_t nowMs, float dt);
        void ApplyMonsterEvents(std::uint32_t nowMs);
        void Resolve(const ActiveAttack& attack, Member& member, std::uint32_t allowedRewindMs);
        static proto::Vec3 PositionAt(const Member& member, std::uint32_t serverMs);

        void Broadcast(const net::Buffer& buffer, std::uint32_t exceptPlayerId = 0);
        Member* Find(std::uint32_t playerId);
        int FreeSlot() const;
        static proto::PlayerInfo ToInfo(const Member& member);

        asio::strand<asio::io_context::executor_type> strand_;
        asio::steady_timer tickTimer_;
        std::vector<Member> members_;
        std::vector<ActiveAttack> attacks_;
        const proto::RoomMode mode_;
        const proto::ModeRules& rules_;
        const std::uint32_t teamCode_;
        const RoomOptions options_;
        std::uint32_t tick_ = 0;
        std::uint32_t nextAttackId_ = 1;
        std::uint32_t nextSlamMs_ = 0;
        std::size_t slamTarget_ = 0;
        bool running_ = false;

        // 몬스터(PvE 방, 옵션이 켜져 있을 때만)
        std::unique_ptr<MonsterField> monsters_;
        MonsterEvents monsterEvents_;
        std::unordered_map<std::uint16_t, std::uint32_t> monsterAttacks_; // 몬스터 → 예고 중인 공격 id
        bool sentMonsters_ = false;      // 직전 틱에 몬스터 목록을 보냈는가(0마리가 되면 빈 목록을 한 번 보낸다)
        Clock::time_point lastTickTime_{};
        double tickCostSumMs_ = 0, tickCostMaxMs_ = 0; // 틱 처리 시간 통계(10초마다 로그)
        double monsterSimSumMs_ = 0, monsterSimMaxMs_ = 0;
        int tickCostSamples_ = 0;
    };
}
