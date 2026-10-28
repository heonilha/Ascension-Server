#include "game/RoomManager.h"

#include "game/Room.h"

namespace asc::game
{
    RoomManager::RoomManager(asio::io_context& io) : io_(io) {}

    std::shared_ptr<Room> RoomManager::Reserve(std::uint32_t teamCode)
    {
        std::lock_guard lock(mutex_);
        Entry& entry = rooms_[teamCode];
        if (!entry.room)
        {
            entry.room = std::make_shared<Room>(io_, teamCode);
            entry.room->Start();
        }
        if (entry.reserved >= proto::kMaxRoomPlayers) return nullptr;
        ++entry.reserved;
        return entry.room;
    }

    void RoomManager::Release(std::uint32_t teamCode)
    {
        std::lock_guard lock(mutex_);
        const auto it = rooms_.find(teamCode);
        if (it == rooms_.end()) return;
        if (--it->second.reserved <= 0)
        {
            it->second.room->Stop();
            rooms_.erase(it);
        }
    }

    std::size_t RoomManager::RoomCount() const
    {
        std::lock_guard lock(mutex_);
        return rooms_.size();
    }
}
