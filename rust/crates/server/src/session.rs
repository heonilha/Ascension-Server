//! 클라이언트 연결 하나.
//!
//! - 읽기 태스크: 패킷을 읽고 해석해서 방에 명령으로 보낸다. 로그인·방 정보는 이 태스크만 가진다.
//! - 쓰기 태스크: 송신 대기열(`SessionHandle::send`)에 쌓인 버퍼를 소켓에 쓴다.
//! - `SessionHandle`은 아무 태스크에서나 쓸 수 있다(방 태스크가 브로드캐스트할 때 등). 기다리지 않는다.

use crate::clock::server_time_ms;
use crate::log;
use crate::room::{RoomCommand, RoomHandle};
use crate::room_manager::RoomManager;
use asc_protocol::*;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};

/// 보낼 패킷 바이트. 여러 세션에 같은 패킷을 보낼 때 복사하지 않도록 공유한다.
pub type Buffer = Arc<[u8]>;

/// 이 수를 넘게 쌓이면 클라이언트가 받지 못하는 것으로 보고 끊는다.
const MAX_QUEUED_BUFFERS: usize = 1024;

struct Shared {
    id: u32,
    queue: mpsc::Sender<Buffer>,
    closed: watch::Sender<bool>,
}

/// 세션에 패킷을 보내거나 끊는 손잡이.
#[derive(Clone)]
pub struct SessionHandle(Arc<Shared>);

impl SessionHandle {
    pub fn id(&self) -> u32 {
        self.0.id
    }

    /// 패킷을 송신 대기열에 넣는다. 대기열이 가득 차면 연결을 끊는다.
    pub fn send(&self, buffer: Buffer) {
        if *self.0.closed.borrow() {
            return;
        }
        if let Err(TrySendError::Full(_)) = self.0.queue.try_send(buffer) {
            log!("세션 {} 송신 대기열 초과 → 연결 종료", self.0.id);
            self.close();
        }
    }

    pub fn send_packet<T: Packet>(&self, packet: &T) {
        self.send(encode(packet).into());
    }

    /// 연결을 끊는다. 읽기·쓰기 태스크가 이 신호를 보고 끝난다.
    pub fn close(&self) {
        self.0.closed.send_replace(true);
    }

    /// 소켓 없이 손잡이만 만든다(시험용). 보낸 버퍼는 돌려준 수신기로 나온다.
    pub fn detached(id: u32, capacity: usize) -> (SessionHandle, mpsc::Receiver<Buffer>, watch::Receiver<bool>) {
        let (queue, receiver) = mpsc::channel(capacity);
        let (closed, closed_receiver) = watch::channel(false);
        (SessionHandle(Arc::new(Shared { id, queue, closed })), receiver, closed_receiver)
    }
}

struct Session {
    handle: SessionHandle,
    rooms: Arc<RoomManager>,
    /// (방, 모드, teamCode, 팀 번호)
    room: Option<(RoomHandle, RoomMode, u32, u8)>,
    name: String,
    logged_in: bool,
}

/// 연결 하나를 끝날 때까지 처리한다.
pub async fn run(socket: TcpStream, remote: SocketAddr, rooms: Arc<RoomManager>, id: u32) {
    log!("세션 {} 접속: {}:{}", id, remote.ip(), remote.port());
    let (mut reader, writer) = socket.into_split();
    let (handle, queue, mut closed) = SessionHandle::detached(id, MAX_QUEUED_BUFFERS);
    let write_task = tokio::spawn(write_loop(writer, queue, closed.clone(), handle.clone()));

    let mut session = Session { handle: handle.clone(), rooms, room: None, name: String::new(), logged_in: false };
    tokio::select! {
        _ = session.read_loop(&mut reader) => {}
        _ = until_closed(&mut closed) => {}
    }
    session.leave_room().await;
    handle.close();
    let _ = write_task.await;
    log!("세션 {} 종료 ({})", id, if session.name.is_empty() { "로그인 전" } else { &session.name });
}

/// 끊으라는 신호가 올 때까지 기다린다.
async fn until_closed(closed: &mut watch::Receiver<bool>) {
    let _ = closed.wait_for(|closed| *closed).await;
}

async fn write_loop(mut writer: OwnedWriteHalf, mut queue: mpsc::Receiver<Buffer>, mut closed: watch::Receiver<bool>, handle: SessionHandle) {
    loop {
        tokio::select! {
            buffer = queue.recv() => {
                let Some(buffer) = buffer else { break };
                if writer.write_all(&buffer).await.is_err() {
                    break;
                }
            }
            _ = until_closed(&mut closed) => break,
        }
    }
    handle.close(); // 쓰기가 실패했으면 읽기 쪽도 끝낸다
    let _ = writer.shutdown().await;
}

impl Session {
    async fn read_loop(&mut self, reader: &mut OwnedReadHalf) {
        const HEADER: usize = std::mem::size_of::<PacketHeader>();
        let id = self.handle.id();
        let mut bytes = vec![0u8; HEADER];
        loop {
            // 1) 헤더 4바이트를 읽고 2) 남은 몸체를 size만큼 읽는다.
            bytes.truncate(HEADER);
            if let Err(error) = reader.read_exact(&mut bytes).await {
                log_read_error(id, &error);
                return;
            }
            let size = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
            let packet_id = u16::from_le_bytes([bytes[2], bytes[3]]);
            if !(HEADER..=MAX_PACKET_SIZE as usize).contains(&size) {
                log!("세션 {id} 잘못된 패킷 크기 {size}");
                return;
            }
            bytes.resize(size, 0);
            if let Err(error) = reader.read_exact(&mut bytes[HEADER..]).await {
                log_read_error(id, &error);
                return;
            }
            if !self.dispatch(packet_id, &bytes).await {
                // 알 수 없는 패킷이거나 크기가 맞지 않는다: 프로토콜 버전이 다른 클라이언트일 가능성이 높다.
                log!("세션 {id} 잘못된 패킷 id={packet_id} size={size} → 연결 종료");
                return;
            }
        }
    }

