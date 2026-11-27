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
    inline constexpr std::uint16_t kVersion = 7;
    inline constexpr std::uint16_t kMaxPacketSize = 4096;
    inline constexpr std::size_t kNameLength = 32;     // UTF-8, 널 문자 포함
    inline constexpr std::uint8_t kMaxPracticePlayers = 3; // 연습 방 최대 인원
    inline constexpr std::uint8_t kMaxTeams = 4;           // 매치 하나의 최대 팀 수(팀 번호 1~4)
    inline constexpr std::uint8_t kMaxTeamPlayers = 3;     // 팀 하나의 최대 인원
    inline constexpr std::uint8_t kMaxMatchPlayers = kMaxTeams * kMaxTeamPlayers; // 매치 최대 인원(12)
    inline constexpr std::uint8_t kMaxMonsters = 48;       // 방 하나의 최대 몬스터 수
    inline constexpr std::uint16_t kDefaultPort = 7777;
    inline constexpr std::uint32_t kServerTickMs = 50; // 서버 틱 20Hz. S_MoveBatch.serverTick × 이 값 = 서버 시간
    inline constexpr std::uint8_t kSkillSlots = 4;         // 배울 수 있는 스킬 수(검·활 합쳐서)
    inline constexpr std::uint8_t kSkillMaxRank = 3;       // 배우기 1 + 강화 2
    inline constexpr std::uint8_t kSkillCount = 24;        // 스킬 번호 1~24(아래 SkillNumber)
    inline constexpr std::uint16_t kStartStat = 5;         // 시작 스탯(힘·민첩·지능·체력 모두)

    enum class PacketId : std::uint16_t
    {
        C_Login = 1,
        S_Login = 2,
        C_EnterRoom = 3,
        S_EnterRoom = 4,
        S_PlayerJoined = 5,
        S_PlayerLeft = 6,
        // 7(C_Move), 9(S_Correction)는 v3에서 입력 전송 + 서버 이동 계산으로 바뀌어 없어졌다
        S_MoveBatch = 8,
        C_Ping = 10,
        S_Pong = 11,
        C_Dodge = 12,
        C_Attack = 13,
        S_AttackStart = 14,
        S_HitResult = 15,
        C_Input = 16,
        S_MoveAck = 17,
        S_AttackCancel = 18,
        S_MonsterBatch = 19,
        S_MonsterHit = 20,
        S_WaveSpawn = 21,
        S_WaveEnd = 22,
        C_Respawn = 23,
        S_Respawn = 24,
        S_MatchState = 25,
        S_PlayerDied = 26,
        S_PlayerProgress = 27,
        S_Teleport = 28,
        S_RiftState = 29,
        C_SkillCommand = 30,
        C_StatCommand = 31,
        S_Loadout = 32,
    };

    // 방 종류. 같은 teamCode라도 모드가 다르면 다른 방이다.
    enum class RoomMode : std::uint8_t
    {
        Practice = 0, // 개발 시험용 연습 방(최대 3명, 같은 편). 게임 기획에는 없다. 회피·몬스터 판정 시험에 쓴다
        Match = 1,    // 배틀로얄 한 판(최대 4팀 × 3명). 몬스터 사냥 + 팀 간 전투 + 라운드·자기장
    };

    enum class AttackKind : std::uint8_t
    {
        Slam = 1,  // 서버가 내는 범위 공격(연습 방 내려찍기 시험 패턴)
        Slash = 2, // 플레이어 근접 공격(매치: 다른 팀 플레이어를 친다)
        MonsterArc = 3,    // 몬스터 부채꼴 베기(center에서 yaw 방향, 반지름 radius, 반각 halfAngle)
        MonsterCharge = 4, // 폭주 망령 돌진(center에서 yaw 방향 직선, hitServerMs에 출발)
        Wave = 5,          // 플레이어 일반 검기(C_Attack). 몬스터를 친다
        HeavyWave = 6,     // 플레이어 충전 검기(C_Attack). charge 0~1
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
        Hidden = 6, // 서버만 쓴다: 풀숲에 숨어 받는 사람에게 보이지 않는다(위치 값은 의미 없음). 다시 보이면 보통 상태로 온다
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
        TeamFull = 5,    // 매치: 그 팀이 이미 3명
        InvalidTeam = 6, // 매치: 팀 번호가 1~4가 아님
    };

    // 매치 진행 단계. S_MatchState로 알린다.
    enum class MatchPhase : std::uint8_t
    {
        Waiting = 0,   // 팀이 모자라 시작 대기
        Countdown = 1, // 시작까지 카운트다운
        Round = 2,     // 라운드 진행(자기장 고정, 4라운드는 계속 축소)
        Shrink = 3,    // 라운드 사이 자기장 축소
        Ended = 4,     // 최후의 1팀 결정. 잠시 뒤 Waiting으로 돌아간다
    };

    // 경험치를 얻은 이유
    enum class ProgressReason : std::uint8_t
    {
        None = 0,    // 입장·초기화 때 현재 값 알림
        Monster = 1, // 몬스터 처치
        Player = 2,  // 플레이어 처치
        Rift = 3,    // 균열 승리·보스 처치 보상(경험치 + 스탯 포인트)
    };

    // 사망 원인
    enum class DeathCause : std::uint8_t
    {
        Player = 0,
        Monster = 1,
        Zone = 2, // 자기장
    };

