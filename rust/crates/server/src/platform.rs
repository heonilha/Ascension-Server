//! 운영체제별 준비와 데이터 파일 찾기.

use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows {
    #[link(name = "winmm")]
    extern "system" {
        pub fn timeBeginPeriod(period: u32) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        pub fn SetConsoleOutputCP(code_page: u32) -> i32;
    }
}

/// Windows: 타이머 해상도를 1ms로 올리고(기본 약 15.6ms라 20Hz 틱이 흔들린다)
/// 콘솔을 UTF-8로 바꾼다(한글 로그). 다른 운영체제: 할 일 없음.
pub fn init() {
    #[cfg(windows)]
    // SAFETY: 인자가 값뿐인 Win32 함수다. 프로세스가 끝날 때까지 유지하므로 되돌리지 않는다.
    unsafe {
        windows::timeBeginPeriod(1);
        windows::SetConsoleOutputCP(65001);
    }
}

/// 데이터 파일을 찾는다: 현재 폴더 기준 → 실행 파일 폴더에서 위로 올라가며.
/// 저장소 루트에서 실행하든 `rust/target/release`에서 바로 실행하든 같은 파일을 쓴다.
/// 못 찾으면 받은 경로를 그대로 돌려준다(여는 쪽에서 오류를 낸다).
pub fn find_data_file(relative: &str) -> PathBuf {
    if Path::new(relative).exists() {
        return PathBuf::from(relative);
    }
    let mut directory = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    for _ in 0..5 {
        let Some(current) = directory else { break };
        let candidate = current.join(relative);
        if candidate.exists() {
            return candidate;
        }
        directory = current.parent().map(Path::to_path_buf);
    }
    PathBuf::from(relative)
}
