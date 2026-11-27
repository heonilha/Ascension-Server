//! 실제 소켓으로 서버에 붙어 보는 시험. 접속 → 로그인 → 입장 → 이동 → 퇴장의 기본 흐름과 거부 응답을 확인한다.
//! 전투·지연 보정·몬스터의 세부 판정은 C++ 봇 시나리오(`Scripts\Test.ps1 -Rust`)가 확인한다.

use asc_game::CollisionMap;
use asc_protocol::*;
use asc_server::room::RoomOptions;
use asc_server::room_manager::RoomManager;
use asc_server::server;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn start(options: RoomOptions) -> (SocketAddr, Arc<RoomManager>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let rooms = Arc::new(RoomManager::new(options));
    tokio::spawn(server::serve(listener, rooms.clone()));
    (address, rooms)
}

fn with_map(practice_monsters: bool) -> RoomOptions {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/CombatLab.txt");
    RoomOptions { practice_monsters, map: Some(Arc::new(CollisionMap::load(&path).unwrap())), ..RoomOptions::default() }
}

struct Client {
    socket: TcpStream,
    player_id: u32,
}

impl Client {
    async fn connect(address: SocketAddr) -> Client {
        let socket = TcpStream::connect(address).await.unwrap();
        socket.set_nodelay(true).unwrap();
        Client { socket, player_id: 0 }
    }

    async fn send(&mut self, bytes: Vec<u8>) {
        self.socket.write_all(&bytes).await.unwrap();
    }

    /// 패킷 하나(헤더 포함 전체 바이트)를 받는다. 5초 안에 안 오거나 끊기면 `None`.
    async fn receive(&mut self) -> Option<Vec<u8>> {
        let read = async {
            let mut bytes = vec![0u8; 4];
            self.socket.read_exact(&mut bytes).await.ok()?;
            let size = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
            bytes.resize(size, 0);
            self.socket.read_exact(&mut bytes[4..]).await.ok()?;
            Some(bytes)
        };
        tokio::time::timeout(Duration::from_secs(5), read).await.ok().flatten()
    }

    /// `T`가 올 때까지 다른 패킷을 건너뛴다. 뒤에 붙은 항목 바이트도 함께 돌려준다.
    async fn expect<T: Packet>(&mut self) -> (T, Vec<u8>) {
        loop {
            let bytes = self.receive().await.unwrap_or_else(|| panic!("{:?}를 받지 못함", T::ID));
            if u16::from_le_bytes([bytes[2], bytes[3]]) == T::ID.raw() {
                let (packet, tail) = read::<T>(&bytes).unwrap();
                return (packet, tail.to_vec());
            }
        }
    }

    async fn login(&mut self, name: &str) -> SLogin {
        let mut login = make::<CLogin>();
        login.version = VERSION;
        copy_name(&mut login.name, name);
        self.send(encode(&login)).await;
        let (reply, _) = self.expect::<SLogin>().await;
        self.player_id = reply.player_id;
        reply
    }

    async fn enter(&mut self, mode: u8, team_code: u32) -> (SEnterRoom, Vec<u8>) {
        self.enter_team(mode, team_code, 0).await
    }

