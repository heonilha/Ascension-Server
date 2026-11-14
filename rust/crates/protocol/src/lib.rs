//! 패킷 정의. 원본은 `protocol/AscensionProtocol.h`(언리얼 클라이언트가 복사해 쓰는 C++ 헤더)이고,
//! 이 파일은 같은 바이트 배치를 Rust로 옮긴 것이다.
//!
//! 규칙
//! - 헤더를 바꾸면 이 파일도 같이 바꾸고 `VERSION`을 올린다.
//! - 모든 구조체는 `#[repr(C, packed)]`이고 크기를 컴파일 시간에 헤더의 `static_assert`와 같은 값으로 확인한다.
//! - 열거형 필드는 받은 값이 범위 밖일 수 있으므로 정수로 두고, `PacketId::from_raw`처럼 변환해서 쓴다.
//! - 바이트 순서는 리틀 엔디언이다.

use bytemuck::{Pod, Zeroable};

#[cfg(not(target_endian = "little"))]
compile_error!("프로토콜은 리틀 엔디언 전용이다.");

pub const VERSION: u16 = 4;
pub const MAX_PACKET_SIZE: u16 = 4096;
/// UTF-8, 널 문자 포함
pub const NAME_LENGTH: usize = 32;
/// PvE 층 인스턴스 최대 인원(팀원 수)
pub const MAX_ROOM_PLAYERS: u8 = 3;
/// PvP 스테이지 최대 인원
pub const MAX_PVP_PLAYERS: u8 = 16;
/// PvE 층 하나의 최대 몬스터 수
pub const MAX_MONSTERS: u8 = 48;
pub const DEFAULT_PORT: u16 = 7777;
/// 서버 틱 20Hz. `SMoveBatch::server_tick` × 이 값 = 서버 시간
pub const SERVER_TICK_MS: u32 = 50;
pub const MAX_INPUTS_PER_PACKET: u8 = 16;

