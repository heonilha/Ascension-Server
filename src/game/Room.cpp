#include "game/Room.h"

#include "core/Log.h"
#include "net/Session.h"

#include <algorithm>
#include <cmath>

namespace asc::game
{
    using namespace proto;

    Room::Room(asio::io_context& io, std::uint32_t teamCode)
        : strand_(asio::make_strand(io)), tickTimer_(strand_), teamCode_(teamCode)
    {
    }

    void Room::Start()
    {
        asio::dispatch(strand_, [self = shared_from_this()] {
            if (self->running_) return;
            self->running_ = true;
            asio::co_spawn(self->strand_, self->TickLoop(self), asio::detached);
            Log("방 {} 생성", self->teamCode_);
        });
    }

    void Room::Stop()
    {
        asio::post(strand_, [self = shared_from_this()] {
            self->running_ = false;
            self->tickTimer_.cancel();
            self->members_.clear();
            Log("방 {} 닫힘", self->teamCode_);
        });
    }

    void Room::Enter(std::shared_ptr<net::Session> session)
    {
        asio::post(strand_, [self = shared_from_this(), session = std::move(session)]() mutable {
            self->DoEnter(std::move(session));
        });
    }

    void Room::Leave(std::uint32_t playerId)
    {
        asio::post(strand_, [self = shared_from_this(), playerId] { self->DoLeave(playerId); });
    }

    void Room::Move(std::uint32_t playerId, const C_Move& packet)
    {
        asio::post(strand_, [self = shared_from_this(), playerId, packet] { self->DoMove(playerId, packet); });
    }

    void Room::DoEnter(std::shared_ptr<net::Session> session)
    {
        if (Find(session->Id())) return;
        const int slot = FreeSlot();
        if (slot < 0)
        {
            // RoomManager가 정원을 먼저 확인하므로 여기 오면 버그다.
            Log("방 {} 빈 자리 없음 (세션 {})", teamCode_, session->Id());
            return;
        }

        Member member;
        member.session = session;
        member.playerId = session->Id();
        member.name = session->Name();
        member.slot = slot;
        member.position = SpawnPosition(slot);
        member.lastMoveTime = Clock::now();

        // 새로 온 사람에게: 입장 결과 + 이미 있던 사람들
        std::vector<PlayerInfo> others;
        for (const Member& existing : members_) others.push_back(ToInfo(existing));

        auto reply = Make<S_EnterRoom>();
        reply.result = EnterResult::Ok;
        reply.teamCode = teamCode_;
        reply.floor = 1;
        reply.spawnPosition = member.position;
        reply.spawnYaw = 0.f;
        session->Send(net::ToBuffer(reply, std::span<const PlayerInfo>(others)));

        // 이미 있던 사람들에게: 새로 온 사람
        auto joined = Make<S_PlayerJoined>();
        joined.player = ToInfo(member);
        Broadcast(net::ToBuffer(joined));

        members_.push_back(std::move(member));
        Log("방 {} 입장: {} (슬롯 {}, 인원 {}/{})", teamCode_, session->Name(), slot, members_.size(), kMaxRoomPlayers);
    }

    void Room::DoLeave(std::uint32_t playerId)
    {
        const auto it = std::ranges::find(members_, playerId, &Member::playerId);
        if (it == members_.end()) return;
        Log("방 {} 퇴장: {}", teamCode_, it->name);
        members_.erase(it);

        auto left = Make<S_PlayerLeft>();
        left.playerId = playerId;
        Broadcast(net::ToBuffer(left));
    }