    async fn enter_team(&mut self, mode: u8, team_code: u32, team: u8) -> (SEnterRoom, Vec<u8>) {
        let mut enter = make::<CEnterRoom>();
        enter.team_code = team_code;
        enter.mode = mode;
        enter.team = team;
        self.send(encode(&enter)).await;
        self.expect::<SEnterRoom>().await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_enter_move_and_leave() {
    let (address, rooms) = start(with_map(false)).await;
    let mut alice = Client::connect(address).await;
    assert_eq!(alice.login("alice").await.result, LoginResult::Ok.raw());
    assert_eq!(alice.login("again").await.result, LoginResult::AlreadyLoggedIn.raw());

    let (entered, others) = alice.enter(RoomMode::Practice.raw(), 7).await;
    assert_eq!((entered.result, entered.count, entered.capacity, { entered.team_code }), (EnterResult::Ok.raw(), 0, 3, 7));
    assert!(others.is_empty());
    assert_eq!({ entered.rules }, asc_game::combat_rules::PRACTICE_RULES);
    let spawn = entered.spawn_position;
    assert_eq!(({ spawn.x }, { spawn.y }), (-750.0, -150.0));
    assert_eq!(alice.enter(RoomMode::Practice.raw(), 8).await.0.result, EnterResult::AlreadyInRoom.raw());

    // 두 번째 사람: 먼저 있던 사람 목록을 받고, 먼저 있던 사람은 입장 알림을 받는다
    let mut bob = Client::connect(address).await;
    bob.login("bob").await;
    let (entered, others) = bob.enter(RoomMode::Practice.raw(), 7).await;
    assert_eq!(entered.count, 1);
    let (info, _) = read::<PlayerInfo>(&others).unwrap();
    assert_eq!(({ info.player_id }, name_str(&info.name).as_ref()), (alice.player_id, "alice"));
    let (joined, _) = alice.expect::<SPlayerJoined>().await;
    assert_eq!({ joined.player.player_id }, bob.player_id);

    // 이동: 입력 12틱(+x) → 확정 상태(S_MoveAck)와 다른 사람에게 가는 이동 묶음(S_MoveBatch)
    let mut input = make::<CInput>();
    input.first_sequence = 1;
    input.state = MoveState::Run.raw();
    let commands = [InputCommand { move_x: 1.0, move_y: 0.0, flags: 0, dodge_x: 0.0, dodge_y: 0.0 }; 12];
    alice.send(encode_with_entries(input, &commands)).await;
    let (ack, _) = alice.expect::<SMoveAck>().await;
    assert_eq!({ ack.sequence }, 12);
    // 서버가 같은 이동 계산을 돌렸는지: 로컬에서 같은 입력을 돌린 결과와 비트까지 같아야 한다(클라이언트 예측과 같은 원리)
    let world = asc_movement::World::empty();
    let mut predicted = bytemuck::Zeroable::zeroed();
    let state: &mut MoveSimState = &mut predicted;
    (state.x, state.y) = (-750.0, -150.0);
    for command in &commands {
        asc_movement::step(state, command, &world);
    }
    assert_eq!(ack.state, predicted);
    assert!({ ack.state.x } > -750.0 + 20.0);

    let (batch, entries) = bob.expect::<SMoveBatch>().await;
    assert_eq!(batch.count, 1);
    let (entry, _) = read::<MoveEntry>(&entries).unwrap();
    assert_eq!(({ entry.player_id }, entry.state, { entry.position.x }), (alice.player_id, MoveState::Run.raw(), { ack.state.x }));

    // 왕복 시간 측정
    let mut ping = make::<CPing>();
    ping.client_time_ms = 1234;
    alice.send(encode(&ping)).await;
    assert_eq!({ alice.expect::<SPong>().await.0.client_time_ms }, 1234);

    // 퇴장: 남은 사람에게 알리고, 모두 나가면 방이 없어진다
    drop(alice);
    let (left, _) = bob.expect::<SPlayerLeft>().await;
    assert_ne!({ left.player_id }, bob.player_id);
    assert_eq!(rooms.room_count(), 1);
    drop(bob);
    for _ in 0..100 {
        if rooms.room_count() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("방이 닫히지 않음");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejections() {
    let (address, _rooms) = start(RoomOptions::default()).await;

    // 버전이 다르면 로그인 거부, 로그인 전에는 입장 거부
    let mut old = Client::connect(address).await;
    let mut login = make::<CLogin>();
    login.version = VERSION + 1;
    old.send(encode(&login)).await;
    let (reply, _) = old.expect::<SLogin>().await;
    assert_eq!((reply.result, { reply.server_version }), (LoginResult::VersionMismatch.raw(), VERSION));
    assert_eq!(old.enter(RoomMode::Practice.raw(), 1).await.0.result, EnterResult::NotLoggedIn.raw());

    // 없는 모드
    let mut client = Client::connect(address).await;
    client.login("").await; // 이름이 비면 서버가 붙인다
    assert_eq!(client.enter(9, 1).await.0.result, EnterResult::InvalidMode.raw());

    // 연습 방 정원 3명: 네 번째는 거부. 모드가 다르면 같은 teamCode라도 다른 방이다
    let mut members = Vec::new();
    for i in 0..3 {
        let mut member = Client::connect(address).await;
        member.login(&format!("m{i}")).await;
        let (entered, _) = member.enter(RoomMode::Practice.raw(), 5).await;
        assert_eq!((entered.result, entered.count as usize), (EnterResult::Ok.raw(), i));
        members.push(member);
    }
    assert_eq!(client.enter(RoomMode::Practice.raw(), 5).await.0.result, EnterResult::RoomFull.raw());
    let (entered, _) = client.enter_team(RoomMode::Match.raw(), 5, 1).await;
    assert_eq!((entered.result, entered.capacity, { entered.rules }), (EnterResult::Ok.raw(), 12, asc_game::combat_rules::MATCH_RULES));

    // 한 명이 나가면 자리가 난다
    members.pop();
    let mut late = Client::connect(address).await;
    late.login("late").await;
    let mut result = EnterResult::RoomFull.raw();
    for _ in 0..50 {
        result = late.enter(RoomMode::Practice.raw(), 5).await.0.result;
        if result == EnterResult::Ok.raw() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(result, EnterResult::Ok.raw());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_packets_close_the_connection() {
    let (address, _rooms) = start(RoomOptions::default()).await;

    // 크기가 맞지 않는 패킷
    let mut client = Client::connect(address).await;
    let mut bytes = encode(&make::<CPing>());
    bytes.push(0);
    bytes[0] += 1;
    client.send(bytes).await;
    assert!(client.receive().await.is_none());

    // 서버가 보내는 패킷 번호
    let mut client = Client::connect(address).await;
    client.send(encode(&make::<SPong>())).await;
    assert!(client.receive().await.is_none());

    // 헤더보다 작은 크기
    let mut client = Client::connect(address).await;
    client.send(vec![2, 0, 10, 0]).await;
    assert!(client.receive().await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn practice_room_with_monsters_sends_monster_batches() {
    let (address, _rooms) = start(with_map(true)).await;
    let mut client = Client::connect(address).await;
    client.login("hunter").await;
    client.enter(RoomMode::Practice.raw(), 3).await;
    // 첫 보충은 방이 생기고 1초 뒤다
    let (batch, entries) = client.expect::<SMonsterBatch>().await;
    assert!(batch.count >= 1);
    assert_eq!(entries.len(), batch.count as usize * std::mem::size_of::<MonsterEntry>());
    let (monster, _) = read::<MonsterEntry>(&entries).unwrap();
    assert!(monster.kind <= 2 && { monster.hp } > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn match_checks_teams_and_sends_match_state() {
    let (address, _rooms) = start(with_map(false)).await;
    let mut lone = Client::connect(address).await;
    lone.login("lone").await;
    // 팀 번호는 1~4
    assert_eq!(lone.enter_team(RoomMode::Match.raw(), 9, 0).await.0.result, EnterResult::InvalidTeam.raw());
    assert_eq!(lone.enter_team(RoomMode::Match.raw(), 9, 5).await.0.result, EnterResult::InvalidTeam.raw());

    // 팀 1에 3명까지. 입장하면 팀 정보가 담긴 목록과 매치 상태(대기)를 받는다
    let mut team1 = Vec::new();
    for i in 0..3 {
        let mut member = Client::connect(address).await;
        member.login(&format!("t1-{i}")).await;
        let (entered, others) = member.enter_team(RoomMode::Match.raw(), 9, 1).await;
        assert_eq!((entered.result, entered.count as usize), (EnterResult::Ok.raw(), i));
        if i > 0 {
            assert_eq!(read::<PlayerInfo>(&others).unwrap().0.team, 1);
        }
        let (state, _) = member.expect::<SMatchState>().await;
        assert_eq!((state.phase, state.round), (MatchPhase::Waiting.raw(), 0));
        team1.push(member);
    }
    assert_eq!(lone.enter_team(RoomMode::Match.raw(), 9, 1).await.0.result, EnterResult::TeamFull.raw());

    // 두 번째 팀이 오면 카운트다운
    let (entered, _) = lone.enter_team(RoomMode::Match.raw(), 9, 2).await;
    assert_eq!(entered.result, EnterResult::Ok.raw());
    let spawn = entered.spawn_position;
    let base = asc_game::room_layout::team_spawn_position(2, 0);
    assert_eq!(({ spawn.x }, { spawn.y }), ({ base.x }, { base.y }));
    loop {
        let (state, _) = lone.expect::<SMatchState>().await;
        if state.phase == MatchPhase::Countdown.raw() {
            assert_eq!(state.teams_alive, 2);
            break;
        }
    }
}
