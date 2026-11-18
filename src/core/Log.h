// 로그. 부르는 쪽(게임 틱·세션 strand)은 줄을 대기열에 넣기만 하고, 전용 스레드가 출력한다.
//
// 출력(콘솔·파일·파이프)이 느리거나 막혀도 게임 틱이 멈추지 않게 하기 위해서다.
// 틱 안에서 바로 출력하던 때는 몬스터 48마리 방의 틱이 평균 10ms를 넘겼다(몬스터 계산은 0.1ms대였다).
#pragma once

#include <condition_variable>
#include <cstdio>
#include <deque>
#include <format>
#include <iterator>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace asc
{
    class Logger
    {
    public:
        static Logger& Instance()
        {
            static Logger logger; // 프로세스가 정상 종료할 때 남은 줄을 모두 쓰고 끝난다
            return logger;
        }

        void Write(std::string line)
        {
            {
                std::lock_guard lock(mutex_);
                if (queue_.size() >= kMaxQueued)
                {
                    ++dropped_; // 출력이 오래 막혔다: 메모리를 무한정 쓰지 않도록 버리고 개수만 센다
                    return;
                }
                queue_.push_back(std::move(line));
            }
            wake_.notify_one();
        }

        Logger(const Logger&) = delete;
        Logger& operator=(const Logger&) = delete;

    private:
        static constexpr std::size_t kMaxQueued = 100'000;

        Logger() : thread_([this](std::stop_token stop) { Run(stop); }) {}

        ~Logger()
        {
            thread_.request_stop();
            wake_.notify_one();
            // jthread 소멸자가 join한다. Run은 멈추기 전에 남은 줄을 다 쓴다.
        }

        void Run(std::stop_token stop)
        {
            std::vector<std::string> batch;
            for (;;)
            {
                std::size_t dropped = 0;
                {
                    std::unique_lock lock(mutex_);
                    wake_.wait(lock, [&] { return !queue_.empty() || stop.stop_requested(); });
                    batch.assign(std::make_move_iterator(queue_.begin()), std::make_move_iterator(queue_.end()));
                    queue_.clear();
                    dropped = dropped_;
                    dropped_ = 0;
                }
                for (const std::string& line : batch) std::fputs(line.c_str(), stdout);
                if (dropped > 0) std::fputs(std::format("[server] 로그 {}줄 버림(출력이 밀림)\n", dropped).c_str(), stdout);
                std::fflush(stdout);
                if (batch.empty() && stop.stop_requested()) return;
                batch.clear();
            }
        }

        std::mutex mutex_;
        std::condition_variable wake_;
        std::deque<std::string> queue_;
        std::size_t dropped_ = 0;
        std::jthread thread_; // 마지막에 선언: 다른 멤버가 준비된 뒤 시작한다
    };

    template <class... Args>
    void Log(std::format_string<Args...> format, Args&&... args)
    {
        Logger::Instance().Write(std::format("[server] {}\n", std::format(format, std::forward<Args>(args)...)));
    }
}
