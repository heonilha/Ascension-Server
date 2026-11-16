//! 로그. 부르는 쪽(방 틱·세션 태스크)은 줄을 대기열에 넣기만 하고, 전용 스레드가 출력한다.
//!
//! 출력(콘솔·파일·파이프)이 느리거나 막혀도 게임 틱이 멈추지 않게 하기 위해서다.
//! C++ 서버에서 틱 안에서 바로 출력하던 때는 몬스터 48마리 방의 틱이 평균 10ms를 넘겼다(몬스터 계산은 0.1ms대였다).

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;

/// 이 수를 넘게 밀리면 버리고 개수만 센다(메모리를 무한정 쓰지 않는다).
const MAX_QUEUED: usize = 100_000;

enum Message {
    Line(String),
    Flush(mpsc::Sender<()>),
}

struct Logger {
    sender: SyncSender<Message>,
    dropped: AtomicUsize,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn logger() -> &'static Logger {
    LOGGER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUED);
        std::thread::Builder::new().name("log".into()).spawn(move || run(receiver)).expect("log thread");
        Logger { sender, dropped: AtomicUsize::new(0) }
    })
}

fn run(receiver: Receiver<Message>) {
    let stdout = std::io::stdout();
    while let Ok(first) = receiver.recv() {
        let mut out = stdout.lock();
        let mut flushes = Vec::new();
        // 쌓인 줄을 한꺼번에 쓰고 한 번만 내보낸다.
        for message in std::iter::once(first).chain(receiver.try_iter()) {
            match message {
                Message::Line(line) => {
                    let _ = out.write_all(line.as_bytes());
                }
                Message::Flush(done) => flushes.push(done),
            }
        }
        let dropped = logger().dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            let _ = writeln!(out, "[server] 로그 {dropped}줄 버림(출력이 밀림)");
        }
        let _ = out.flush();
        for done in flushes {
            let _ = done.send(());
        }
    }
}

/// 한 줄을 대기열에 넣는다. `log!` 매크로가 부른다.
pub fn write(text: std::fmt::Arguments<'_>) {
    let logger = logger();
    if let Err(TrySendError::Full(_)) = logger.sender.try_send(Message::Line(format!("[server] {text}\n"))) {
        logger.dropped.fetch_add(1, Ordering::Relaxed); // 출력이 오래 막혔다
    }
}

/// 지금까지 쌓인 줄이 모두 출력될 때까지 기다린다. 프로세스를 끝내기 전에 부른다.
pub fn flush() {
    let (done, wait) = mpsc::channel();
    if logger().sender.send(Message::Flush(done)).is_ok() {
        let _ = wait.recv();
    }
}

/// `log!("방 {} 생성", team_code)` → `[server] 방 1 생성`
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(format_args!($($arg)*))
    };
}
