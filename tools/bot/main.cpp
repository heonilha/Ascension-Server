// 헤드리스 봇 클라이언트. 서버 동작 확인과 이후 부하 테스트에 쓴다.
//
// ascension_bot [--host 127.0.0.1] [--port 7777] [--bots 6] [--team-size 3] [--seconds 5]
//               [--team-base 1000] [--mode pve|pvp] [--cheat] [--burst] [--wall]
//               [--latency 0] [--dodge-test] [--dodge-cheat] [--rtt-lie 0]
//
// 봇 i는 teamCode = team-base + i / team-size 방에 들어가 시작 위치 주변을 원을 그리며 돈다.
// 끝나면 봇마다 받은 패킷을 확인하고, 문제가 있으면 종료 코드 1을 돌려준다.
//   - 같은 방의 다른 봇 이동을 모두 받았는가
//   - 정상 이동에 S_Correction이 오지 않았는가
//   - 정원을 넘긴 봇은 RoomFull을 받았는가
//   - --cheat: 0번 봇이 순간이동을 시도하면 S_Correction을 받는가
//   - --wall: PvE 0번 봇이 남쪽 부서진 기둥(100,-650)을 뚫고 걸어가면 S_Correction을 받는가(서버 충돌 지형)
//
// 회피 판정 시험(--dodge-test)
//   봇은 제자리에 서서, 자기를 노린 공격(S_AttackStart)마다 정해 둔 시각에 회피를 누른다.
//   PvE는 서버의 내려찍기 시험 패턴(서버 --pve-slam-ms)을, PvP는 0번 봇의 근접 공격을 받는다.
//   서버 결과(S_HitResult)가 봇이 계산한 기대값(회피/피격)과 같은지 확인한다.
//   기대값이 판정 경계에서 25ms 안이면 시계 추정 오차 때문에 비교에서 뺀다.
//   --latency: 송신·수신에 각각 이만큼 인공 지연을 넣는다(한쪽 방향 ms).
//   --dodge-cheat: 늦게 누르고 누른 시각을 앞당겨 보내는 조작을 섞는다. 상한 밖이면 피격이어야 한다.
//   --rtt-lie: 서버에 알려 주는 왕복 시간을 이 값으로 속인다(되감기 상한 시험).
#include "AscensionProtocol.h"
#include "net/Packet.h"

#include <asio.hpp>

