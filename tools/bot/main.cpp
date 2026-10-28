// 헤드리스 봇 클라이언트. 서버 동작 확인과 이후 부하 테스트에 쓴다.
//
// ascension_bot [--host 127.0.0.1] [--port 7777] [--bots 6] [--team-size 3] [--seconds 5]
//               [--team-base 1000] [--cheat] [--burst]
//
// 봇 i는 teamCode = team-base + i / team-size 방에 들어가 시작 위치 주변을 원을 그리며 돈다.
// 끝나면 봇마다 받은 패킷을 확인하고, 문제가 있으면 종료 코드 1을 돌려준다.
//   - 같은 방의 다른 봇 이동을 모두 받았는가
//   - 정상 이동에 S_Correction이 오지 않았는가
//   - 정원(3명)을 넘긴 봇은 RoomFull을 받았는가
//   - --cheat: 0번 봇이 순간이동을 시도하면 S_Correction을 받는가
#include "AscensionProtocol.h"
#include "net/Packet.h"

#include <asio.hpp>

#include <charconv>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <format>
#include <memory>
#include <print>
#include <set>
#include <string>
#include <string_view>
#include <vector>

using namespace asc;
using namespace asc::proto;
using asio::ip::tcp;

namespace
{
    struct Options
    {
        std::string host = "127.0.0.1";
        std::uint16_t port = kDefaultPort;
        int bots = 6;
        int teamSize = 3;
        int seconds = 5;
        std::uint32_t teamBase = 1000;
        bool cheat = false;
        bool burst = false; // 회피 크기 이동 3개를 한꺼번에 보낸다(네트워크 몰림 재현)
    };

    struct Stats
    {
        EnterResult enterResult = EnterResult::NotLoggedIn;
        bool entered = false;
        int moveBatches = 0;
        int joined = 0;
        int left = 0;
        int corrections = 0;
        int pongs = 0;
        std::uint32_t lastRttMs = 0;
        std::set<std::uint32_t> movedOthers; // 이동을 받아 본 다른 플레이어
    };

    std::uint32_t NowMs()
    {
        using namespace std::chrono;
        return static_cast<std::uint32_t>(duration_cast<milliseconds>(steady_clock::now().time_since_epoch()).count());
    }

    template <net::Packet T>
    asio::awaitable<void> SendPacket(tcp::socket& socket, const T& packet)
    {
        co_await asio::async_write(socket, asio::buffer(&packet, sizeof(T)), asio::use_awaitable);
    }

    // 패킷 하나를 통째로 읽는다(헤더 포함).
    asio::awaitable<std::vector<std::byte>> ReadPacket(tcp::socket& socket)
    {
        PacketHeader header{};
        co_await asio::async_read(socket, asio::buffer(&header, sizeof(header)), asio::use_awaitable);
        if (header.size < sizeof(header) || header.size > kMaxPacketSize) throw std::runtime_error("잘못된 패킷 크기");
        std::vector<std::byte> bytes(header.size);
        std::memcpy(bytes.data(), &header, sizeof(header));
        if (header.size > sizeof(header))
            co_await asio::async_read(socket, asio::buffer(bytes.data() + sizeof(header), header.size - sizeof(header)), asio::use_awaitable);
        co_return bytes;
    }

    PacketId IdOfBytes(const std::vector<std::byte>& bytes)
    {
        PacketHeader header;
        std::memcpy(&header, bytes.data(), sizeof(header));
        return header.id;
    }

    class Bot : public std::enable_shared_from_this<Bot>
    {
    public:
        Bot(asio::io_context& io, const Options& options, int index)
            : socket_(io), timer_(io), options_(options), index_(index),
              teamCode_(options.teamBase + static_cast<std::uint32_t>(index / options.teamSize))
        {
        }

