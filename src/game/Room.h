// PvE 층 인스턴스 하나(팀 하나, 최대 3명).
//
// 스레드 규칙
// - 방의 상태(members_)는 방 전용 strand에서만 읽고 쓴다.
// - Enter/Leave/Move는 아무 스레드에서나 불러도 되고, 내부에서 strand로 넘긴다.
// - 서로 다른 방은 서로 다른 strand라서 여러 스레드에서 동시에 돌아간다.
#pragma once

#include "AscensionProtocol.h"
#include "net/Packet.h"

#include <asio.hpp>

#include <chrono>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace asc::net
{
    class Session;
}

namespace asc::game
{
    class Room : public std::enable_shared_from_this<Room>
    {
    public:
        Room(asio::io_context& io, std::uint32_t teamCode);

        void Start();
        void Stop();

        void Enter(std::shared_ptr<net::Session> session);
        void Leave(std::uint32_t playerId);
        void Move(std::uint32_t playerId, const proto::C_Move& packet);

        std::uint32_t TeamCode() const { return teamCode_; }

        // 서버 틱 주기. 이동 묶음을 이 주기로 보낸다.
        static constexpr auto kTickInterval = std::chrono::milliseconds(proto::kServerTickMs); // 20Hz

        // 이동 검증 기준(토큰 버킷). 시간이 지나면 이동 가능 거리(budget)가 kMaxSpeed로 쌓이고,
        // 이동할 때 그만큼 쓴다. 패킷 몇 개가 한꺼번에 도착해도 앞서 쌓인 거리로 통과한다.
        // 달리기 450cm/s, 회피 최고 약 920cm/s에 여유를 둔 값이다.
        static constexpr float kMaxSpeed = 1100.f;
        static constexpr float kMaxBudget = kMaxSpeed * 0.5f; // 최대 0.5초 분량까지만 모아 둔다
        static constexpr float kMoveSlack = 20.f;
        // 현재 데모 맵(개방형 폐허 정원)의 이동 범위. 클라이언트 AAscensionMode::ArenaHalfWidth/Depth와 같다.
        static constexpr float kArenaHalfWidth = 2800.f;
        static constexpr float kArenaHalfDepth = 2400.f;

    private:
        using Clock = std::chrono::steady_clock;

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
            Clock::time_point lastMoveTime{};
            float moveBudget = kMaxBudget;
            Clock::time_point lastCorrectionTime{};
            bool dirty = false;
        };

        void DoEnter(std::shared_ptr<net::Session> session);
        void DoLeave(std::uint32_t playerId);
        void DoMove(std::uint32_t playerId, const proto::C_Move& packet);
        asio::awaitable<void> TickLoop(std::shared_ptr<Room> self);

        void Broadcast(const net::Buffer& buffer, std::uint32_t exceptPlayerId = 0);
        Member* Find(std::uint32_t playerId);
        int FreeSlot() const;
        static proto::Vec3 SpawnPosition(int slot);
        static proto::PlayerInfo ToInfo(const Member& member);

        asio::strand<asio::io_context::executor_type> strand_;
        asio::steady_timer tickTimer_;
        std::vector<Member> members_;
        const std::uint32_t teamCode_;
        std::uint32_t tick_ = 0;
        bool running_ = false;
    };
}
