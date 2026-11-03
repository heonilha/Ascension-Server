#include "game/RoomManager.h"

#include "game/CombatRules.h"

namespace asc::game
{
    RoomManager::RoomManager(asio::io_context& io, RoomOptions options) : io_(io), options_(options) {}

    std::shared_ptr<Room> RoomManager::Reserve(proto::RoomMode mode, std::uint32_t teamCode)
    {
        std::lock_guard lock(mutex_);
        Entry& entry = rooms_[Key(mode, teamCode)];
        if (!entry.room)
        {
            entry.room = std::make_shared<Room>(io_, mode, teamCode, options_);
            entry.room->Start();
        }
        if (entry.reserved >= CapacityFor(mode)) return nullptr;
        ++entry.reserved;
        return entry.room;
    }

    void RoomManager::Release(proto::RoomMode mode, std::uint32_t teamCode)
    {
        std::lock_guard lock(mutex_);
        const auto it = rooms_.find(Key(mode, teamCode));
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