        asio::awaitable<void> Run()
        {
            auto self = shared_from_this();
            try
            {
                tcp::resolver resolver(socket_.get_executor());
                const auto endpoints = co_await resolver.async_resolve(options_.host, std::to_string(options_.port), asio::use_awaitable);
                co_await asio::async_connect(socket_, endpoints, asio::use_awaitable);
                socket_.set_option(tcp::no_delay(true));

                auto login = Make<C_Login>();
                login.version = kVersion;
                CopyName(login.name, std::format("bot-{}", index_).c_str());
                co_await SendPacket(socket_, login);
                const auto loginReply = net::Read<S_Login>(co_await ReadPacket(socket_));
                if (!loginReply || loginReply->result != LoginResult::Ok) throw std::runtime_error("로그인 실패");
                playerId_ = loginReply->playerId;

                auto enter = Make<C_EnterRoom>();
                enter.teamCode = teamCode_;
                co_await SendPacket(socket_, enter);
                const auto enterBytes = co_await ReadPacket(socket_);
                if (IdOfBytes(enterBytes) != PacketId::S_EnterRoom) throw std::runtime_error("S_EnterRoom 대신 다른 패킷");
                S_EnterRoom reply;
                std::memcpy(&reply, enterBytes.data(), sizeof(reply));
                stats_.enterResult = reply.result;
                if (reply.result != EnterResult::Ok) co_return; // 정원 초과 등: 여기서 끝
                stats_.entered = true;
                spawn_ = reply.spawnPosition;
                position_ = spawn_;

                asio::co_spawn(socket_.get_executor(), ReadLoop(self), asio::detached);
                co_await MoveLoop();
            }
            catch (const std::exception& error)
            {
                std::println("bot-{} 오류: {}", index_, error.what());
                failed_ = true;
            }
            asio::error_code ignored;
            socket_.close(ignored);
        }

        bool Report(int expectedOthers) const
        {
            const bool overCapacity = index_ % options_.teamSize >= kMaxRoomPlayers;
            bool ok = !failed_;
            std::string problems;
            if (overCapacity)
            {
                if (stats_.enterResult != EnterResult::RoomFull) ok = false, problems += " RoomFull을 받지 못함";
            }
            else
            {
                if (!stats_.entered) ok = false, problems += " 입장 실패";
                if (static_cast<int>(stats_.movedOthers.size()) != expectedOthers)
                    ok = false, problems += std::format(" 다른 봇 이동 {}/{}", stats_.movedOthers.size(), expectedOthers);
                const bool cheater = options_.cheat && index_ == 0;
                if (cheater && stats_.corrections == 0) ok = false, problems += " 순간이동이 거부되지 않음";
                if (!cheater && stats_.corrections > 0) ok = false, problems += std::format(" 정상 이동 거부 {}회", stats_.corrections);
            }
            std::println("bot-{} 방 {} {} | 입장 {} 이동묶음 {} 보정 {} RTT {}ms {}", index_, teamCode_, ok ? "PASS" : "FAIL",
                         static_cast<int>(stats_.enterResult), stats_.moveBatches, stats_.corrections, stats_.lastRttMs, problems);
            return ok;
        }

        int Index() const { return index_; }

    private:
        asio::awaitable<void> ReadLoop([[maybe_unused]] std::shared_ptr<Bot> self)
        {
            try
            {
                for (;;)
                {
                    const auto bytes = co_await ReadPacket(socket_);
                    switch (IdOfBytes(bytes))
                    {
                    case PacketId::S_MoveBatch:
                    {
                        ++stats_.moveBatches;
                        S_MoveBatch batch;
                        std::memcpy(&batch, bytes.data(), sizeof(batch));
                        if (bytes.size() != sizeof(batch) + batch.count * sizeof(MoveEntry)) throw std::runtime_error("S_MoveBatch 크기 불일치");
                        for (int i = 0; i < batch.count; ++i)
                        {
                            MoveEntry entry;
                            std::memcpy(&entry, bytes.data() + sizeof(batch) + i * sizeof(MoveEntry), sizeof(entry));
                            if (entry.playerId != playerId_) stats_.movedOthers.insert(entry.playerId);
                        }
                        break;
                    }
                    case PacketId::S_PlayerJoined: ++stats_.joined; break;
                    case PacketId::S_PlayerLeft: ++stats_.left; break;
                    case PacketId::S_Correction:
                        if (const auto correction = net::Read<S_Correction>(bytes))
                        {
                            ++stats_.corrections;
                            position_ = correction->position; // 서버가 정한 위치로 되돌린다
                        }
                        break;
                    case PacketId::S_Pong:
                        if (const auto pong = net::Read<S_Pong>(bytes))
                        {
                            ++stats_.pongs;
                            stats_.lastRttMs = NowMs() - pong->clientTimeMs;
                        }
                        break;
                    default: break;
                    }
                }
            }
            catch (const std::exception&)
            {
                // 연결 종료
            }
        }

