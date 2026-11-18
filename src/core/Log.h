// 여러 스레드에서 줄이 섞이지 않게 출력하는 간단한 로그.
#pragma once

#include <format>
#include <mutex>
#include <print>
#include <string>

namespace asc
{
    inline std::mutex& LogMutex()
    {
        static std::mutex mutex;
        return mutex;
    }

    template <class... Args>
    void Log(std::format_string<Args...> format, Args&&... args)
    {
        const std::string line = std::format(format, std::forward<Args>(args)...);
        std::lock_guard lock(LogMutex());
        std::println("[server] {}", line);
        std::fflush(stdout);
    }
}