    /// 패킷 하나를 처리한다. 알 수 없거나 크기가 맞지 않으면 false.
    async fn dispatch(&mut self, packet_id: u16, bytes: &[u8]) -> bool {
        let player_id = self.handle.id();
        let Some(packet_id) = PacketId::from_raw(packet_id) else { return false };
        match packet_id {
            PacketId::CLogin => {
                let Some(packet) = read_exact::<CLogin>(bytes) else { return false };
                self.handle_login(&packet);
            }
            PacketId::CEnterRoom => {
                let Some(packet) = read_exact::<CEnterRoom>(bytes) else { return false };
                self.handle_enter_room(&packet).await;
            }
            PacketId::CInput => {
                // 가변 길이: 고정 부분 + InputCommand[count]
                let Some((packet, mut tail)) = read::<CInput>(bytes) else { return false };
                let count = packet.count as usize;
                if count == 0 || count > MAX_INPUTS_PER_PACKET as usize || tail.len() != count * std::mem::size_of::<InputCommand>() {
                    return false;
                }
                let mut commands = Vec::with_capacity(count);
                while let Some((command, rest)) = read::<InputCommand>(tail) {
                    commands.push(command);
                    tail = rest;
                }
                self.to_room(RoomCommand::Input { player_id, packet, commands }).await;
            }
            PacketId::CPing => {
                let Some(packet) = read_exact::<CPing>(bytes) else { return false };
                let mut pong = make::<SPong>();
                pong.client_time_ms = packet.client_time_ms;
                pong.server_time_ms = server_time_ms();
                self.handle.send_packet(&pong);
                if packet.last_rtt_ms > 0 {
                    self.to_room(RoomCommand::ReportRtt { player_id, rtt_ms: packet.last_rtt_ms }).await;
                }
            }
            PacketId::CDodge => {
                let Some(packet) = read_exact::<CDodge>(bytes) else { return false };
                self.to_room(RoomCommand::Dodge { player_id, packet }).await;
            }
            PacketId::CRespawn => {
                if read_exact::<CRespawn>(bytes).is_none() {
                    return false;
                }
                self.to_room(RoomCommand::Respawn { player_id }).await;
            }
            PacketId::CAttack => {
                let Some(packet) = read_exact::<CAttack>(bytes) else { return false };
                self.to_room(RoomCommand::Attack { player_id, packet }).await;
            }
            _ => return false, // 서버가 보내는 패킷 번호
        }
        true
    }

    /// 방에 들어가 있으면 명령을 보낸다. 방의 대기열이 가득 차 있으면 빌 때까지 이 세션의 읽기가 멈춘다.
    async fn to_room(&self, command: RoomCommand) {
        if let Some((room, _, _, _)) = &self.room {
            room.send(command).await;
        }
    }

    fn handle_login(&mut self, packet: &CLogin) {
        let id = self.handle.id();
        let mut reply = make::<SLogin>();
        reply.server_version = VERSION;
        reply.player_id = id;

        let result = if self.logged_in {
            LoginResult::AlreadyLoggedIn
        } else if packet.version != VERSION {
            LoginResult::VersionMismatch
        } else {
            let mut name = packet.name;
            name[NAME_LENGTH - 1] = 0; // 클라이언트가 널 문자를 빠뜨려도 안전하게
            let name = name_str(&name);
            self.name = if name.is_empty() { format!("player-{id}") } else { name.into_owned() };
            self.logged_in = true;
            log!("세션 {} 로그인: {}", id, self.name);
            LoginResult::Ok
        };
        reply.result = result.raw();
        self.handle.send_packet(&reply);
    }

    async fn handle_enter_room(&mut self, packet: &CEnterRoom) {
        let (team_code, raw_mode) = (packet.team_code, packet.mode);
        let reject = |result: EnterResult| {
            let mut reply = make::<SEnterRoom>();
            reply.result = result.raw();
            reply.team_code = team_code;
            reply.mode = raw_mode;
            self.handle.send(encode_with_entries(reply, &[] as &[PlayerInfo]).into());
        };

        if !self.logged_in {
            return reject(EnterResult::NotLoggedIn);
        }
        if self.room.is_some() {
            return reject(EnterResult::AlreadyInRoom);
        }
        let Some(mode) = RoomMode::from_raw(raw_mode) else {
            return reject(EnterResult::InvalidMode);
        };
        let team = if mode == RoomMode::Match { packet.team } else { 0 };
        if !RoomManager::valid_team(mode, team) {
            return reject(EnterResult::InvalidTeam);
        }
        let room = match self.rooms.reserve(mode, team_code, team) {
            Ok(room) => room,
            Err(result) => return reject(result),
        };
        // 성공 응답(S_EnterRoom)은 방 태스크에서 보낸다
        room.send(RoomCommand::Enter { session: self.handle.clone(), name: self.name.clone(), team }).await;
        self.room = Some((room, mode, team_code, team));
    }

    async fn leave_room(&mut self) {
        if let Some((room, mode, team_code, team)) = self.room.take() {
            room.send(RoomCommand::Leave { player_id: self.handle.id() }).await;
            self.rooms.release(mode, team_code, team);
        }
    }
}

fn log_read_error(id: u32, error: &std::io::Error) {
    if !matches!(error.kind(), ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted) {
        log!("세션 {id} 읽기 오류: {error}");
    }
}
