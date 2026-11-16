use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// 서버 프로세스 시작 후 경과 밀리초. 처음 부른 순간이 0이다(main이 시작할 때 한 번 부른다).
pub fn server_time_ms() -> u32 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
}
