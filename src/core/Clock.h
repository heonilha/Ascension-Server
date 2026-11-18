#pragma once

#include <chrono>
#include <cstdint>

namespace asc
{
    // 서버 프로세스 시작 후 경과 밀리초.
    inline std::uint32_t ServerTimeMs()
    {
        using namespace std::chrono;
        static const steady_clock::time_point start = steady_clock::now();
        return static_cast<std::uint32_t>(duration_cast<milliseconds>(steady_clock::now() - start).count());
    }
}
