cmake_minimum_required(VERSION 3.25)
project(AscensionServer LANGUAGES CXX)

# 기준 환경은 Windows + MSVC(Visual Studio 2022 이상)다. Linux + GCC 14 이상에서도 빌드된다(ThreadSanitizer 검사용).
# C++23: std::print, std::ranges::contains, 코루틴(asio)을 쓴다.
set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# 실행 파일을 생성기와 구성(Debug/Release)에 상관없이 build/<프리셋>/bin 한곳에 모은다.
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "$<1:${CMAKE_BINARY_DIR}/bin>")

# ---- 외부 라이브러리: standalone asio (헤더 전용. Windows에서는 IOCP, Linux에서는 epoll 위에서 돈다) ----
include(FetchContent)
FetchContent_Declare(asio
    GIT_REPOSITORY https://github.com/chriskohlhoff/asio.git
    GIT_TAG asio-1-38-2
    GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(asio)

add_library(asio INTERFACE)
# asio/include는 include로 가는 심볼릭 링크라 Windows에서 풀리지 않는다. 실제 폴더를 직접 쓴다.
target_include_directories(asio SYSTEM INTERFACE ${asio_SOURCE_DIR}/include)
target_compile_definitions(asio INTERFACE ASIO_STANDALONE ASIO_NO_DEPRECATED)
find_package(Threads REQUIRED)
target_link_libraries(asio INTERFACE Threads::Threads)
if(WIN32)
    # Windows 10 이상 API, windows.h의 min/max 매크로 끄기
    target_compile_definitions(asio INTERFACE _WIN32_WINNT=0x0A00 NOMINMAX WIN32_LEAN_AND_MEAN)
    target_link_libraries(asio INTERFACE winmm) # timeBeginPeriod(타이머 1ms 해상도)
endif()

# ---- 공유 코드 (클라이언트와 같은 파일: 프로토콜, 이동 계산) ----
add_library(ascension_protocol INTERFACE)
target_include_directories(ascension_protocol INTERFACE protocol)

# ---- 서버 코어 ----
add_library(ascension_core STATIC
    src/net/Session.cpp
    src/net/Server.cpp
    src/game/Room.cpp
    src/game/RoomManager.cpp
    src/game/CollisionMap.cpp)
target_include_directories(ascension_core PUBLIC src)
target_link_libraries(ascension_core PUBLIC asio ascension_protocol)
if(MSVC)
    # /utf-8: 소스와 실행 문자열을 UTF-8로(한글 로그). /permissive-: 표준 준수
    target_compile_options(ascension_core PUBLIC /W4 /utf-8 /permissive-)
else()
    target_compile_options(ascension_core PUBLIC -Wall -Wextra -Wpedantic)
endif()

add_executable(ascension_server src/main.cpp)
target_link_libraries(ascension_server PRIVATE ascension_core)

# ---- 도구: 봇 클라이언트 ----
add_executable(ascension_bot tools/bot/main.cpp)
target_link_libraries(ascension_bot PRIVATE ascension_core)

# ---- 시험: 충돌 지형 단위 시험 ----
add_executable(ascension_terrain_test tools/tests/terrain_test.cpp)
target_link_libraries(ascension_terrain_test PRIVATE ascension_core)
