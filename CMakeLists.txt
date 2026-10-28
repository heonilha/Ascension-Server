cmake_minimum_required(VERSION 3.25)
project(AscensionServer LANGUAGES CXX)

# CMake 3.30 미만은 GCC의 C++26 플래그를 모르므로 직접 지정한다.
add_compile_options(-std=c++26)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON) # VSCode clangd/IntelliSense용

if(NOT CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    message(WARNING "이 프로젝트는 GCC 기준입니다. 현재 컴파일러: ${CMAKE_CXX_COMPILER_ID}")
elseif(CMAKE_CXX_COMPILER_VERSION VERSION_LESS 16)
    message(WARNING "GCC ${CMAKE_CXX_COMPILER_VERSION}: C++26 리플렉션·계약은 GCC 16 이상에서 사용할 수 있습니다.")
endif()

# ---- 외부 라이브러리: standalone asio (헤더 전용) ----
include(FetchContent)
FetchContent_Declare(asio
    GIT_REPOSITORY https://github.com/chriskohlhoff/asio.git
    GIT_TAG asio-1-38-2
    GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(asio)

add_library(asio INTERFACE)
target_include_directories(asio SYSTEM INTERFACE ${asio_SOURCE_DIR}/asio/include)
target_compile_definitions(asio INTERFACE ASIO_STANDALONE ASIO_NO_DEPRECATED)
find_package(Threads REQUIRED)
target_link_libraries(asio INTERFACE Threads::Threads)

# ---- 공유 프로토콜 (클라이언트와 같은 파일) ----
add_library(ascension_protocol INTERFACE)
target_include_directories(ascension_protocol INTERFACE protocol)

# ---- 서버 코어 ----
add_library(ascension_core STATIC
    src/net/Session.cpp
    src/net/Server.cpp
    src/game/Room.cpp
    src/game/RoomManager.cpp)
target_include_directories(ascension_core PUBLIC src)
target_link_libraries(ascension_core PUBLIC asio ascension_protocol)
target_compile_options(ascension_core PUBLIC -Wall -Wextra -Wpedantic)

add_executable(ascension_server src/main.cpp)
target_link_libraries(ascension_server PRIVATE ascension_core)

# ---- 도구: 봇 클라이언트 ----
add_executable(ascension_bot tools/bot/main.cpp)
target_link_libraries(ascension_bot PRIVATE ascension_core)