    void Room::DoMove(std::uint32_t playerId, const C_Move& packet)
    {
        Member* member = Find(playerId);
        if (!member) return;

        const auto now = Clock::now();
        const float elapsed = std::chrono::duration<float>(now - member->lastMoveTime).count();
        member->lastMoveTime = now;
        member->moveBudget = std::min(member->moveBudget + kMaxSpeed * elapsed, kMaxBudget);

        const Vec3& to = packet.position;
        const bool finite = std::isfinite(to.x) && std::isfinite(to.y) && std::isfinite(to.z) && std::isfinite(packet.yaw);
        const bool inside = std::abs(to.x) <= kArenaHalfWidth && std::abs(to.y) <= kArenaHalfDepth && to.z > -500.f && to.z < 3000.f;
        const float distance = std::hypot(to.x - member->position.x, to.y - member->position.y);
        const bool reachable = distance <= member->moveBudget + kMoveSlack;

        if (!finite || !inside || !reachable)
        {
            // 거부: 마지막으로 인정한 위치로 되돌리게 한다. 대기 중인 이동이 연달아 거부되므로 0.2초에 한 번만 보낸다.
            if (now - member->lastCorrectionTime > std::chrono::milliseconds(200))
            {
                member->lastCorrectionTime = now;
                auto correction = Make<S_Correction>();
                correction.sequence = packet.sequence;
                correction.position = member->position;
                member->session->Send(net::ToBuffer(correction));
                Log("방 {} 이동 거부: {} (이동 {:.0f}cm, 허용 {:.0f}cm, 범위 {})", teamCode_, member->name, distance, member->moveBudget, inside);
            }
            return;
        }

        member->moveBudget = std::max(0.f, member->moveBudget - distance);
        member->position = to;
        member->velocity = packet.velocity;
        member->yaw = packet.yaw;
        member->state = packet.state;
        member->dirty = true;
    }

    asio::awaitable<void> Room::TickLoop([[maybe_unused]] std::shared_ptr<Room> self)
    {
        auto next = Clock::now();
        while (running_)
        {
            next += kTickInterval;
            // 멈춰 있다 깨어났을 때 밀린 틱을 몰아서 돌지 않는다.
            if (Clock::now() - next > std::chrono::seconds(1)) next = Clock::now();
            tickTimer_.expires_at(next);
            asio::error_code ec;
            co_await tickTimer_.async_wait(asio::redirect_error(asio::use_awaitable, ec));
            if (!running_) break;
            ++tick_;

            std::vector<MoveEntry> entries;
            for (Member& member : members_)
            {
                if (!member.dirty) continue;
                member.dirty = false;
                entries.push_back({member.playerId, member.position, member.velocity, member.yaw, member.state});
            }
            if (entries.empty()) continue;

            // 모두에게 같은 버퍼를 보낸다. 클라이언트는 자기 playerId 항목을 무시한다.
            auto batch = Make<S_MoveBatch>();
            batch.serverTick = tick_;
            Broadcast(net::ToBuffer(batch, std::span<const MoveEntry>(entries)));
        }
    }

    void Room::Broadcast(const net::Buffer& buffer, std::uint32_t exceptPlayerId)
    {
        for (const Member& member : members_)
        {
            if (member.playerId != exceptPlayerId) member.session->Send(buffer);
        }
    }

    Room::Member* Room::Find(std::uint32_t playerId)
    {
        const auto it = std::ranges::find(members_, playerId, &Member::playerId);
        return it != members_.end() ? &*it : nullptr;
    }

    int Room::FreeSlot() const
    {
        for (int slot = 0; slot < kMaxRoomPlayers; ++slot)
        {
            if (std::ranges::none_of(members_, [slot](const Member& m) { return m.slot == slot; })) return slot;
        }
        return -1;
    }

    Vec3 Room::SpawnPosition(int slot)
    {
        // 클라이언트 데모의 시작 위치(-750, 0) 근처에 1.5m 간격으로 나란히 세운다.
        return {-750.f, -150.f + 150.f * static_cast<float>(slot), 100.f};
    }

    PlayerInfo Room::ToInfo(const Member& member)
    {
        PlayerInfo info{};
        info.playerId = member.playerId;
        CopyName(info.name, member.name.c_str());
        info.position = member.position;
        info.yaw = member.yaw;
        return info;
    }
}