/// 정수로 오가는 열거형. `from_raw`는 모르는 값이면 `None`.
macro_rules! wire_enum {
    ($(#[$m:meta])* $name:ident : $repr:ident { $($(#[$vm:meta])* $variant:ident = $value:literal),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr($repr)]
        pub enum $name { $($(#[$vm])* $variant = $value),* }

        impl $name {
            pub const fn from_raw(raw: $repr) -> Option<Self> {
                match raw {
                    $($value => Some(Self::$variant),)*
                    _ => None,
                }
            }

            pub const fn raw(self) -> $repr {
                self as $repr
            }
        }
    };
}

wire_enum! {
    PacketId: u16 {
        CLogin = 1,
        SLogin = 2,
        CEnterRoom = 3,
        SEnterRoom = 4,
        SPlayerJoined = 5,
        SPlayerLeft = 6,
        // 7(C_Move), 9(S_Correction)는 v3에서 없어졌다
        SMoveBatch = 8,
        CPing = 10,
        SPong = 11,
        CDodge = 12,
        CAttack = 13,
        SAttackStart = 14,
        SHitResult = 15,
        CInput = 16,
        SMoveAck = 17,
        SAttackCancel = 18,
        SMonsterBatch = 19,
        SMonsterHit = 20,
        SWaveSpawn = 21,
        SWaveEnd = 22,
        CRespawn = 23,
        SRespawn = 24,
    }
}

wire_enum! {
    /// 방 종류. 같은 teamCode라도 모드가 다르면 다른 방이다.
    RoomMode: u8 {
        /// 협동 층 인스턴스(최대 3명)
        PvE = 0,
        /// 대전 스테이지(최대 16명)
        PvP = 1,
    }
}

wire_enum! {
    AttackKind: u8 {
        /// 서버가 내는 범위 공격(PvE 내려찍기 시험 패턴, 이후 수문장 패턴)
        Slam = 1,
        /// 플레이어 근접 공격(PvP)
        Slash = 2,
        /// 몬스터 부채꼴 베기
        MonsterArc = 3,
        /// 폭주 망령 돌진
        MonsterCharge = 4,
        /// 플레이어 일반 검기(PvE)
        Wave = 5,
        /// 플레이어 충전 검기(PvE). charge 0~1
        HeavyWave = 6,
    }
}

wire_enum! {
    HitResult: u8 {
        Hit = 0,
        /// 회피 무적 구간 안에서 맞음 → 정확한 회피
        Dodged = 1,
    }
}

wire_enum! {
    /// 클라이언트 애니메이션 상태. 판정에는 쓰지 않고 원격 플레이어 연출에만 쓴다.
    MoveState: u8 {
        Idle = 0,
        Run = 1,
        Attack = 2,
        Heavy = 3,
        Dodge = 4,
        Dead = 5,
    }
}

wire_enum! {
    LoginResult: u8 {
        Ok = 0,
        VersionMismatch = 1,
        AlreadyLoggedIn = 2,
    }
}

wire_enum! {
    EnterResult: u8 {
        Ok = 0,
        RoomFull = 1,
        NotLoggedIn = 2,
        AlreadyInRoom = 3,
        InvalidMode = 4,
    }
}

wire_enum! {
    MonsterState: u8 {
        /// 노릴 플레이어가 없다
        Idle = 0,
        /// 추적
        Chase = 1,
        /// 공격 예고(제자리)
        Tell = 2,
        /// 폭주 망령 돌진
        Dash = 3,
        /// 공격 뒤 빈틈
        Recover = 4,
        /// 피격 경직(넉백 중)
        Stagger = 5,
    }
}

/// 고정 배치 구조체. 대괄호 안은 헤더의 `static_assert(sizeof(...) == N)`과 같은 값이다.
macro_rules! wire_struct {
    ($(#[$m:meta])* $name:ident [$size:expr] { $($(#[$fm:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
        #[repr(C, packed)]
        pub struct $name { $($(#[$fm])* pub $field: $ty),* }

        const _: () = assert!(core::mem::size_of::<$name>() == $size);
    };
}

/// `PacketHeader`로 시작하는 패킷. `make::<T>()`로 헤더가 채워진 빈 패킷을 만든다.
pub trait Packet: Pod {
    const ID: PacketId;
}

macro_rules! packet {
    ($(#[$m:meta])* $name:ident [$size:expr] { $($body:tt)* }) => {
        wire_struct! { $(#[$m])* $name [$size] { header: PacketHeader, $($body)* } }

        impl Packet for $name {
            const ID: PacketId = PacketId::$name;
        }
    };
}

wire_struct! {
    /// 모든 패킷의 머리. `size`는 헤더를 포함한 전체 바이트 수다.
    PacketHeader [4] {
        size: u16,
        /// `PacketId` 값
        id: u16,
    }
}

wire_struct! {
    /// 언리얼 좌표계(cm)
    Vec3 [12] { x: f32, y: f32, z: f32 }
}

wire_struct! {
    PlayerInfo [52] {
        player_id: u32,
        name: [u8; NAME_LENGTH],
        position: Vec3,
        yaw: f32,
    }
}

wire_struct! {
    MoveEntry [33] {
        player_id: u32,
        position: Vec3,
        velocity: Vec3,
        yaw: f32,
        /// `MoveState` 값
        state: u8,
    }
}

wire_struct! {
    /// 방 모드별 전투 판정 규칙. 서버가 정해 `SEnterRoom`으로 알려 준다.
    ModeRules [8] {
        /// 회피 시작부터 무적 시간
        iframe_ms: u16,
        /// 회피 입력 시각을 도착 시각보다 최대 이만큼 과거로 인정한다
        rewind_cap_ms: u16,
        /// 회피 재사용 대기
        dodge_cooldown_ms: u16,
        /// 플레이어 공격 입력 → 서버 명중 시각까지(PvP)
        attack_windup_ms: u16,
    }
}

wire_struct! {
    InputCommand [17] {
        /// 월드 좌표 이동 방향(길이 1 이하)
        move_x: f32,
        move_y: f32,
        /// `asc_movement::INPUT_DODGE`
        flags: u8,
        dodge_x: f32,
        dodge_y: f32,
    }
}

wire_struct! {
    /// 이동 계산 상태(`AscensionMovement.h`의 `move::State`와 같은 내용)
    MoveSimState [28] {
        x: f32,
        y: f32,
        vx: f32,
        vy: f32,
        /// 0 = 회피 아님, 1~39 = 다음에 진행할 회피 틱
        dodge_tick: u16,
        /// 다음 회피까지 남은 틱
        cooldown: u16,
        dodge_x: f32,
        dodge_y: f32,
    }
}

wire_struct! {
    /// 위치는 cm 정수, 방향은 360도를 256단계로 줄여 보낸다.
    MonsterEntry [11] {
        id: u16,
        /// 0 망령 검병, 1 폭주 망령, 2 망령 지휘관
        kind: u8,
        /// `MonsterState` 값
        state: u8,
        x: i16,
        y: i16,
        /// 도 = yaw × 360 / 256
        yaw: u8,
        hp: u16,
    }
}

// ---- 접속 ----
packet! {
    CLogin [38] {
        version: u16,
        name: [u8; NAME_LENGTH],
    }
}

packet! {
    SLogin [11] {
        /// `LoginResult` 값
        result: u8,
        player_id: u32,
        server_version: u16,
    }
}

// ---- 층 인스턴스(방) ----
packet! {
    /// 같은 모드·teamCode를 보낸 플레이어끼리 같은 방에 들어간다.
    CEnterRoom [9] {
        team_code: u32,
        /// `RoomMode` 값
        mode: u8,
    }
}

packet! {
    /// 뒤에 `PlayerInfo[count]`가 붙는다: 이미 방에 있던 다른 플레이어들.
    SEnterRoom [42] {
        /// `EnterResult` 값
        result: u8,
        team_code: u32,
        /// `RoomMode` 값
        mode: u8,
        floor: u16,
        spawn_position: Vec3,
        spawn_yaw: f32,
        capacity: u8,
        rules: ModeRules,
        /// 시계 맞추기 첫 추정값
        server_time_ms: u32,
        count: u8,
    }
}

packet! {
    SPlayerJoined [56] { player: PlayerInfo }
}

packet! {
    SPlayerLeft [8] { player_id: u32 }
}

// ---- 이동 (v3: 서버 권한) ----
packet! {
    /// 뒤에 `InputCommand[count]`가 붙는다. 번호는 `first_sequence`부터 1씩 늘어난다.
    CInput [14] {
        first_sequence: u32,
        /// 바라보는 방향(연출용)
        yaw: f32,
        /// `MoveState` 값(연출용)
        state: u8,
        count: u8,
    }
}

packet! {
    /// 서버가 `sequence`번 입력까지 처리한 결과. 입력을 보낸 본인에게만 보낸다.
    SMoveAck [36] {
        sequence: u32,
        state: MoveSimState,
    }
}

packet! {
    /// 뒤에 `MoveEntry[count]`가 붙는다. 서버 틱마다 바뀐 플레이어만 보낸다.
    SMoveBatch [9] {
        server_tick: u32,
        count: u8,
    }
}

// ---- 지연 측정 ----
packet! {
    CPing [12] {
        client_time_ms: u32,
        /// 직전에 잰 왕복 시간(0 = 아직 없음)
        last_rtt_ms: u32,
    }
}

packet! {
    SPong [12] {
        /// `CPing` 값을 그대로 돌려준다
        client_time_ms: u32,
        /// 서버 시작 후 경과 시간
        server_time_ms: u32,
    }
}

// ---- 전투 (P1: 서버 판정). 시각은 모두 서버 시계 기준이다. ----
packet! {
    CDodge [16] {
        sequence: u32,
        press_server_ms: u32,
        yaw: f32,
    }
}

packet! {
    CAttack [21] {
        sequence: u32,
        press_server_ms: u32,
        yaw: f32,
        /// `AttackKind` 값
        kind: u8,
        /// HeavyWave의 충전량 0~1
        charge: f32,
    }
}

packet! {
    /// 서버가 공격을 시작했다. 클라이언트는 `hit_server_ms`에 맞춰 예고를 재생한다.
    SAttackStart [51] {
        attack_id: u32,
        /// 0 = 서버(몬스터·시험 패턴)
        attacker_id: u32,
        /// `AttackKind` 값
        kind: u8,
        start_server_ms: u32,
        hit_server_ms: u32,
        center: Vec3,
        radius: f32,
        damage: f32,
        /// 0 = 몬스터 공격이 아님
        monster_id: u16,
        /// MonsterArc·MonsterCharge의 방향(도)
        yaw: f32,
        /// MonsterArc의 반각(도). 0이면 원
        half_angle: f32,
    }
}

packet! {
    /// 예고한 공격이 취소됐다(몬스터가 맞아서 경직됨).
    SAttackCancel [8] { attack_id: u32 }
}

// ---- 몬스터 (P1: 서버 권한, PvE 방) ----
packet! {
    /// 뒤에 `MonsterEntry[count]`가 붙는다. 살아 있는 몬스터 전부를 서버 틱마다 보낸다.
    SMonsterBatch [9] {
        server_tick: u32,
        count: u8,
    }
}

packet! {
    /// 몬스터가 피해를 입었다. `hp_after`가 0이면 죽었다.
    SMonsterHit [24] {
        monster_id: u16,
        attacker_id: u32,
        damage: f32,
        hp_after: u16,
        /// 맞은 순간의 몬스터 위치
        x: f32,
        y: f32,
    }
}

packet! {
    /// 플레이어 검기가 나갔다(서버가 판정하는 투사체).
    SWaveSpawn [38] {
        wave_id: u32,
        owner_id: u32,
        x: f32,
        y: f32,
        yaw: f32,
        radius: f32,
        speed: f32,
        life_ms: u16,
        charge: f32,
    }
}

packet! {
    /// 검기가 수명 전에 끝났다(관통 수 소진 또는 벽).
    SWaveEnd [16] {
        wave_id: u32,
        x: f32,
        y: f32,
    }
}

// ---- 사망·부활 ----
packet! {
    CRespawn [4] {}
}

packet! {
    /// 부활: 시작 위치로 옮기고 HP를 채운다.
    SRespawn [24] {
        player_id: u32,
        position: Vec3,
        hp: f32,
    }
}

packet! {
    /// 공격 범위 안에 있던 플레이어 한 명의 판정 결과.
    SHitResult [31] {
        attack_id: u32,
        target_id: u32,
        /// `HitResult` 값
        result: u8,
        damage: f32,
        hp_after: f32,
        hit_server_ms: u32,
        /// 판정에 쓴 회피 시각(0 = 없음)
        dodge_server_ms: u32,
        /// 그 회피를 도착 시각보다 얼마나 과거로 인정했는지
        rewind_ms: u16,
    }
}

/// 헤더가 채워진 빈 패킷을 만든다. 가변 길이 패킷은 항목을 붙인 뒤 `header.size`를 다시 넣는다.
pub fn make<T: Packet>() -> T {
    let mut packet = T::zeroed();
    let header = PacketHeader { size: core::mem::size_of::<T>() as u16, id: T::ID.raw() };
    bytemuck::bytes_of_mut(&mut packet)[..core::mem::size_of::<PacketHeader>()].copy_from_slice(bytemuck::bytes_of(&header));
    packet
}

/// 바이트 앞부분을 `T`로 읽고 나머지를 돌려준다. 길이가 모자라면 `None`.
/// 패킷의 고정 부분과 뒤에 붙은 항목(`PlayerInfo`, `InputCommand` 등)을 차례로 읽는 데 쓴다.
pub fn read<T: Pod>(bytes: &[u8]) -> Option<(T, &[u8])> {
    let size = core::mem::size_of::<T>();
    if bytes.len() < size {
        return None;
    }
    let (head, tail) = bytes.split_at(size);
    Some((bytemuck::pod_read_unaligned(head), tail))
}

/// 이름을 고정 길이 배열에 복사한다. 넘치면 글자 경계에서 자르고 항상 널 문자로 끝낸다.
pub fn copy_name(destination: &mut [u8; NAME_LENGTH], source: &str) {
    *destination = [0; NAME_LENGTH];
    let mut length = source.len().min(NAME_LENGTH - 1);
    while !source.is_char_boundary(length) {
        length -= 1;
    }
    destination[..length].copy_from_slice(&source.as_bytes()[..length]);
}

/// 고정 길이 배열에서 이름을 읽는다(첫 널 문자 앞까지). UTF-8이 아닌 바이트는 대체 문자가 된다.
pub fn name_str(name: &[u8; NAME_LENGTH]) -> std::borrow::Cow<'_, str> {
    let length = name.iter().position(|&b| b == 0).unwrap_or(NAME_LENGTH);
    String::from_utf8_lossy(&name[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_fills_header() {
        let login = make::<CLogin>();
        let bytes = bytemuck::bytes_of(&login);
        assert_eq!(bytes.len(), 38);
        assert_eq!(&bytes[..4], &[38, 0, 1, 0]);
        assert!(bytes[4..].iter().all(|&b| b == 0));

        let hit = make::<SHitResult>();
        assert_eq!({ hit.header.size }, 31);
        assert_eq!(PacketId::from_raw(hit.header.id), Some(PacketId::SHitResult));
    }

    #[test]
    fn fields_sit_at_packed_offsets() {
        // 헤더 주석의 배치를 몇 개 직접 확인한다(정렬 채움이 없어야 한다).
        let mut ack = make::<SMoveAck>();
        ack.sequence = 0x0403_0201;
        ack.state.dodge_tick = 0x0605;
        let bytes = bytemuck::bytes_of(&ack);
        assert_eq!(&bytes[4..8], &[1, 2, 3, 4]);
        assert_eq!(&bytes[8 + 16..8 + 18], &[5, 6]);

        let mut attack = make::<CAttack>();
        attack.kind = AttackKind::HeavyWave.raw();
        attack.charge = 1.0;
        let bytes = bytemuck::bytes_of(&attack);
        assert_eq!(bytes[16], 6);
        assert_eq!(&bytes[17..21], &1.0f32.to_le_bytes());
    }

    #[test]
    fn read_splits_fixed_part_and_entries() {
        let mut batch = make::<SMoveBatch>();
        batch.count = 2;
        let entry = MoveEntry { player_id: 7, position: Vec3 { x: 1.0, y: 2.0, z: 3.0 }, velocity: Vec3::zeroed(), yaw: 90.0, state: MoveState::Run.raw() };
        let mut bytes = bytemuck::bytes_of(&batch).to_vec();
        bytes.extend_from_slice(bytemuck::bytes_of(&entry));
        bytes.extend_from_slice(bytemuck::bytes_of(&entry));

        let (head, tail) = read::<SMoveBatch>(&bytes).unwrap();
        assert_eq!(head.count, 2);
        let (first, tail) = read::<MoveEntry>(tail).unwrap();
        let (second, tail) = read::<MoveEntry>(tail).unwrap();
        assert_eq!(first, entry);
        assert_eq!(second, entry);
        assert!(read::<MoveEntry>(tail).is_none());
    }

    #[test]
    fn unknown_enum_values_are_rejected() {
        assert_eq!(PacketId::from_raw(7), None);
        assert_eq!(PacketId::from_raw(25), None);
        assert_eq!(RoomMode::from_raw(2), None);
        assert_eq!(AttackKind::from_raw(0), None);
    }

    #[test]
    fn names_are_truncated_and_terminated() {
        let mut name = [0xffu8; NAME_LENGTH];
        copy_name(&mut name, "player1");
        assert_eq!(name_str(&name), "player1");
        assert!(name[7..].iter().all(|&b| b == 0));

        // 한글 11자 = 33바이트 → 31바이트 안에 들어가는 10자까지만
        copy_name(&mut name, "가나다라마바사아자차카");
        assert_eq!(name_str(&name), "가나다라마바사아자차");
        assert_eq!(name[NAME_LENGTH - 1], 0);
    }
}
