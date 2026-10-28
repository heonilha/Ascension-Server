#include "net/Session.h"

#include "core/Clock.h"
#include "core/Log.h"
#include "game/Room.h"
#include "game/RoomManager.h"

namespace asc::net
{
    using namespace proto;

    Session::Session(asio::ip::tcp::socket socket, game::RoomManager& rooms, std::uint32_t id)
        : socket_(std::move(socket)), writeSignal_(socket_.get_executor()), rooms_(rooms), id_(id)
    {
        writeSignal_.expires_at(asio::steady_timer::time_point::max());
    }

    void Session::Start()
    {
        asio::error_code ec;
        const auto remote = socket_.remote_endpoint(ec);
        Log("세션 {} 접속: {}:{}", id_, remote.address().to_string(), remote.port());

        auto self = shared_from_this();
        asio::co_spawn(socket_.get_executor(), ReadLoop(self), asio::detached);
        asio::co_spawn(socket_.get_executor(), WriteLoop(self), asio::detached);
    }

    void Session::Send(Buffer buffer)
    {
        asio::post(socket_.get_executor(), [self = shared_from_this(), buffer = std::move(buffer)]() mutable {
            if (self->closed_) return;
            if (self->writeQueue_.size() >= kMaxQueuedBuffers)
            {
                Log("세션 {} 송신 대기열 초과 → 연결 종료", self->id_);
                self->DoClose();
                return;
            }
            self->writeQueue_.push_back(std::move(buffer));
            self->writeSignal_.cancel(); // WriteLoop를 깨운다
        });
    }

    void Session::Close()
    {
        asio::dispatch(socket_.get_executor(), [self = shared_from_this()] { self->DoClose(); });
    }

    void Session::DoClose()
    {
        if (closed_) return;
        closed_ = true;
        LeaveRoom();

        asio::error_code ec;
        socket_.shutdown(asio::ip::tcp::socket::shutdown_both, ec);
        socket_.close(ec);
        writeSignal_.cancel();
        Log("세션 {} 종료 ({})", id_, name_.empty() ? "로그인 전" : name_);
    }

    asio::awaitable<void> Session::ReadLoop([[maybe_unused]] std::shared_ptr<Session> self)
    {
        std::vector<std::byte> bytes;
        try
        {
            while (!closed_)
            {
                // 1) 헤더 4바이트를 읽고 2) 남은 몸체를 size만큼 읽는다.
                PacketHeader header{};
                co_await asio::async_read(socket_, asio::buffer(&header, sizeof(header)), asio::use_awaitable);
                if (header.size < sizeof(PacketHeader) || header.size > kMaxPacketSize)
                {
                    Log("세션 {} 잘못된 패킷 크기 {}", id_, header.size);
                    break;
                }

                bytes.resize(header.size);
                std::memcpy(bytes.data(), &header, sizeof(header));
                if (header.size > sizeof(header))
                {
                    co_await asio::async_read(socket_, asio::buffer(bytes.data() + sizeof(header), header.size - sizeof(header)),
                                              asio::use_awaitable);
                }
                Dispatch(header.id, bytes);
            }
        }
        catch (const std::system_error& error)
        {
            if (error.code() != asio::error::eof && error.code() != asio::error::operation_aborted &&
                error.code() != asio::error::connection_reset)
            {
                Log("세션 {} 읽기 오류: {}", id_, error.what());
            }
        }
        DoClose();
    }

    asio::awaitable<void> Session::WriteLoop([[maybe_unused]] std::shared_ptr<Session> self)
    {
        try
        {
            while (!closed_)
            {
                if (writeQueue_.empty())
                {
                    // 보낼 것이 없으면 Send()가 cancel할 때까지 기다린다.
                    asio::error_code ignored;
                    co_await writeSignal_.async_wait(asio::redirect_error(asio::use_awaitable, ignored));
                    continue;
                }
                const Buffer front = writeQueue_.front();
                co_await asio::async_write(socket_, asio::buffer(*front), asio::use_awaitable);
                writeQueue_.pop_front();
            }
        }
        catch (const std::system_error&)
        {
        }
        DoClose();
    }

    void Session::Dispatch(PacketId id, std::span<const std::byte> bytes)
    {
        switch (id)
        {
        case PacketId::C_Login:
            if (const auto packet = Read<C_Login>(bytes)) return HandleLogin(*packet);
            break;
        case PacketId::C_EnterRoom:
            if (const auto packet = Read<C_EnterRoom>(bytes)) return HandleEnterRoom(*packet);
            break;
        case PacketId::C_Move:
            if (const auto packet = Read<C_Move>(bytes))
            {
                if (room_) room_->Move(id_, *packet);
                return;
            }
            break;
        case PacketId::C_Ping:
            if (const auto packet = Read<C_Ping>(bytes))
            {
                auto pong = Make<S_Pong>();
                pong.clientTimeMs = packet->clientTimeMs;
                pong.serverTimeMs = ServerTimeMs();
                Send(ToBuffer(pong));
                return;
            }
            break;
        default:
            break;
        }
        // 알 수 없는 패킷이거나 크기가 맞지 않는다: 프로토콜 버전이 다른 클라이언트일 가능성이 높다.
        Log("세션 {} 잘못된 패킷 id={} size={} → 연결 종료", id_, static_cast<int>(id), bytes.size());
        DoClose();
    }

    void Session::HandleLogin(const C_Login& packet)
    {
        auto reply = Make<S_Login>();
        reply.serverVersion = kVersion;
        reply.playerId = id_;

        if (loggedIn_) reply.result = LoginResult::AlreadyLoggedIn;
        else if (packet.version != kVersion) reply.result = LoginResult::VersionMismatch;
        else
        {
            char name[kNameLength];
            std::memcpy(name, packet.name, kNameLength);
            name[kNameLength - 1] = '\0'; // 클라이언트가 널 문자를 빠뜨려도 안전하게
            name_ = name[0] != '\0' ? std::string(name) : std::format("player-{}", id_);
            loggedIn_ = true;
            reply.result = LoginResult::Ok;
            Log("세션 {} 로그인: {}", id_, name_);
        }
        Send(ToBuffer(reply));
    }

    void Session::HandleEnterRoom(const C_EnterRoom& packet)
    {
        auto Reject = [this, &packet](EnterResult result) {
            auto reply = Make<S_EnterRoom>();
            reply.result = result;
            reply.teamCode = packet.teamCode;
            Send(ToBuffer(reply, std::span<const PlayerInfo>{}));
        };

        if (!loggedIn_) return Reject(EnterResult::NotLoggedIn);
        if (room_) return Reject(EnterResult::AlreadyInRoom);

        auto room = rooms_.Reserve(packet.teamCode);
        if (!room) return Reject(EnterResult::RoomFull);

        room_ = std::move(room);
        teamCode_ = packet.teamCode;
        room_->Enter(shared_from_this()); // 성공 응답(S_EnterRoom)은 방 strand에서 보낸다
    }

    void Session::LeaveRoom()
    {
        if (!room_) return;
        room_->Leave(id_);
        rooms_.Release(teamCode_);
        room_.reset();
    }
}
