// 클라이언트 연결 하나.
//
// 스레드 규칙
// - 소켓은 세션 전용 strand에 묶여 있다. 읽기·쓰기·패킷 처리는 모두 이 strand에서 실행된다.
// - Send()는 아무 스레드에서나 불러도 된다(방 strand에서 브로드캐스트할 때 등). 내부에서 strand로 넘긴다.
#pragma once

#include "net/Packet.h"

#include <asio.hpp>

#include <cstdint>
#include <deque>
#include <memory>
#include <string>

namespace asc::game
{
    class Room;
    class RoomManager;
}

namespace asc::net
{
    class Session : public std::enable_shared_from_this<Session>
    {
    public:
        Session(asio::ip::tcp::socket socket, game::RoomManager& rooms, std::uint32_t id);

        void Start();
        void Send(Buffer buffer);
        void Close();

        std::uint32_t Id() const { return id_; }
        // 로그인 이후 바뀌지 않으므로 방 strand에서 읽어도 안전하다.
        const std::string& Name() const { return name_; }

    private:
        // self를 인자로 받아 코루틴이 끝날 때까지 세션이 살아 있게 한다.
        asio::awaitable<void> ReadLoop(std::shared_ptr<Session> self);
        asio::awaitable<void> WriteLoop(std::shared_ptr<Session> self);
        void DoClose();
        void Dispatch(proto::PacketId id, std::span<const std::byte> bytes);
        void HandleLogin(const proto::C_Login& packet);
        void HandleEnterRoom(const proto::C_EnterRoom& packet);
        void LeaveRoom();

        // 이 수를 넘게 쌓이면 클라이언트가 받지 못하는 것으로 보고 끊는다.
        static constexpr std::size_t kMaxQueuedBuffers = 1024;

        asio::ip::tcp::socket socket_;
        asio::steady_timer writeSignal_; // 보낼 것이 생기면 cancel해서 WriteLoop를 깨운다
        std::deque<Buffer> writeQueue_;
        game::RoomManager& rooms_;
        std::shared_ptr<game::Room> room_;
        std::uint32_t teamCode_ = 0;
        const std::uint32_t id_;
        std::string name_;
        bool loggedIn_ = false;
        bool closed_ = false;
    };
}
