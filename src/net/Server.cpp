#include "net/Server.h"

#include "core/Log.h"
#include "net/Session.h"

namespace asc::net
{
    Server::Server(asio::io_context& io, std::uint16_t port, game::RoomManager& rooms)
        : io_(io), acceptor_(io, asio::ip::tcp::endpoint(asio::ip::tcp::v4(), port), /*reuse_address*/ true), rooms_(rooms)
    {
    }

    void Server::Start()
    {
        asio::co_spawn(acceptor_.get_executor(), AcceptLoop(), asio::detached);
    }

    asio::awaitable<void> Server::AcceptLoop()
    {
        for (;;)
        {
            asio::error_code ec;
            // 세션마다 전용 strand를 준다. 이 소켓의 모든 비동기 작업은 그 strand에서 순서대로 실행된다.
            asio::ip::tcp::socket socket =
                co_await acceptor_.async_accept(asio::make_strand(io_), asio::redirect_error(asio::use_awaitable, ec));
            if (ec)
            {
                if (ec == asio::error::operation_aborted) co_return;
                Log("accept 실패: {}", ec.message());
                continue;
            }
            socket.set_option(asio::ip::tcp::no_delay(true), ec); // 작은 이동 패킷을 모으지 않고 바로 보낸다
            std::make_shared<Session>(std::move(socket), rooms_, ++nextSessionId_)->Start();
        }
    }
}
