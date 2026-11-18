// ascension_server [--port 7777] [--threads N] [--pve-monsters 1] [--pve-slam-ms 0] [--map data/CombatLab.txt]
//   --pve-monsters: PvE 방에 서버 몬스터를 둔다(기본 켬). 0이면 빈 방
//   --pve-slam-ms: PvE 방에서 이 주기로 서버 내려찍기 시험 패턴을 낸다(회피 판정 시험용, 기본 끔)
//   --map: 충돌 지형 파일. 기본값은 현재 폴더나 실행 파일 위쪽 폴더에서 찾는다. none이면 지형 검사를 끈다
#include "AscensionProtocol.h"
#include "core/Log.h"
#include "game/RoomManager.h"
#include "net/Server.h"

#include <asio.hpp>

#include "core/Platform.h"

#include <algorithm>
#include <charconv>
#include <cstdlib>
#include <memory>
#include <string>
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
    asc::InitPlatform();
    std::uint16_t port = asc::proto::kDefaultPort;
    unsigned threads = std::max(1u, std::thread::hardware_concurrency());
    asc::game::RoomOptions roomOptions;
    std::string mapPath = asc::FindDataFile(argv[0], "data/CombatLab.txt");

    for (int i = 1; i + 1 < argc; i += 2)
    {
        const std::string_view key = argv[i], value = argv[i + 1];
        if (key == "--port") port = ParseNumber<std::uint16_t>(value, port);
        else if (key == "--threads") threads = std::max(1u, ParseNumber<unsigned>(value, threads));
        else if (key == "--pve-slam-ms") roomOptions.pveSlamIntervalMs = ParseNumber<std::uint32_t>(value, 0);
        else if (key == "--pve-monsters") roomOptions.pveMonsters = ParseNumber<int>(value, 1) != 0;
        else if (key == "--map") mapPath = value;
    }

    if (mapPath != "none")
    {
        std::string error;
        auto map = asc::game::CollisionMap::Load(mapPath, error);
        if (!map)
        {
            asc::Log("충돌 지형을 읽지 못함: {} (--map 경로를 지정, 끄려면 --map none)", error);
            return EXIT_FAILURE;
        }
        roomOptions.map = std::make_shared<const asc::game::CollisionMap>(std::move(*map));
        asc::Log("충돌 지형: {} (장애물 {}개)", mapPath, roomOptions.map->Obstacles().size());
    }
    else asc::Log("충돌 지형 끔(--map none): 이동·공격을 지형으로 검사하지 않음");

    try
    {
        asio::io_context io(static_cast<int>(threads));
        asc::game::RoomManager rooms(io, roomOptions);
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
