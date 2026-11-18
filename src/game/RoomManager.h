// teamCode → 층 인스턴스(Room) 대응표.
//
// 정원 확인을 여기서 뮤텍스 안에 끝내기 때문에, 두 명이 동시에 마지막 자리에
// 들어오려 해도 한 명만 성공한다. 방에 들어가는 일 자체는 방 strand에서 처리한다.
#pragma once

#include <asio.hpp>

#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>

namespace asc::game
{
    class Room;

    class RoomManager
    {
    public:
        explicit RoomManager(asio::io_context& io);

        // 자리를 하나 잡는다. 방이 없으면 만들고, 가득 찼으면 nullptr.
        std::shared_ptr<Room> Reserve(std::uint32_t teamCode);
        // Reserve로 잡은 자리를 돌려준다. 마지막 자리면 방을 닫는다.
        void Release(std::uint32_t teamCode);

        std::size_t RoomCount() const;

    private:
        struct Entry
        {
            std::shared_ptr<Room> room;
            int reserved = 0;
        };

        asio::io_context& io_;
        mutable std::mutex mutex_;
        std::unordered_map<std::uint32_t, Entry> rooms_;
    };
}
