//! ascension-server [--port 7777] [--threads N] [--pve-monsters 1] [--pve-slam-ms 0] [--map data/CombatLab.txt]
//!   --pve-monsters: PvE 방에 서버 몬스터를 둔다(기본 켬). 0이면 빈 방
//!   --pve-slam-ms: PvE 방에서 이 주기로 서버 내려찍기 시험 패턴을 낸다(회피 판정 시험용, 기본 끔)
//!   --map: 충돌 지형 파일. 기본값은 현재 폴더나 실행 파일 위쪽 폴더에서 찾는다. none이면 지형 검사를 끈다

use asc_game::CollisionMap;
use asc_server::room::RoomOptions;
use asc_server::room_manager::RoomManager;
use asc_server::{clock, log, platform, server};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    platform::init();
    clock::server_time_ms(); // 서버 시계를 지금부터 센다
    let code = run();
    log::flush();
    code
}

fn run() -> ExitCode {
    let mut port = asc_protocol::DEFAULT_PORT;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut options = RoomOptions { pve_slam_interval_ms: 0, pve_monsters: true, map: None };
    let mut map_path: Option<PathBuf> = Some(platform::find_data_file("data/CombatLab.txt"));

    let args: Vec<String> = std::env::args().skip(1).collect();
    for [key, value] in args.as_chunks::<2>().0 {
        let value = value.as_str();
        match key.as_str() {
            "--port" => port = value.parse().unwrap_or(port),
            "--threads" => threads = value.parse().unwrap_or(threads).max(1),
            "--pve-slam-ms" => options.pve_slam_interval_ms = value.parse().unwrap_or(0),
            "--pve-monsters" => options.pve_monsters = value.parse::<i32>().unwrap_or(1) != 0,
            "--map" => map_path = (value != "none").then(|| PathBuf::from(value)),
            _ => {}
        }
    }

    match &map_path {
        Some(path) => match CollisionMap::load(path) {
            Ok(map) => {
                log!("충돌 지형: {} (장애물 {}개)", path.display(), map.obstacle_count());
                options.map = Some(Arc::new(map));
            }
            Err(error) => {
                log!("충돌 지형을 읽지 못함: {error} (--map 경로를 지정, 끄려면 --map none)");
                return ExitCode::FAILURE;
            }
        },
        None => log!("충돌 지형 끔(--map none): 이동·공격을 지형으로 검사하지 않음"),
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            log!("치명적 오류: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => listener,
            Err(error) => {
                log!("치명적 오류: 포트 {port}를 열 수 없음: {error}");
                return ExitCode::FAILURE;
            }
        };
        log!("Ascension 서버 시작(Rust): 포트 {}, 스레드 {}, 프로토콜 v{}", port, threads, asc_protocol::VERSION);
        let rooms = Arc::new(RoomManager::new(options));
        tokio::select! {
            _ = server::serve(listener, rooms) => {}
            _ = tokio::signal::ctrl_c() => log!("종료 신호 수신"),
        }
        ExitCode::SUCCESS
    })
}
