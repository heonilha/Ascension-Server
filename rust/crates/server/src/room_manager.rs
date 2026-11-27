//! (모드, teamCode) → 방 대응표. 매치에서 teamCode는 판 번호다.
//!
//! 정원(방 전체, 매치는 팀별 3명도) 확인을 여기서 뮤텍스 안에 끝내기 때문에, 두 명이 동시에 마지막 자리에
//! 들어오려 해도 한 명만 성공한다. 방에 들어가는 일 자체는 방 태스크에서 처리한다.

use crate::room::{self, RoomHandle, RoomOptions};
use asc_game::combat_rules::capacity_for;
use asc_protocol::{EnterResult, RoomMode, MAX_TEAMS, MAX_TEAM_PLAYERS};
use std::collections::HashMap;
use std::sync::Mutex;

struct Entry {
    room: RoomHandle,
    reserved: u32,
    /// 매치 팀별 자리(인덱스 = 팀 번호, 0은 쓰지 않음)
    team_reserved: [u32; MAX_TEAMS as usize + 1],
}

pub struct RoomManager {
    options: RoomOptions,
    rooms: Mutex<HashMap<u64, Entry>>,
}

fn key(mode: RoomMode, team_code: u32) -> u64 {
    ((mode.raw() as u64) << 32) | team_code as u64
}

impl RoomManager {
    pub fn new(options: RoomOptions) -> RoomManager {
        RoomManager { options, rooms: Mutex::new(HashMap::new()) }
    }

    /// 매치 팀 번호가 맞는지. 연습 방은 팀을 보지 않는다.
    pub fn valid_team(mode: RoomMode, team: u8) -> bool {
        mode == RoomMode::Practice || (1..=MAX_TEAMS).contains(&team)
    }

    /// 자리를 하나 잡는다. 방이 없으면 만든다(tokio 런타임 안에서 불러야 한다).
    /// 방이 가득 찼으면 `RoomFull`, 매치에서 그 팀이 3명이면 `TeamFull`. `team`은 `valid_team`으로 확인한 값이어야 한다.
    pub fn reserve(&self, mode: RoomMode, team_code: u32, team: u8) -> Result<RoomHandle, EnterResult> {
        let mut rooms = self.rooms.lock().unwrap();
        let entry = rooms.entry(key(mode, team_code)).or_insert_with(|| Entry {
            room: room::spawn(mode, team_code, self.options.clone()),
            reserved: 0,
            team_reserved: [0; MAX_TEAMS as usize + 1],
        });
        if entry.reserved >= capacity_for(mode) as u32 {
            return Err(EnterResult::RoomFull);
        }
        if mode == RoomMode::Match && entry.team_reserved[team as usize] >= MAX_TEAM_PLAYERS as u32 {
            return Err(EnterResult::TeamFull);
        }
        entry.reserved += 1;
        if mode == RoomMode::Match {
            entry.team_reserved[team as usize] += 1;
        }
        Ok(entry.room.clone())
    }

    /// `reserve`로 잡은 자리를 돌려준다. 마지막 자리면 방을 닫는다:
    /// 대응표에서 지우면 방으로 가는 채널이 모두 닫히고, 방 태스크는 남은 명령을 처리한 뒤 끝난다.
    pub fn release(&self, mode: RoomMode, team_code: u32, team: u8) {
        let mut rooms = self.rooms.lock().unwrap();
        let key = key(mode, team_code);
        if let Some(entry) = rooms.get_mut(&key) {
            entry.reserved = entry.reserved.saturating_sub(1);
            if mode == RoomMode::Match {
                entry.team_reserved[team as usize] = entry.team_reserved[team as usize].saturating_sub(1);
            }
            if entry.reserved == 0 {
                rooms.remove(&key);
            }
        }
    }

    pub fn room_count(&self) -> usize {
        self.rooms.lock().unwrap().len()
    }
}
