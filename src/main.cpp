// ascension_server [--port 7777] [--threads N]
#include "AscensionProtocol.h"
#include "core/Log.h"
#include "game/RoomManager.h"
#include "net/Server.h"

#include <asio.hpp>

#include <algorithm>
#include <charconv>
#include <cstdlib>
#include <string_view>
#include <thread>
#include <vector>

namespace
{
    template <class T>
    T ParseNumber(std::string_view text, T fallback)
    {
        T value{};
        const auto [end, error] = std::from_chars(text.data(), text.data() + text.size(), value);
        return error == std::errc{} && end == text.data() + text.size() ? value : fallback;
    }
}

int main(int argc, char** argv)
{
    std::uint16_t port = asc::proto::kDefaultPort;
    unsigned threads = std::max(1u, std::thread::hardware_concurrency());

    for (int i = 1; i + 1 < argc; i += 2)
    {
        const std::string_view key = argv[i], value = argv[i + 1];
        if (key == "--port") port = ParseNumber<std::uint16_t>(value, port);
        else if (key == "--threads") threads = std::max(1u, ParseNumber<unsigned>(value, threads));
    }

    try
    {
        asio::io_context io(static_cast<int>(threads));
        asc::game::RoomManager rooms(io);
        asc::net::Server server(io, port, rooms);
        server.Start();

        asio::signal_set signals(io, SIGINT, SIGTERM);
        signals.async_wait([&io](const asio::error_code&, int) {
            asc::Log("종료 신호 수신");
            io.stop();
        });

        asc::Log("Ascension 서버 시작: 포트 {}, 스레드 {}, 프로토콜 v{}", port, threads, asc::proto::kVersion);

        std::vector<std::jthread> workers;
        for (unsigned i = 1; i < threads; ++i) workers.emplace_back([&io] { io.run(); });
        io.run(); // 메인 스레드도 일한다
    }
    catch (const std::exception& error)
    {
        asc::Log("치명적 오류: {}", error.what());
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