#pragma pack(push, 1)
    // 균열(2라운드 시작 2분 뒤 열린다). 기준: 11.26 기획안 "균열"
    enum class RiftPhase : std::uint8_t
    {
        Closed = 0,   // 아직 안 열렸거나 끝났다
        Open = 1,     // 입구 2개가 열려 있다(아무도 안 들어감)
        Waiting = 2,  // 한 팀이 들어가 상대를 기다린다(phaseEnd까지 없으면 보스전)
        Pvp = 3,      // 두 팀 대결(자기장이 phase 중간부터 좁아진다)
        Boss = 4,     // 한 팀 보스전
        Finished = 5, // 결과 표시(winnerTeam). 잠시 뒤 Closed
    };

    enum class TeleportReason : std::uint8_t
    {
        RiftEnter = 1,
        RiftExit = 2,
    };

    // 스킬 번호 = 무기 × 12 + 스탯 × 3 + 티어 + 1 (무기 0 검·1 활, 스탯 0 힘·1 민첩·2 지능·3 체력, 티어 0 초급·1 중급·2 고급)
    enum class SkillAction : std::uint8_t
    {
        Learn = 1,   // 스킬 포인트 1. 빈 칸이 있고 해금 조건(스탯 5/15/25, 고급은 12레벨)을 채워야 한다
        Upgrade = 2, // 스킬 포인트 1. 최대 3단계
        Awaken = 3,  // 각성 포인트 1. 3단계 고급 스킬만, 스킬당 1번
        Remove = 4,  // 쓴 포인트를 모두 돌려받는다. 전투 중(최근 5초 안에 주고받은 피해) 불가
    };

    enum class LoadoutResult : std::uint8_t
    {
        Ok = 0,
        NoPoints = 1,
        SlotsFull = 2,
        Locked = 3,        // 스탯·레벨 조건 부족
        NotLearned = 4,
        MaxRank = 5,
        InCombat = 6,
        Invalid = 7,
        AlreadyLearned = 8,
        NotNow = 9,        // 매치가 끝났거나 죽어 있다
    };

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
        std::uint8_t team; // 매치 팀 번호 1~4. 연습 방은 0
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

    // ---- 방 ----
    // 같은 모드·teamCode를 보낸 플레이어끼리 같은 방에 들어간다. 매치에서 teamCode는 "판 번호"이고,
    // 소속 팀은 team(1~4)으로 고른다. 같은 팀끼리는 서로 공격이 맞지 않는다.
    struct C_EnterRoom
    {
        PacketHeader header;
        std::uint32_t teamCode;
        RoomMode mode;
        std::uint8_t team; // 매치: 1~4. 연습 방은 무시
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

    // ---- 이동 (v3: 서버 권한) ----
    // 클라이언트는 60Hz 틱마다 입력 하나를 만들어 몇 개씩 묶어 보낸다. 서버는 AscensionMovement.h로
    // 입력을 하나씩 계산해 위치를 정하고, 마지막으로 처리한 입력 번호와 상태를 S_MoveAck로 돌려준다.
    inline constexpr std::uint8_t kMaxInputsPerPacket = 16;

    struct InputCommand
    {
        float moveX, moveY; // 월드 좌표 이동 방향(길이 1 이하)
        std::uint8_t flags; // move::kInputDodge
        float dodgeX, dodgeY;
    };

    // 뒤에 InputCommand[count]가 붙는다. 번호는 firstSequence부터 1씩 늘어난다.
    struct C_Input
    {
        PacketHeader header;
        std::uint32_t firstSequence;
        float yaw;       // 바라보는 방향(연출용)
        MoveState state; // 애니메이션 상태(연출용)
        std::uint8_t count;
    };

    // 이동 계산 상태(AscensionMovement.h move::State와 같은 내용)
    struct MoveSimState
    {
        float x, y, vx, vy;
        std::uint16_t dodgeTick, cooldown;
        float dodgeX, dodgeY;
    };

    // 서버가 sequence번 입력까지 처리한 결과. 입력을 보낸 본인에게만 보낸다.
    struct S_MoveAck
    {
        PacketHeader header;
        std::uint32_t sequence;
        MoveSimState state;
    };

    // 뒤에 MoveEntry[count]가 붙는다. 서버 틱마다 바뀐 플레이어만 보낸다.
    struct S_MoveBatch
    {
        PacketHeader header;
        std::uint32_t serverTick;
        std::uint8_t count;
    };


    // ---- 지연 측정 ----
    struct C_Ping
    {
        PacketHeader header;
        std::uint32_t clientTimeMs;
        std::uint32_t lastRttMs; // 직전에 잰 왕복 시간(0 = 아직 없음, 잰 뒤에는 1 이상). 서버는 판정 유예 계산에 쓰고 상한으로 제한한다
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
        float charge; // HeavyWave의 충전량 0~1. 서버가 직전 공격 뒤 지난 시간으로 상한을 건다
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
        std::uint16_t monsterId; // 0 = 몬스터 공격이 아님
        float yaw;               // MonsterArc·MonsterCharge의 방향(도)
        float halfAngle;         // MonsterArc의 반각(도). 0이면 원
    };

    // 예고한 공격이 취소됐다(몬스터가 맞아서 경직됨). 클라이언트는 예고 표시를 지운다.
    struct S_AttackCancel
    {
        PacketHeader header;
        std::uint32_t attackId;
    };

    // ---- 몬스터 (서버 권한) ----
    enum class MonsterState : std::uint8_t
    {
        Idle = 0,    // 노릴 플레이어가 없다
        Chase = 1,   // 추적
        Tell = 2,    // 공격 예고(제자리)
        Dash = 3,    // 폭주 망령 돌진
        Recover = 4, // 공격 뒤 빈틈
        Stagger = 5, // 피격 경직(넉백 중)
    };

    // 위치는 cm 정수, 방향은 360도를 256단계로 줄여 보낸다(몬스터 48마리 × 20Hz).
    struct MonsterEntry
    {
        std::uint16_t id;
        std::uint8_t kind; // 0 망령 검병, 1 폭주 망령, 2 망령 지휘관
        MonsterState state;
        std::int16_t x, y;
        std::uint8_t yaw; // 도 = yaw × 360 / 256
        std::uint16_t hp;
    };

    // 뒤에 MonsterEntry[count]가 붙는다. 살아 있는 몬스터 전부를 서버 틱마다 보낸다(목록에 없으면 사라진 것).
    struct S_MonsterBatch
    {
        PacketHeader header;
        std::uint32_t serverTick;
        std::uint8_t count;
    };

    // 몬스터가 피해를 입었다. hpAfter가 0이면 죽었다. 방 전체에 보낸다.
    struct S_MonsterHit
    {
        PacketHeader header;
        std::uint16_t monsterId;
        std::uint32_t attackerId;
        float damage;
        std::uint16_t hpAfter;
        float x, y; // 맞은 순간의 몬스터 위치
    };

    // 플레이어 검기가 나갔다(서버가 판정하는 투사체). 방 전체에 보낸다.
    struct S_WaveSpawn
    {
        PacketHeader header;
        std::uint32_t waveId;
        std::uint32_t ownerId;
        float x, y;
        float yaw;
        float radius;
        float speed;
        std::uint16_t lifeMs;
        float charge;
    };

    // 검기가 수명 전에 끝났다(관통 수 소진 또는 벽).
    struct S_WaveEnd
    {
        PacketHeader header;
        std::uint32_t waveId;
        float x, y;
    };

    // ---- 사망·부활 ----
    // 서버 HP가 0이 되면 그 플레이어는 죽은 상태다(몬스터가 노리지 않고 공격·이동 입력을 무시한다).
    struct C_Respawn
    {
        PacketHeader header;
    };

    // 부활: 위치를 옮기고 HP를 채운다. 방 전체에 보낸다.
    // 매치에서는 서버가 부활 대기(1라운드 10초 / 2라운드 20초 / 3라운드 30초)가 끝나면 자기장 안 안전한 곳에서 스스로 부활시킨다(C_Respawn 무시).
    struct S_Respawn
    {
        PacketHeader header;
        std::uint32_t playerId;
        Vec3 position;
        float hp;
    };

    // ---- 매치 진행 ----
    // 단계가 바뀔 때, 남은 팀 수가 바뀔 때, 입장했을 때, 그리고 2초마다 보낸다.
    // 자기장 반지름은 [phaseStartServerMs, phaseEndServerMs] 동안 zoneRadiusFrom → zoneRadiusTo로 일정하게 줄어든다(같으면 고정).
    struct S_MatchState
    {
        PacketHeader header;
        MatchPhase phase;
        std::uint8_t round;              // 1~4. 시작 전 0
        std::uint32_t phaseStartServerMs;
        std::uint32_t phaseEndServerMs;  // 0 = 끝 시각 없음(대기·4라운드 축소 완료 뒤)
        float zoneX, zoneY;              // 자기장 중심
        float zoneRadiusFrom, zoneRadiusTo;
        float zoneDamagePercent;         // 자기장 밖 피해: 초당 최대 체력의 이 퍼센트(0 = 피해 없음)
        std::uint8_t teamsAlive;         // 탈락하지 않은 팀 수
        std::uint8_t winnerTeam;         // Ended일 때 이긴 팀(0 = 없음)
        std::uint8_t respawnAllowed;     // 1 = 지금 죽으면 부활한다(1~3라운드), 0 = 탈락(4라운드)
    };

    // 플레이어가 죽었다. 방 전체에 보낸다.
    struct S_PlayerDied
    {
        PacketHeader header;
        std::uint32_t playerId;
        std::uint32_t killerId;          // cause가 Player일 때 죽인 플레이어
        DeathCause cause;
        std::uint32_t respawnServerMs;   // 이 시각에 부활한다. 0 = 탈락(다시 부활하지 않음)
    };

    // 레벨·경험치·포인트(방 전체에 보낸다: 다른 사람 레벨도 이름표에 쓴다). 경험치를 얻었을 때, 입장했을 때(방 안 모두의 값),
    // 매치가 시작·초기화될 때(1레벨로) 보낸다. 포인트는 받은 합이다(쓰는 것은 클라이언트 스킬·스탯 화면).
    struct S_PlayerProgress
    {
        PacketHeader header;
        std::uint32_t playerId;
        std::uint8_t level;          // 1~20
        std::uint32_t xp;            // 지금 레벨 안에서 모은 경험치
        std::uint32_t xpToNext;      // 다음 레벨까지 필요한 경험치(최대 레벨이면 0)
        std::uint8_t skillPoints;    // 받은 스킬 포인트 합(1~12레벨에 1개씩)
        std::uint8_t awakeningPoints; // 받은 각성 포인트 합(15·17레벨)
        std::uint16_t statPoints;    // 받은 스탯 포인트 합(레벨업마다 3)
        std::uint32_t gainedXp;      // 이번에 얻은 경험치(0 = 값 알림만)
        ProgressReason reason;
        std::uint8_t levelsGained;   // 이번에 오른 레벨 수
    };

    // 위치만 옮긴다(HP 그대로). 균열 입장·퇴장. 방 전체에 보낸다. 본인은 S_MoveAck의 새 상태로 재조정된다.
    struct S_Teleport
    {
        PacketHeader header;
        std::uint32_t playerId;
        Vec3 position;
        float yaw;
        TeleportReason reason;
    };

    // 균열 상태. 바뀔 때와 2초마다 방 전체에 보낸다(매치만).
    // 입구에 1초 서 있으면 들어간다(입구 하나에 한 팀, 팀마다 입구 하나, 한 판에 한 번).
    // 전투장 자기장: [zoneShrinkStart, zoneShrinkEnd] 동안 arenaRadius → 0. 밖 피해는 zoneLate 전 zoneDamageEarly, 뒤 zoneDamageLate(초당 최대 체력 %).
    struct S_RiftState
    {
        PacketHeader header;
        RiftPhase phase;
        std::uint8_t gateTeam[2];        // 입구를 차지한 팀(0 = 비어 있음)
        float gateX[2], gateY[2];
        float gateRadius;
        float arenaX, arenaY, arenaRadius;
        std::uint32_t phaseStartServerMs;
        std::uint32_t phaseEndServerMs;
        std::uint32_t zoneShrinkStartServerMs; // Pvp만. 0 = 자기장 없음
        std::uint32_t zoneShrinkEndServerMs;
        std::uint32_t zoneLateServerMs;
        float zoneDamageEarly, zoneDamageLate;
        std::uint16_t bossId;            // Boss: 보스 몬스터 id
        std::uint8_t winnerTeam;         // Finished: 이긴 팀(0 = 없음)
        std::uint8_t rewardStats;        // 보상 스탯 포인트(팀원 모두)
        std::uint32_t rewardXp;          // 보상 경험치(팀원 모두, 배율 적용 전)
    };

    // 스킬 배우기·강화·각성·지우기. 서버가 확인하고 S_Loadout으로 답한다.
    struct C_SkillCommand
    {
        PacketHeader header;
        SkillAction action;
        std::uint8_t skillNumber; // 1~24
    };

    // 스탯 포인트 분배(되돌릴 수 없다). 서버가 확인하고 S_Loadout으로 답한다.
    struct C_StatCommand
    {
        PacketHeader header;
        std::uint8_t stat;   // 0 힘, 1 민첩, 2 지능, 3 체력
        std::uint8_t amount; // 1 이상
    };

    struct SkillSlot
    {
        std::uint8_t skillNumber; // 0 = 빈 칸
        std::uint8_t rank;        // 1~3
        std::uint8_t awakened;    // 0·1
    };

    // 본인의 스킬·스탯. 명령의 답, 입장·레벨업·균열 보상·새 판 때 본인에게만 보낸다.
    struct S_Loadout
    {
        PacketHeader header;
        std::uint32_t playerId;
        LoadoutResult result;     // 명령의 답일 때 결과(값 알림은 Ok)
        std::uint8_t action;      // 답한 명령(SkillAction 값, 스탯은 0x10 + stat, 값 알림 0)
        std::uint8_t skillNumber; // 답한 명령의 스킬 번호
        SkillSlot slots[4];
        std::uint16_t stats[4];   // 힘·민첩·지능·체력(시작 5 + 분배)
        std::uint8_t skillPointsLeft;
        std::uint8_t awakeningPointsLeft;
        std::uint16_t statPointsLeft;
    };

    // 공격 범위 안에 있던 플레이어 한 명의 판정 결과. 방 전체에 보낸다.
    // attackId 0 = 자기장 피해(1초마다).
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
    static_assert(sizeof(PlayerInfo) == 53);
    static_assert(sizeof(MoveEntry) == 33);
    static_assert(sizeof(C_Login) == 38);
    static_assert(sizeof(S_Login) == 11);
    static_assert(sizeof(ModeRules) == 8);
    static_assert(sizeof(C_EnterRoom) == 10);
    static_assert(sizeof(S_EnterRoom) == 42);
    static_assert(sizeof(S_PlayerJoined) == 57);
    static_assert(sizeof(S_PlayerLeft) == 8);
    static_assert(sizeof(InputCommand) == 17);
    static_assert(sizeof(C_Input) == 14);
    static_assert(sizeof(MoveSimState) == 28);
    static_assert(sizeof(S_MoveAck) == 36);
    static_assert(sizeof(S_MoveBatch) == 9);
    static_assert(sizeof(C_Ping) == 12);
    static_assert(sizeof(S_Pong) == 12);
    static_assert(sizeof(C_Dodge) == 16);
    static_assert(sizeof(C_Attack) == 21);
    static_assert(sizeof(S_AttackStart) == 51);
    static_assert(sizeof(S_AttackCancel) == 8);
    static_assert(sizeof(MonsterEntry) == 11);
    static_assert(sizeof(S_MonsterBatch) == 9);
    static_assert(sizeof(S_MonsterHit) == 24);
    static_assert(sizeof(S_WaveSpawn) == 38);
    static_assert(sizeof(S_WaveEnd) == 16);
    static_assert(sizeof(C_Respawn) == 4);
    static_assert(sizeof(S_Respawn) == 24);
    static_assert(sizeof(S_HitResult) == 31);
    static_assert(sizeof(S_MatchState) == 37);
    static_assert(sizeof(S_PlayerDied) == 17);
    static_assert(sizeof(S_PlayerProgress) == 27);
    static_assert(sizeof(S_Teleport) == 25);
    static_assert(sizeof(S_RiftState) == 75);
    static_assert(sizeof(C_SkillCommand) == 6);
    static_assert(sizeof(C_StatCommand) == 6);
    static_assert(sizeof(SkillSlot) == 3);
    static_assert(sizeof(S_Loadout) == 35);

    // 패킷 타입 → PacketId
    template <class T> struct IdOf;
    template <> struct IdOf<C_Login> { static constexpr PacketId value = PacketId::C_Login; };
    template <> struct IdOf<S_Login> { static constexpr PacketId value = PacketId::S_Login; };
    template <> struct IdOf<C_EnterRoom> { static constexpr PacketId value = PacketId::C_EnterRoom; };
    template <> struct IdOf<S_EnterRoom> { static constexpr PacketId value = PacketId::S_EnterRoom; };
    template <> struct IdOf<S_PlayerJoined> { static constexpr PacketId value = PacketId::S_PlayerJoined; };
    template <> struct IdOf<S_PlayerLeft> { static constexpr PacketId value = PacketId::S_PlayerLeft; };
    template <> struct IdOf<C_Input> { static constexpr PacketId value = PacketId::C_Input; };
    template <> struct IdOf<S_MoveAck> { static constexpr PacketId value = PacketId::S_MoveAck; };
    template <> struct IdOf<S_AttackCancel> { static constexpr PacketId value = PacketId::S_AttackCancel; };
    template <> struct IdOf<S_MonsterBatch> { static constexpr PacketId value = PacketId::S_MonsterBatch; };
    template <> struct IdOf<S_MonsterHit> { static constexpr PacketId value = PacketId::S_MonsterHit; };
    template <> struct IdOf<S_WaveSpawn> { static constexpr PacketId value = PacketId::S_WaveSpawn; };
    template <> struct IdOf<S_WaveEnd> { static constexpr PacketId value = PacketId::S_WaveEnd; };
    template <> struct IdOf<C_Respawn> { static constexpr PacketId value = PacketId::C_Respawn; };
    template <> struct IdOf<S_Respawn> { static constexpr PacketId value = PacketId::S_Respawn; };
    template <> struct IdOf<S_MoveBatch> { static constexpr PacketId value = PacketId::S_MoveBatch; };
    template <> struct IdOf<C_Ping> { static constexpr PacketId value = PacketId::C_Ping; };
    template <> struct IdOf<S_Pong> { static constexpr PacketId value = PacketId::S_Pong; };
    template <> struct IdOf<C_Dodge> { static constexpr PacketId value = PacketId::C_Dodge; };
    template <> struct IdOf<C_Attack> { static constexpr PacketId value = PacketId::C_Attack; };
    template <> struct IdOf<S_AttackStart> { static constexpr PacketId value = PacketId::S_AttackStart; };
    template <> struct IdOf<S_HitResult> { static constexpr PacketId value = PacketId::S_HitResult; };
    template <> struct IdOf<S_MatchState> { static constexpr PacketId value = PacketId::S_MatchState; };
    template <> struct IdOf<S_PlayerDied> { static constexpr PacketId value = PacketId::S_PlayerDied; };
    template <> struct IdOf<S_PlayerProgress> { static constexpr PacketId value = PacketId::S_PlayerProgress; };
    template <> struct IdOf<S_Teleport> { static constexpr PacketId value = PacketId::S_Teleport; };
    template <> struct IdOf<S_RiftState> { static constexpr PacketId value = PacketId::S_RiftState; };
    template <> struct IdOf<C_SkillCommand> { static constexpr PacketId value = PacketId::C_SkillCommand; };
    template <> struct IdOf<C_StatCommand> { static constexpr PacketId value = PacketId::C_StatCommand; };
    template <> struct IdOf<S_Loadout> { static constexpr PacketId value = PacketId::S_Loadout; };

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
