//! 접속을 받아 세션을 만든다.

use crate::log;
use crate::room_manager::RoomManager;
use crate::session;
use std::sync::Arc;
use tokio::net::TcpListener;

/// 접속을 계속 받는다. 세션마다 태스크를 띄운다. 끝나지 않는다(런타임이 내려가면 같이 끝난다).
///
/// Windows에서 tokio는 SO_REUSEADDR를 켜지 않는다: 이미 떠 있는 서버와 같은 포트를 또 열 수 없다(두 번째 서버는 오류로 끝난다).
pub async fn serve(listener: TcpListener, rooms: Arc<RoomManager>) {
    let mut next_session_id = 0u32;
    loop {
        match listener.accept().await {
            Ok((socket, remote)) => {
                let _ = socket.set_nodelay(true); // 작은 이동 패킷을 모으지 않고 바로 보낸다
                next_session_id = next_session_id.wrapping_add(1);
                tokio::spawn(session::run(socket, remote, rooms.clone(), next_session_id));
            }
            Err(error) => log!("accept 실패: {error}"),
        }
    }
}
