//! ascension-server [--port 7777] [--threads N] [--map data/CombatLab.txt]
//!                  [--match-monsters 1] [--match-time-scale 1] [--match-start-teams 2] [--match-xp-scale 1]
//!                  [--practice-monsters 1] [--practice-slam-ms 0]
//!   --match-monsters: 매치에 서버 몬스터를 둔다(기본 켬). 0이면 플레이어끼리만
//!   --match-time-scale: 매치 시간표(기본 36분)의 모든 시간에 곱한다. 0.1이면 약 3.6분 판(데모·시험용)
//!   --match-start-teams: 이 수 이상의 팀이 들어오면 카운트다운을 시작한다(기본 2, 1이면 혼자 시험)
//!   --match-xp-scale: 처치 경험치 배율(기본 1). 시간을 줄인 데모에서 레벨을 같은 속도로 올리려면 1 / 시간 배율
//!   --practice-monsters: 연습 방(개발 시험용)에 서버 몬스터를 둔다(기본 켬). 0이면 빈 방
//!   --practice-slam-ms: 연습 방에서 이 주기로 서버 내려찍기 시험 패턴을 낸다(회피 판정 시험용, 기본 끔)
//!   --map: 충돌 지형 파일. 기본값은 현재 폴더나 실행 파일 위쪽 폴더에서 찾는다. none이면 지형 검사를 끈다
//!   (예전 이름 --pve-monsters, --pve-slam-ms도 받는다)

use asc_game::match_rules::MatchSchedule;
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
    let mut options = RoomOptions {
        practice_slam_interval_ms: 0,
        practice_monsters: true,
        match_monsters: true,
        schedule: MatchSchedule::default(),
        xp_scale: 1.0,
        map: None,
    };
    let mut time_scale = 1.0f32;
    let mut start_teams = options.schedule.start_teams;
    let mut map_path: Option<PathBuf> = Some(platform::find_data_file("data/CombatLab.txt"));

    let args: Vec<String> = std::env::args().skip(1).collect();
    for [key, value] in args.as_chunks::<2>().0 {
        let value = value.as_str();
        match key.as_str() {
            "--port" => port = value.parse().unwrap_or(port),
            "--threads" => threads = value.parse().unwrap_or(threads).max(1),
            "--practice-slam-ms" | "--pve-slam-ms" => options.practice_slam_interval_ms = value.parse().unwrap_or(0),
            "--practice-monsters" | "--pve-monsters" => options.practice_monsters = value.parse::<i32>().unwrap_or(1) != 0,
            "--match-monsters" => options.match_monsters = value.parse::<i32>().unwrap_or(1) != 0,
            "--match-time-scale" => time_scale = value.parse().unwrap_or(1.0),
            "--match-start-teams" => start_teams = value.parse().unwrap_or(start_teams),
            "--match-xp-scale" => options.xp_scale = value.parse().unwrap_or(1.0),
            "--map" => map_path = (value != "none").then(|| PathBuf::from(value)),
            _ => {}
        }
    }

    options.schedule = MatchSchedule::scaled(time_scale);
    options.schedule.start_teams = start_teams.max(1);
    log!(
        "매치: 시간 배율 {} (한 판 {}초), 경험치 배율 {}, {}팀 이상이면 시작, 몬스터 {}",
        time_scale,
        options.xp_scale,
        options.schedule.total_ms() / 1000,
        options.schedule.start_teams,
        if options.match_monsters { "켬" } else { "끔" }
    );

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
