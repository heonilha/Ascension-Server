// Rust 서버가 공유 이동 코드(protocol/AscensionMovement.h)를 부르는 C 인터페이스.
// 계산은 전부 헤더에 있고 여기는 형식만 맞춘다. 헤더를 고치면 Rust 쪽은 다시 빌드만 하면 된다.
//
// - 예외는 이 경계를 넘지 않는다(잡아서 실패 값으로 바꾼다).
// - 이동 상태·입력은 프로토콜의 packed 구조체(MoveSimState, InputCommand)를 그대로 주고받는다.
#include "AscensionMovement.h"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>

using namespace asc;

static_assert(sizeof(move::Vec2) == 2 * sizeof(float));

extern "C"
{
    struct AscWorld
    {
        move::World world;
    };

    // Rust의 ffi::ObstacleView와 같은 배치
    struct AscObstacleView
    {
        const char* name;
        std::size_t nameLength;
        const float* points; // x, y가 pointCount번
        std::size_t pointCount;
        float zMin, zMax;
        std::uint8_t blocking;
        std::uint8_t solid;
    };

    // Rust의 ffi::Constants와 같은 배치
    struct AscMoveConstants
    {
        std::int32_t tickRate, dodgeTicks, dodgeCooldownTicks;
        float tick, runSpeed, acceleration, braking, radius, halfHeight, groundZ, dodgeDistance;
        std::uint8_t inputDodge;
        std::uint16_t protocolVersion;
    };

    static void WriteError(char* error, std::size_t capacity, const std::string& message)
    {
        if (error == nullptr || capacity == 0) return;
        const std::size_t length = message.size() < capacity - 1 ? message.size() : capacity - 1;
        std::memcpy(error, message.data(), length);
        error[length] = '\0';
    }

    // 충돌 파일 내용을 읽는다. 실패하면 nullptr을 돌려주고 error에 이유를 쓴다.
    AscWorld* asc_world_parse(const char* text, std::size_t length, char* error, std::size_t errorCapacity) noexcept
    {
        try
        {
            auto* result = new AscWorld{};
            std::string message;
            if (!move::ParseWorld(std::string(text, length), result->world, message))
            {
                delete result;
                WriteError(error, errorCapacity, message);
                return nullptr;
            }
            return result;
        }
        catch (...)
        {
            WriteError(error, errorCapacity, "exception while parsing");
            return nullptr;
        }
    }

    void asc_world_free(AscWorld* world) noexcept
    {
        delete world;
    }

    std::size_t asc_world_obstacle_count(const AscWorld* world) noexcept
    {
        return world->world.obstacles.size();
    }

    // index번 장애물을 out에 채운다. 포인터는 world가 살아 있는 동안 유효하다. 범위 밖이면 0.
    int asc_world_obstacle(const AscWorld* world, std::size_t index, AscObstacleView* out) noexcept
    {
        if (index >= world->world.obstacles.size()) return 0;
        const move::Obstacle& o = world->world.obstacles[index];
        out->name = o.name.data();
        out->nameLength = o.name.size();
        out->points = &o.polygon.data()->x;
        out->pointCount = o.polygon.size();
        out->zMin = o.zMin;
        out->zMax = o.zMax;
        out->blocking = o.blocking ? 1 : 0;
        out->solid = move::Solid(o) ? 1 : 0;
        return 1;
    }

    // 점에서 index번 장애물 경계까지 거리(안쪽이면 음수). outward(float 2개)에는 밀어낼 방향을 쓴다(없으면 0, 0).
    float asc_world_signed_distance(const AscWorld* world, std::size_t index, float x, float y, float* outward) noexcept
    {
        move::Vec2 normal{};
        const float distance = move::SignedDistance({x, y}, world->world.obstacles[index].polygon, &normal);
        if (outward != nullptr)
        {
            outward[0] = normal.x;
            outward[1] = normal.y;
        }
        return distance;
    }

    // 1틱 진행.
    void asc_move_step(proto::MoveSimState* state, const proto::InputCommand* input, const AscWorld* world) noexcept
    {
        move::State s = move::FromProto(*state);
        move::Input in;
        in.moveX = input->moveX;
        in.moveY = input->moveY;
        in.flags = input->flags;
        in.dodgeX = input->dodgeX;
        in.dodgeY = input->dodgeY;
        move::Step(s, in, world->world);
        *state = move::ToProto(s);
    }

    void asc_move_constants(AscMoveConstants* out) noexcept
    {
        out->tickRate = move::kTickRate;
        out->dodgeTicks = move::kDodgeTicks;
        out->dodgeCooldownTicks = move::kDodgeCooldownTicks;
        out->tick = move::kTick;
        out->runSpeed = move::kRunSpeed;
        out->acceleration = move::kAcceleration;
        out->braking = move::kBraking;
        out->radius = move::kRadius;
        out->halfHeight = move::kHalfHeight;
        out->groundZ = move::kGroundZ;
        out->dodgeDistance = move::kDodgeDistance;
        out->inputDodge = move::kInputDodge;
        out->protocolVersion = proto::kVersion;
    }
}
