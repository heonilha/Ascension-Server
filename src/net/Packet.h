// 송신 버퍼와 수신 패킷 해석 도우미.
#pragma once

#include "AscensionProtocol.h"

#include <cstddef>
#include <cstring>
#include <memory>
#include <optional>
#include <span>
#include <type_traits>
#include <vector>

namespace asc::net
{
    // 여러 세션에 같은 패킷을 보낼 때 복사하지 않도록 공유 포인터로 들고 다닌다.
    using Buffer = std::shared_ptr<const std::vector<std::byte>>;

    template <class T>
    concept Packet = std::is_trivially_copyable_v<T> && requires { proto::IdOf<T>::value; };

    // 고정 길이 패킷 하나를 버퍼로 만든다.
    template <Packet T>
    Buffer ToBuffer(const T& packet)
    {
        auto bytes = std::make_shared<std::vector<std::byte>>(sizeof(T));
        std::memcpy(bytes->data(), &packet, sizeof(T));
        return bytes;
    }

    // 고정 부분 + 항목 배열로 이뤄진 가변 길이 패킷을 버퍼로 만든다.
    // count 필드와 header.size는 여기서 채운다.
    template <Packet T, class Entry>
    Buffer ToBuffer(T packet, std::span<const Entry> entries)
    {
        static_assert(std::is_trivially_copyable_v<Entry>);
        const std::size_t total = sizeof(T) + entries.size_bytes();
        packet.header.size = static_cast<std::uint16_t>(total);
        packet.count = static_cast<std::uint8_t>(entries.size());

        auto bytes = std::make_shared<std::vector<std::byte>>(total);
        std::memcpy(bytes->data(), &packet, sizeof(T));
        if (!entries.empty()) std::memcpy(bytes->data() + sizeof(T), entries.data(), entries.size_bytes());
        return bytes;
    }

    // 수신한 바이트를 고정 길이 패킷으로 해석한다. 크기가 다르면 nullopt.
    // 패킹된 구조체라 정렬이 맞지 않을 수 있으므로 포인터 캐스팅 대신 복사한다.
    template <Packet T>
    std::optional<T> Read(std::span<const std::byte> bytes)
    {
        if (bytes.size() != sizeof(T)) return std::nullopt;
        T packet;
        std::memcpy(&packet, bytes.data(), sizeof(T));
        return packet;
    }
}
