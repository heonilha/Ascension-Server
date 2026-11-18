// AscensionProtocol.h
// 서버(GCC, C++26)와 언리얼 클라이언트(MSVC)가 함께 쓰는 패킷 정의.
// 두 컴파일러에서 같은 메모리 배치가 나와야 하므로 이 파일에는 C++20 기능까지만 쓴다.
//
// 규칙
// - 모든 패킷은 PacketHeader로 시작한다. size는 헤더를 포함한 전체 바이트 수다.
// - 바이트 순서는 리틀 엔디언(x64 Windows/Linux 공통)이다.
// - 가변 길이 패킷은 고정 부분 뒤에 count개의 항목이 이어 붙는다.
// - 패킷을 바꾸면 kVersion을 올린다. 버전이 다르면 서버가 로그인을 거부한다.
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace asc::proto
{
    inline constexpr std::uint16_t kVersion = 2;
    inline constexpr std::uint16_t kMaxPacketSize = 4096;
    inline constexpr std::size_t kNameLength = 32;     // UTF-8, 널 문자 포함
    inline constexpr std::uint8_t kMaxRoomPlayers = 3; // PvE 층 인스턴스 최대 인원(팀원 수)
    inline constexpr std::uint8_t kMaxPvpPlayers = 16; // PvP 스테이지 최대 인원
    inline constexpr std::uint16_t kDefaultPort = 7777;
    inline constexpr std::uint32_t kServerTickMs = 50; // 서버 틱 20Hz. S_MoveBatch.serverTick × 이 값 = 서버 시간

    enum class PacketId : std::uint16_t
    {
        C_Login = 1,
        S_Login = 2,
        C_EnterRoom = 3,
        S_EnterRoom = 4,
        S_PlayerJoined = 5,
        S_PlayerLeft = 6,
        C_Move = 7,
        S_MoveBatch = 8,
        S_Correction = 9,
        C_Ping = 10,
        S_Pong = 11,
        C_Dodge = 12,
        C_Attack = 13,
        S_AttackStart = 14,
        S_HitResult = 15,
    };

    // 방 종류. 같은 teamCode라도 모드가 다르면 다른 방이다.
    enum class RoomMode : std::uint8_t
    {
        PvE = 0, // 협동 층 인스턴스(최대 3명)
        PvP = 1, // 대전 스테이지(최대 16명)
    };

    enum class AttackKind : std::uint8_t
    {
        Slam = 1,  // 서버가 내는 범위 공격(PvE 내려찍기 시험 패턴, 이후 수문장 패턴)
        Slash = 2, // 플레이어 근접 공격(PvP)
    };

    enum class HitResult : std::uint8_t
    {
        Hit = 0,
        Dodged = 1, // 회피 무적 구간 안에서 맞음 → 정확한 회피
    };

    // 클라이언트 애니메이션 상태. 판정에는 쓰지 않고 원격 플레이어 연출에만 쓴다.
    enum class MoveState : std::uint8_t
    {
        Idle = 0,
        Run = 1,
        Attack = 2,
        Heavy = 3,
        Dodge = 4,
        Dead = 5,
    };

    enum class LoginResult : std::uint8_t
    {
        Ok = 0,
        VersionMismatch = 1,
        AlreadyLoggedIn = 2,
    };

    enum class EnterResult : std::uint8_t
    {
        Ok = 0,
        RoomFull = 1,
        NotLoggedIn = 2,
        AlreadyInRoom = 3,
        InvalidMode = 4,
    };

#pragma pack(push, 1)
    struct PacketHeader
    {
        std::uint16_t size;
        PacketId id;
    };

    struct Vec3
    {
        float x, y, z; // 언리얼 좌표계(cm)
    };

    struct PlayerInfo
    {
        std::uint32_t playerId;
        char name[kNameLength];
        Vec3 position;
        float yaw;
    };

    struct MoveEntry
    {
        std::uint32_t playerId;
        Vec3 position;
        Vec3 velocity;
        float yaw;
        MoveState state;
    };

    // ---- 접속 ----
    struct C_Login
    {
        PacketHeader header;
        std::uint16_t version;
        char name[kNameLength];
    };

    struct S_Login
    {
        PacketHeader header;
        LoginResult result;
        std::uint32_t playerId;
        std::uint16_t serverVersion;
    };

    // 방 모드별 전투 판정 규칙. 서버가 정해 S_EnterRoom으로 알려 준다.
    struct ModeRules
    {
        std::uint16_t iframeMs;        // 회피 시작부터 무적 시간
        std::uint16_t rewindCapMs;     // 회피 입력 시각을 도착 시각보다 최대 이만큼 과거로 인정한다
        std::uint16_t dodgeCooldownMs; // 회피 재사용 대기
        std::uint16_t attackWindupMs;  // 플레이어 공격 입력 → 서버 명중 시각까지(PvP)
    };

    // ---- 층 인스턴스(방) ----
    // 같은 모드·teamCode를 보낸 플레이어끼리 같은 방에 들어간다.
    struct C_EnterRoom
    {
        PacketHeader header;
        std::uint32_t teamCode;
        RoomMode mode;
    };

    // 뒤에 PlayerInfo[count]가 붙는다: 이미 방에 있던 다른 플레이어들.
    struct S_EnterRoom
    {
        PacketHeader header;
        EnterResult result;
        std::uint32_t teamCode;
        RoomMode mode;
        std::uint16_t floor;
        Vec3 spawnPosition;
        float spawnYaw;
        std::uint8_t capacity;
        ModeRules rules;
        std::uint32_t serverTimeMs; // 시계 맞추기 첫 추정값
        std::uint8_t count;
    };

    struct S_PlayerJoined
    {
        PacketHeader header;
        PlayerInfo player;
    };

    struct S_PlayerLeft
    {
        PacketHeader header;
        std::uint32_t playerId;
    };

    // ---- 이동 ----
    // P0 단계: 클라이언트가 위치를 보내고 서버가 속도·범위를 검증한다.
    // 서버에 충돌 지형이 생기면 입력 전송 + 서버 이동 계산으로 바꾼다.
    struct C_Move
    {
        PacketHeader header;
        std::uint32_t sequence;
        Vec3 position;
        Vec3 velocity;
        float yaw;
        MoveState state;
    };

    // 뒤에 MoveEntry[count]가 붙는다. 서버 틱마다 바뀐 플레이어만 보낸다.
    struct S_MoveBatch
    {
        PacketHeader header;
        std::uint32_t serverTick;
        std::uint8_t count;
    };

    // 서버가 이동을 거부했다. 클라이언트는 이 위치로 되돌린다.
    struct S_Correction
    {
        PacketHeader header;
        std::uint32_t sequence;
        Vec3 position;
    };

    // ---- 지연 측정 ----
    struct C_Ping
    {
        PacketHeader header;
        std::uint32_t clientTimeMs;
        std::uint32_t lastRttMs; // 직전에 잰 왕복 시간(0 = 아직 없음). 서버는 판정 유예 계산에 쓰고 상한으로 제한한다
    };

    struct S_Pong
    {
        PacketHeader header;
        std::uint32_t clientTimeMs; // C_Ping 값을 그대로 돌려준다 → 왕복 시간 계산
        std::uint32_t serverTimeMs; // 서버 시작 후 경과 시간
    };

    // ---- 전투 (P1: 서버 판정) ----
    // 시각은 모두 서버 시계(ServerTimeMs) 기준이다. 클라이언트는 S_Pong으로 추정한 서버 시각을 넣는다.
    // 서버는 누른 시각을 [도착 시각 - 허용 되감기, 도착 시각]으로 제한해서 쓴다.
    struct C_Dodge
    {
        PacketHeader header;
        std::uint32_t sequence;
        std::uint32_t pressServerMs;
        float yaw;
    };

    struct C_Attack
    {
        PacketHeader header;
        std::uint32_t sequence;
        std::uint32_t pressServerMs;
        float yaw;
        AttackKind kind;
    };

    // 서버가 공격을 시작했다. 클라이언트는 hitServerMs에 맞춰 예고를 재생한다(보간 지연 없이).
    struct S_AttackStart
    {
        PacketHeader header;
        std::uint32_t attackId;
        std::uint32_t attackerId; // 0 = 서버(몬스터·시험 패턴)
        AttackKind kind;
        std::uint32_t startServerMs;
        std::uint32_t hitServerMs;
        Vec3 center;
        float radius;
        float damage;
    };

    // 공격 범위 안에 있던 플레이어 한 명의 판정 결과. 방 전체에 보낸다.
    struct S_HitResult
    {
        PacketHeader header;
        std::uint32_t attackId;
        std::uint32_t targetId;
        HitResult result;
        float damage;
        float hpAfter;
        std::uint32_t hitServerMs;
        std::uint32_t dodgeServerMs; // 판정에 쓴 회피 시각(0 = 없음)
        std::uint16_t rewindMs;      // 그 회피를 도착 시각보다 얼마나 과거로 인정했는지
    };
#pragma pack(pop)

    // 두 컴파일러의 배치가 같은지 컴파일 시간에 확인한다.
    static_assert(sizeof(PacketHeader) == 4);
    static_assert(sizeof(Vec3) == 12);
    static_assert(sizeof(PlayerInfo) == 52);
    static_assert(sizeof(MoveEntry) == 33);
    static_assert(sizeof(C_Login) == 38);
    static_assert(sizeof(S_Login) == 11);
    static_assert(sizeof(ModeRules) == 8);
    static_assert(sizeof(C_EnterRoom) == 9);
    static_assert(sizeof(S_EnterRoom) == 42);
    static_assert(sizeof(S_PlayerJoined) == 56);
    static_assert(sizeof(S_PlayerLeft) == 8);
    static_assert(sizeof(C_Move) == 37);
    static_assert(sizeof(S_MoveBatch) == 9);
    static_assert(sizeof(S_Correction) == 20);
    static_assert(sizeof(C_Ping) == 12);
    static_assert(sizeof(S_Pong) == 12);
    static_assert(sizeof(C_Dodge) == 16);
    static_assert(sizeof(C_Attack) == 17);
    static_assert(sizeof(S_AttackStart) == 41);
    static_assert(sizeof(S_HitResult) == 31);

    // 패킷 타입 → PacketId
    template <class T> struct IdOf;
    template <> struct IdOf<C_Login> { static constexpr PacketId value = PacketId::C_Login; };
    template <> struct IdOf<S_Login> { static constexpr PacketId value = PacketId::S_Login; };
    template <> struct IdOf<C_EnterRoom> { static constexpr PacketId value = PacketId::C_EnterRoom; };
    template <> struct IdOf<S_EnterRoom> { static constexpr PacketId value = PacketId::S_EnterRoom; };
    template <> struct IdOf<S_PlayerJoined> { static constexpr PacketId value = PacketId::S_PlayerJoined; };
    template <> struct IdOf<S_PlayerLeft> { static constexpr PacketId value = PacketId::S_PlayerLeft; };
    template <> struct IdOf<C_Move> { static constexpr PacketId value = PacketId::C_Move; };
    template <> struct IdOf<S_MoveBatch> { static constexpr PacketId value = PacketId::S_MoveBatch; };
    template <> struct IdOf<S_Correction> { static constexpr PacketId value = PacketId::S_Correction; };
    template <> struct IdOf<C_Ping> { static constexpr PacketId value = PacketId::C_Ping; };
    template <> struct IdOf<S_Pong> { static constexpr PacketId value = PacketId::S_Pong; };
    template <> struct IdOf<C_Dodge> { static constexpr PacketId value = PacketId::C_Dodge; };
    template <> struct IdOf<C_Attack> { static constexpr PacketId value = PacketId::C_Attack; };
    template <> struct IdOf<S_AttackStart> { static constexpr PacketId value = PacketId::S_AttackStart; };
    template <> struct IdOf<S_HitResult> { static constexpr PacketId value = PacketId::S_HitResult; };

    // 헤더가 채워진 빈 패킷을 만든다. 예: auto p = Make<C_Login>();
    template <class T>
    T Make()
    {
        T packet{};
        packet.header.size = static_cast<std::uint16_t>(sizeof(T));
        packet.header.id = IdOf<T>::value;
        return packet;
    }

    // 이름을 고정 길이 배열에 복사한다. 넘치면 자르고 항상 널 문자로 끝낸다.
    inline void CopyName(char (&destination)[kNameLength], const char* source)
    {
        std::memset(destination, 0, kNameLength);
        if (source != nullptr)
        {
            std::size_t length = std::strlen(source);
            if (length >= kNameLength) length = kNameLength - 1;
            std::memcpy(destination, source, length);
        }
    }
}