        asio::awaitable<void> MoveLoop()
        {
            using namespace std::chrono;
            const auto start = steady_clock::now();
            const auto end = start + seconds(options_.seconds);
            std::uint32_t sequence = 0;
            const float phase = static_cast<float>(index_);

            while (steady_clock::now() < end)
            {
                timer_.expires_after(milliseconds(66)); // 약 15Hz
                co_await timer_.async_wait(asio::use_awaitable);

                const float t = duration<float>(steady_clock::now() - start).count();
                const float angle = t * 1.5f + phase; // 반지름 120cm, 약 180cm/s
                const Vec3 previous = position_;
                position_ = {spawn_.x + 120.f * std::cos(angle) - 120.f * std::cos(phase), spawn_.y + 120.f * std::sin(angle) - 120.f * std::sin(phase), spawn_.z};

                // --cheat: 2초쯤 한 번 40m 순간이동을 시도한다
                if (options_.cheat && index_ == 0 && !cheated_ && t > 2.f)
                {
                    cheated_ = true;
                    position_.x += 4000.f;
                }

                auto move = Make<C_Move>();
                move.sequence = ++sequence;
                move.position = position_;
                move.velocity = {(position_.x - previous.x) / 0.066f, (position_.y - previous.y) / 0.066f, 0.f};
                move.yaw = angle * 57.2958f;
                move.state = MoveState::Run;
                co_await SendPacket(socket_, move);

                // --burst: 1초마다 회피 한 번 분량(60cm × 3)을 기다림 없이 연달아 보낸다. 거부되면 안 된다.
                if (options_.burst && sequence % 15 == 7)
                {
                    for (int step = 1; step <= 3; ++step)
                    {
                        move.sequence = ++sequence;
                        move.position.x = position_.x + 60.f * static_cast<float>(step);
                        move.state = MoveState::Dodge;
                        co_await SendPacket(socket_, move);
                    }
                }

                if (sequence % 15 == 0)
                {
                    auto ping = Make<C_Ping>();
                    ping.clientTimeMs = NowMs();
                    co_await SendPacket(socket_, ping);
                }
            }
            // 마지막 패킷이 도착할 시간을 조금 준다
            timer_.expires_after(milliseconds(300));
            co_await timer_.async_wait(asio::use_awaitable);
        }

        tcp::socket socket_;
        asio::steady_timer timer_;
        const Options& options_;
        const int index_;
        const std::uint32_t teamCode_;
        std::uint32_t playerId_ = 0;
        Vec3 spawn_{}, position_{};
        Stats stats_;
        bool cheated_ = false;
        bool failed_ = false;
    };

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
    Options options;
    for (int i = 1; i < argc; ++i)
    {
        const std::string_view key = argv[i];
        const std::string_view value = i + 1 < argc ? argv[i + 1] : "";
        if (key == "--cheat") { options.cheat = true; continue; }
        if (key == "--burst") { options.burst = true; continue; }
        if (key == "--host") options.host = value;
        else if (key == "--port") options.port = ParseNumber<std::uint16_t>(value, options.port);
        else if (key == "--bots") options.bots = ParseNumber<int>(value, options.bots);
        else if (key == "--team-size") options.teamSize = std::max(1, ParseNumber<int>(value, options.teamSize));
        else if (key == "--seconds") options.seconds = ParseNumber<int>(value, options.seconds);
        else if (key == "--team-base") options.teamBase = ParseNumber<std::uint32_t>(value, options.teamBase);
        ++i;
    }

    asio::io_context io;
    std::vector<std::shared_ptr<Bot>> bots;
    for (int i = 0; i < options.bots; ++i)
    {
        auto bot = std::make_shared<Bot>(io, options, i);
        bots.push_back(bot);
        asio::co_spawn(io, bot->Run(), asio::detached);
    }
    std::println("봇 {}개 접속: {}:{}, 팀당 {}명, {}초", options.bots, options.host, options.port, options.teamSize, options.seconds);
    io.run();

    bool allOk = true;
    for (const auto& bot : bots)
    {
        // 같은 방에서 정상 입장한 다른 봇 수
        const int team = bot->Index() / options.teamSize;
        const int membersInTeam = std::min({options.teamSize, static_cast<int>(kMaxRoomPlayers), options.bots - team * options.teamSize});
        allOk &= bot->Report(membersInTeam - 1);
    }
    std::println("{}", allOk ? "결과: PASS" : "결과: FAIL");
    return allOk ? EXIT_SUCCESS : EXIT_FAILURE;
}