#include <algorithm>
#include <charconv>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <deque>
#include <format>
#include <memory>
#include <numbers>
#include <optional>
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
    using SteadyClock = std::chrono::steady_clock;

    struct Options
    {
        std::string host = "127.0.0.1";
        std::uint16_t port = kDefaultPort;
        int bots = 6;
        int teamSize = 3;
        int seconds = 5;
        std::uint32_t teamBase = 1000;
        RoomMode mode = RoomMode::PvE;
        bool cheat = false;
        bool wall = false; // 0번 봇이 기둥을 뚫고 지나가려 한다
        bool burst = false; // 회피 크기 이동 3개를 한꺼번에 보낸다(네트워크 몰림 재현)
        int latencyMs = 0;
        bool dodgeTest = false;
        bool dodgeCheat = false;
        std::uint32_t rttLie = 0;
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
        int attacksSent = 0;
        int myAttackResults = 0; // 내 공격에 대한 판정 수(PvP 공격 봇)
    };

    // 회피 시험 계획 하나: 명중 시각 기준 언제 누를지
    struct DodgePlan
    {
        const char* name;
        std::optional<int> pressOffsetMs;  // 없으면 누르지 않는다
        std::optional<int> stampOffsetMs;  // 있으면 누른 시각 대신 "명중 시각 + 이 값"을 보낸다(조작)
    };

    struct DodgeTrial
    {
        std::uint32_t attackId = 0;
        std::uint32_t hitMs = 0;
        DodgePlan plan{};
        bool pressed = false;
        std::uint32_t pressServerMs = 0; // 실제로 누른 순간(봇이 추정한 서버 시각)
        std::uint32_t stampMs = 0;       // 서버에 보낸 시각
        std::uint32_t reportedRttMs = 0; // 누를 때 서버에 마지막으로 알려 준 왕복 시간
        std::optional<HitResult> result;
    };

    std::uint32_t NowMs()
    {
        using namespace std::chrono;
        return static_cast<std::uint32_t>(duration_cast<milliseconds>(SteadyClock::now().time_since_epoch()).count());
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

    // 서버와 같은 규칙으로 "이 회피가 인정되면 무적 구간에 명중이 들어가는가"를 계산한다.
    // 반환: {기대 결과, 판정 경계까지 거리(ms)}
    std::pair<HitResult, int> Expect(const DodgeTrial& trial, const ModeRules& rules, int latencyMs)
    {
        if (!trial.pressed) return {HitResult::Hit, 1000};
        const std::int64_t arrival = static_cast<std::int64_t>(trial.pressServerMs) + latencyMs;
        const std::int64_t allowed = trial.reportedRttMs == 0
                                         ? rules.rewindCapMs
                                         : std::min<std::int64_t>(trial.reportedRttMs / 2 + 30, rules.rewindCapMs);
        const std::int64_t accepted = std::clamp<std::int64_t>(trial.stampMs, arrival - allowed, arrival);
        const std::int64_t before = static_cast<std::int64_t>(trial.hitMs) - accepted; // 회피가 명중보다 얼마나 앞섰나
        const bool dodged = before >= 0 && before <= rules.iframeMs;
        const int margin = static_cast<int>(std::min(std::abs(before), std::abs(before - rules.iframeMs)));
        return {dodged ? HitResult::Dodged : HitResult::Hit, margin};
    }

    class Bot : public std::enable_shared_from_this<Bot>
    {
    public:
        Bot(asio::io_context& io, const Options& options, int index)
            : socket_(io), timer_(io), outSignal_(io), inSignal_(io), options_(options), index_(index),
              teamCode_(options.teamBase + static_cast<std::uint32_t>(index / options.teamSize))
        {
            outSignal_.expires_at(SteadyClock::time_point::max());
            inSignal_.expires_at(SteadyClock::time_point::max());
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
                enter.mode = options_.mode;
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
                rules_ = reply.rules;
                // 첫 시계 추정(왕복 시간을 모르므로 거칠다). S_Pong이 오면 더 정확한 값으로 바꾼다.
                serverOffsetMs_ = static_cast<std::int64_t>(reply.serverTimeMs) - NowMs();
                for (int i = 0; i < reply.count; ++i)
                {
                    PlayerInfo info;
                    std::memcpy(&info, enterBytes.data() + sizeof(reply) + i * sizeof(PlayerInfo), sizeof(info));
                    target_ = info.position;
                }

                asio::co_spawn(socket_.get_executor(), ReadLoop(self), asio::detached);
                asio::co_spawn(socket_.get_executor(), ProcessLoop(self), asio::detached);
                asio::co_spawn(socket_.get_executor(), WriteLoop(self), asio::detached);
                co_await MoveLoop();
            }
            catch (const std::exception& error)
            {
                std::println("bot-{} 오류: {}", index_, error.what());
                failed_ = true;
            }
            stopped_ = true;
            outSignal_.cancel();
            inSignal_.cancel();
            asio::error_code ignored;
            socket_.close(ignored);
        }

        // 정원 초과 여부는 접속 순서로 정해지므로 봇 번호가 아니라 받은 결과로 나눈다. 개수는 main에서 확인한다.
        bool Report(int expectedOthers) const
        {
            bool ok = !failed_;
            std::string problems;
            if (stats_.enterResult == EnterResult::RoomFull)
            {
                // 정원 초과로 거절된 봇: 거절 외에 확인할 것이 없다.
            }
            else
            {
                if (!stats_.entered) ok = false, problems += " 입장 실패";
                if (static_cast<int>(stats_.movedOthers.size()) != expectedOthers)
                    ok = false, problems += std::format(" 다른 봇 이동 {}/{}", stats_.movedOthers.size(), expectedOthers);
                const bool cheater = (options_.cheat || options_.wall) && index_ == 0;
                if (cheater && stats_.corrections == 0) ok = false, problems += options_.wall ? " 기둥 통과가 거부되지 않음" : " 순간이동이 거부되지 않음";
                if (!cheater && stats_.corrections > 0) ok = false, problems += std::format(" 정상 이동 거부 {}회", stats_.corrections);
                if (options_.dodgeTest) ok &= ReportDodges(problems);
            }
            std::println("bot-{} 방 {} {} | 입장 {} 이동묶음 {} 보정 {} RTT {}ms {}", index_, teamCode_, ok ? "PASS" : "FAIL",
                         static_cast<int>(stats_.enterResult), stats_.moveBatches, stats_.corrections, stats_.lastRttMs, problems);
            return ok;
        }

        int Index() const { return index_; }
        bool RoomFull() const { return stats_.enterResult == EnterResult::RoomFull; }

    private:
        bool IsAttacker() const { return options_.dodgeTest && options_.mode == RoomMode::PvP && index_ == 0; }

        std::uint32_t ServerNowMs() const { return static_cast<std::uint32_t>(static_cast<std::int64_t>(NowMs()) + serverOffsetMs_); }

        bool ReportDodges(std::string& problems) const
        {
            if (IsAttacker())
            {
                std::println("bot-{} 공격 {}회, 내 공격 판정 {}건", index_, stats_.attacksSent, stats_.myAttackResults);
                if (stats_.myAttackResults == 0)
                {
                    problems += " 내 공격 판정을 받지 못함";
                    return false;
                }
                return true;
            }

            int decided = 0, mismatched = 0, skipped = 0;
            for (const DodgeTrial& trial : trials_)
            {
                if (!trial.result) continue; // 시험 시간이 끝나 결과를 못 받은 공격
                const auto [expected, margin] = Expect(trial, rules_, options_.latencyMs);
                const bool boundary = margin < 25;
                const bool match = *trial.result == expected;
                const auto Name = [](HitResult r) { return r == HitResult::Dodged ? "회피" : "피격"; };
                std::string when = "누르지 않음";
                if (trial.pressed)
                {
                    when = std::format("누름 명중{:+}ms", static_cast<int>(trial.pressServerMs) - static_cast<int>(trial.hitMs));
                    if (trial.stampMs != trial.pressServerMs)
                        when += std::format(", 보낸 시각 명중{:+}ms", static_cast<int>(trial.stampMs) - static_cast<int>(trial.hitMs));
                }
                std::println("  bot-{} 공격 {} [{}] {} → 서버 {} / 예상 {}{}", index_, trial.attackId, trial.plan.name, when, Name(*trial.result),
                             Name(expected), boundary ? " (경계 25ms 안: 비교 제외)" : match ? " OK" : " 불일치");
                if (boundary) ++skipped;
                else
                {
                    ++decided;
                    if (!match) ++mismatched;
                }
            }
            std::println("bot-{} 회피 시험: 비교 {}건, 불일치 {}건, 경계 제외 {}건 (무적 {}ms, 되감기 상한 {}ms, 지연 {}ms)", index_, decided, mismatched,
                         skipped, rules_.iframeMs, rules_.rewindCapMs, options_.latencyMs);
            if (mismatched > 0) problems += std::format(" 회피 판정 불일치 {}건", mismatched);
            if (decided < 3) problems += std::format(" 비교한 회피 시험이 {}건뿐", decided);
            return mismatched == 0 && decided >= 3;
        }

        std::vector<DodgePlan> Plans() const
        {
            const int iframe = rules_.iframeMs;
            std::vector<DodgePlan> plans{
                {"무적 앞부분", -iframe * 7 / 10, std::nullopt},
                {"명중 직전", -iframe * 3 / 10, std::nullopt},
                {"명중 뒤", 60, std::nullopt},
                {"너무 이름", -iframe - 120, std::nullopt},
                {"회피 없음", std::nullopt, std::nullopt},
            };
            if (options_.dodgeCheat)
            {
                // 명중을 보고 나서 누르고, 누른 시각은 명중 전으로 속인다.
                plans.push_back({"조작: 늦게 누름", 60, -iframe / 2});
                plans.push_back({"조작: 상한 넘게 늦음", rules_.rewindCapMs + 60, -iframe / 2});
            }
            return plans;
        }

        // ---- 인공 지연을 넣은 송수신 ----
        template <net::Packet T>
        void Queue(const T& packet)
        {
            std::vector<std::byte> bytes(sizeof(T));
            std::memcpy(bytes.data(), &packet, sizeof(T));
            outbox_.push_back({SteadyClock::now() + std::chrono::milliseconds(options_.latencyMs), std::move(bytes)});
            outSignal_.cancel();
        }

        struct Delayed
        {
            SteadyClock::time_point due;
            std::vector<std::byte> bytes;
        };

        // 대기열 맨 앞이 준비될 때까지 기다린다. 준비되면 true, 멈추면 false.
        asio::awaitable<bool> WaitFront(std::deque<Delayed>& queue, asio::steady_timer& signal)
        {
            while (!stopped_)
            {
                if (!queue.empty() && SteadyClock::now() >= queue.front().due) co_return true;
                signal.expires_at(queue.empty() ? SteadyClock::time_point::max() : queue.front().due);
                asio::error_code ignored;
                co_await signal.async_wait(asio::redirect_error(asio::use_awaitable, ignored));
            }
            co_return false;
        }

        asio::awaitable<void> WriteLoop([[maybe_unused]] std::shared_ptr<Bot> self)
        {
            try
            {
                while (co_await WaitFront(outbox_, outSignal_))
                {
                    const std::vector<std::byte> bytes = std::move(outbox_.front().bytes);
                    outbox_.pop_front();
                    co_await asio::async_write(socket_, asio::buffer(bytes), asio::use_awaitable);
                }
            }
            catch (const std::exception&)
            {
            }
        }

        asio::awaitable<void> ReadLoop([[maybe_unused]] std::shared_ptr<Bot> self)
        {
            try
            {
                for (;;)
                {
                    auto bytes = co_await ReadPacket(socket_);
                    inbox_.push_back({SteadyClock::now() + std::chrono::milliseconds(options_.latencyMs), std::move(bytes)});
                    inSignal_.cancel();
                }
            }
            catch (const std::exception&)
            {
                // 연결 종료
            }
        }

        asio::awaitable<void> ProcessLoop(std::shared_ptr<Bot> self)
        {
            try
            {
                while (co_await WaitFront(inbox_, inSignal_))
                {
                    const std::vector<std::byte> bytes = std::move(inbox_.front().bytes);
                    inbox_.pop_front();
                    Handle(bytes, self);
                }
            }
            catch (const std::exception& error)
            {
                std::println("bot-{} 수신 처리 오류: {}", index_, error.what());
                failed_ = true;
            }
        }

        void Handle(const std::vector<std::byte>& bytes, const std::shared_ptr<Bot>& self)
        {
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
                    if (entry.playerId != playerId_)
                    {
                        stats_.movedOthers.insert(entry.playerId);
                        target_ = entry.position;
                    }
                }
                break;
            }
            case PacketId::S_PlayerJoined:
                ++stats_.joined;
                if (const auto joined = net::Read<S_PlayerJoined>(bytes)) target_ = joined->player.position;
                break;
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
                    const std::uint32_t rtt = NowMs() - pong->clientTimeMs;
                    stats_.lastRttMs = rtt;
                    // 왕복 시간이 가장 짧았던 측정이 가장 정확하다: 서버 시각 + 왕복/2 = 지금
                    if (rtt <= bestRttMs_)
                    {
                        bestRttMs_ = rtt;
                        serverOffsetMs_ = static_cast<std::int64_t>(pong->serverTimeMs) + rtt / 2 - NowMs();
                    }
                }
                break;
            case PacketId::S_AttackStart:
                if (const auto attack = net::Read<S_AttackStart>(bytes)) OnAttackStart(*attack, self);
                break;
            case PacketId::S_HitResult:
                if (const auto hit = net::Read<S_HitResult>(bytes))
                {
                    if (myAttacks_.contains(hit->attackId)) ++stats_.myAttackResults;
                    if (hit->targetId == playerId_)
                    {
                        const auto trial = std::ranges::find(trials_, hit->attackId, &DodgeTrial::attackId);
                        if (trial != trials_.end()) trial->result = hit->result;
                    }
                }
                break;
            default: break;
            }
        }

        void OnAttackStart(const S_AttackStart& attack, const std::shared_ptr<Bot>& self)
        {
            if (attack.attackerId == playerId_)
            {
                myAttacks_.insert(attack.attackId);
                return;
            }
            if (!options_.dodgeTest || IsAttacker()) return;
            const float distance = std::hypot(position_.x - attack.center.x, position_.y - attack.center.y);
            if (distance > attack.radius) return; // 나를 노린 공격이 아니다

            const std::vector<DodgePlan> plans = Plans();
            DodgeTrial trial;
            trial.attackId = attack.attackId;
            trial.hitMs = attack.hitServerMs;
            trial.plan = plans[trials_.size() % plans.size()];
            trials_.push_back(trial);
            if (!trial.plan.pressOffsetMs) return;

            // 누를 순간(서버 시각) → 내 시계. 이미 지났으면 바로 누른다(지연이 큰 경우).
            const std::int64_t pressServer = static_cast<std::int64_t>(attack.hitServerMs) + *trial.plan.pressOffsetMs;
            const std::int64_t waitMs = std::max<std::int64_t>(0, pressServer - ServerNowMs());
            asio::co_spawn(socket_.get_executor(), PressDodge(self, attack.attackId, waitMs), asio::detached);
        }

        asio::awaitable<void> PressDodge([[maybe_unused]] std::shared_ptr<Bot> self, std::uint32_t attackId, std::int64_t waitMs)
        {
            asio::steady_timer wait(socket_.get_executor());
            wait.expires_after(std::chrono::milliseconds(waitMs));
            co_await wait.async_wait(asio::use_awaitable);
            if (stopped_) co_return;

            const auto trial = std::ranges::find(trials_, attackId, &DodgeTrial::attackId);
            if (trial == trials_.end()) co_return;
            trial->pressed = true;
            trial->pressServerMs = ServerNowMs();
            trial->stampMs = trial->plan.stampOffsetMs ? trial->hitMs + *trial->plan.stampOffsetMs : trial->pressServerMs;
            trial->reportedRttMs = reportedRttMs_;

            auto dodge = Make<C_Dodge>();
            dodge.sequence = ++dodgeSequence_;
            dodge.pressServerMs = trial->stampMs;
            dodge.yaw = 0.f;
            Queue(dodge);
        }

        void SendPing()
        {
            auto ping = Make<C_Ping>();
            ping.clientTimeMs = NowMs();
            ping.lastRttMs = options_.rttLie > 0 ? options_.rttLie : stats_.lastRttMs;
            reportedRttMs_ = ping.lastRttMs;
            Queue(ping);
        }

        asio::awaitable<void> MoveLoop()
        {
            using namespace std::chrono;
            const auto start = steady_clock::now();
            const auto end = start + seconds(options_.seconds);
            std::uint32_t sequence = 0;
            const float phase = static_cast<float>(index_);
            auto nextAttack = start + milliseconds(1500);
            SendPing(); // 시계와 왕복 시간을 빨리 잡는다

            while (steady_clock::now() < end)
            {
                timer_.expires_after(milliseconds(66)); // 약 15Hz
                co_await timer_.async_wait(asio::use_awaitable);

                const float t = duration<float>(steady_clock::now() - start).count();
                const Vec3 previous = position_;
                float yaw = 0.f;
                if (!options_.dodgeTest)
                {
                    const float angle = t * 1.5f + phase; // 반지름 120cm, 약 180cm/s
                    position_ = {spawn_.x + 120.f * std::cos(angle) - 120.f * std::cos(phase), spawn_.y + 120.f * std::sin(angle) - 120.f * std::sin(phase), spawn_.z};
                    yaw = angle * 57.2958f;
                }
                else if (IsAttacker() && target_)
                {
                    // PvP 공격 봇은 상대를 쫓아가 1.5m 안쪽을 유지한다(상대가 회피로 멀어져도 다음 공격이 닿게).
                    // 400cm/s: 달리기 속도 이하라 서버 이동 검증에 걸리지 않는다.
                    const float dx = target_->x - position_.x, dy = target_->y - position_.y;
                    const float distance = std::hypot(dx, dy);
                    if (distance > 150.f)
                    {
                        const float step = std::min(400.f * 0.066f, distance - 150.f);
                        position_.x += dx / distance * step;
                        position_.y += dy / distance * step;
                    }
                }
                // 방어 봇은 제자리에 선다(무적 판정만 보도록 범위 밖으로 나가지 않는다).

                // --wall: 시작 위치에서 남쪽 부서진 기둥 중심을 지나 그 너머까지 300cm/s로 곧장 걷는다.
                // 막히면 서버가 되돌린 위치에서 다시 같은 방향으로 민다.
                if (options_.wall && index_ == 0)
                {
                    const float dx = 100.f - spawn_.x, dy = -650.f - spawn_.y, length = std::hypot(dx, dy);
                    position_ = {previous.x + dx / length * 300.f * 0.066f, previous.y + dy / length * 300.f * 0.066f, spawn_.z};
                }

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
                move.yaw = yaw;
                move.state = options_.dodgeTest ? MoveState::Idle : MoveState::Run;
                Queue(move);

                // --burst: 1초마다 회피 한 번 분량(60cm × 3)을 기다림 없이 연달아 보낸다. 거부되면 안 된다.
                if (options_.burst && sequence % 15 == 7)
                {
                    for (int step = 1; step <= 3; ++step)
                    {
                        move.sequence = ++sequence;
                        move.position.x = position_.x + 60.f * static_cast<float>(step);
                        move.state = MoveState::Dodge;
                        Queue(move);
                    }
                }

                // PvP 공격 봇: 상대를 향해 1.6초마다 근접 공격(회피 재사용 대기 1초보다 길게)
                if (IsAttacker() && target_ && steady_clock::now() >= nextAttack)
                {
                    nextAttack += milliseconds(1600);
                    auto attack = Make<C_Attack>();
                    attack.sequence = ++stats_.attacksSent;
                    attack.pressServerMs = ServerNowMs();
                    attack.yaw = std::atan2(target_->y - position_.y, target_->x - position_.x) * 180.f / std::numbers::pi_v<float>;
                    attack.kind = AttackKind::Slash;
                    Queue(attack);
                }

                if (sequence % 15 == 0) SendPing();
            }
            // 마지막 판정이 도착할 시간을 준다: 판정 유예(상한) + 왕복 지연 + 여유
            timer_.expires_after(milliseconds(rules_.rewindCapMs + 2 * options_.latencyMs + 400));
            co_await timer_.async_wait(asio::use_awaitable);
        }

        tcp::socket socket_;
        asio::steady_timer timer_;
        asio::steady_timer outSignal_, inSignal_; // 대기열에 새 항목이 들어오면 cancel해서 깨운다
        std::deque<Delayed> outbox_, inbox_;
        const Options& options_;
        const int index_;
        const std::uint32_t teamCode_;
        std::uint32_t playerId_ = 0;
        Vec3 spawn_{}, position_{};
        std::optional<Vec3> target_; // 같은 방 다른 플레이어의 마지막 위치(PvP 공격 봇이 노린다)
        ModeRules rules_{};
        std::int64_t serverOffsetMs_ = 0; // 서버 시각 - 내 시각
        std::uint32_t bestRttMs_ = UINT32_MAX;
        std::uint32_t reportedRttMs_ = 0;
        std::uint32_t dodgeSequence_ = 0;
        std::vector<DodgeTrial> trials_;
        std::set<std::uint32_t> myAttacks_;
        Stats stats_;
        bool cheated_ = false;
        bool failed_ = false;
        bool stopped_ = false;
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
        if (key == "--wall") { options.wall = true; continue; }
        if (key == "--burst") { options.burst = true; continue; }
        if (key == "--dodge-test") { options.dodgeTest = true; continue; }
        if (key == "--dodge-cheat") { options.dodgeCheat = true; continue; }
        if (key == "--host") options.host = value;
        else if (key == "--port") options.port = ParseNumber<std::uint16_t>(value, options.port);
        else if (key == "--bots") options.bots = ParseNumber<int>(value, options.bots);
        else if (key == "--team-size") options.teamSize = std::max(1, ParseNumber<int>(value, options.teamSize));
        else if (key == "--seconds") options.seconds = ParseNumber<int>(value, options.seconds);
        else if (key == "--team-base") options.teamBase = ParseNumber<std::uint32_t>(value, options.teamBase);
        else if (key == "--mode") options.mode = value == "pvp" ? RoomMode::PvP : RoomMode::PvE;
        else if (key == "--latency") options.latencyMs = std::max(0, ParseNumber<int>(value, 0));
        else if (key == "--rtt-lie") options.rttLie = ParseNumber<std::uint32_t>(value, 0);
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
    std::println("봇 {}개 접속: {}:{}, {} 방, 팀당 {}명, {}초, 지연 {}ms{}", options.bots, options.host, options.port,
                 options.mode == RoomMode::PvP ? "PvP" : "PvE", options.teamSize, options.seconds, options.latencyMs,
                 options.dodgeTest ? ", 회피 시험" : "");
    io.run();

    bool allOk = true;
    const int capacity = options.mode == RoomMode::PvP ? kMaxPvpPlayers : kMaxRoomPlayers;
    for (int team = 0; team * options.teamSize < options.bots; ++team)
    {
        // 같은 방에 들어가려 한 봇 수 → 입장 성공 수와 정원 초과 수
        const int requested = std::min(options.teamSize, options.bots - team * options.teamSize);
        const int membersInTeam = std::min(requested, capacity);
        int full = 0;
        for (const auto& bot : bots)
        {
            if (bot->Index() / options.teamSize != team) continue;
            allOk &= bot->Report(membersInTeam - 1);
            full += bot->RoomFull() ? 1 : 0;
        }
        if (full != requested - membersInTeam)
        {
            std::println("팀 {} 정원 초과 {}명 (예상 {}명) FAIL", team, full, requested - membersInTeam);
            allOk = false;
        }
    }
    std::println("{}", allOk ? "결과: PASS" : "결과: FAIL");
    return allOk ? EXIT_SUCCESS : EXIT_FAILURE;
}
