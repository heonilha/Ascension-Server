// 접속을 받아 세션을 만든다.
#pragma once

#include <asio.hpp>

#include <atomic>
#include <cstdint>

namespace asc::game
{
    class RoomManager;
}

namespace asc::net
{
    class Server
    {
    public:
        Server(asio::io_context& io, std::uint16_t port, game::RoomManager& rooms);
        void Start();

    private:
        asio::awaitable<void> AcceptLoop();

        asio::io_context& io_;
        asio::ip::tcp::acceptor acceptor_;
        game::RoomManager& rooms_;
        std::atomic<std::uint32_t> nextSessionId_{0};
    };
}
