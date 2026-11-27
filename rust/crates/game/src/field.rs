//! 배틀로얄 필드 배치: 몬스터 캠프, 풀숲, 균열 입구·전투장.
//!
//! 기준: Ascension-Team `Docs/11.26기획안.md` "필드", "균열".
//! 자리는 맵(클라이언트 `Scripts/CreateMap.py`)이 정하고, 충돌 파일의 `area` 줄로 서버에 온다.
//! 이 파일은 종류별 의미(캠프 구성 등)만 정한다.

use crate::collision::{Area, CollisionMap};

/// 캠프 한 곳: 중심, 반지름, 채울 몬스터 종류(0 검병, 1 폭주 망령, 2 지휘관)
#[derive(Clone, Debug, PartialEq)]
pub struct CampSpot {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub kinds: Vec<u8>,
}

/// 원 하나(풀숲·균열 입구·전투장)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

impl Circle {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        (x - self.x).hypot(y - self.y) <= self.radius
    }
}

impl From<&Area> for Circle {
    fn from(a: &Area) -> Circle {
        Circle { x: a.x, y: a.y, radius: a.radius }
    }
}

/// 캠프 종류별 구성(임시, 플레이 시험으로 조정)
/// - 외곽 캠프(팀 기지 사이): 검병 2 + 폭주 망령 1
/// - 폐허 캠프(안쪽): 검병 2 + 폭주 망령 2
/// - 정예 캠프(중앙 남북): 지휘관 1 + 검병 2
pub fn camp_kinds(kind: &str) -> Option<Vec<u8>> {
    match kind {
        "camp_outer" => Some(vec![0, 0, 1]),
        "camp_ruin" => Some(vec![0, 1, 0, 1]),
        "camp_elite" => Some(vec![2, 0, 0]),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldLayout {
    pub camps: Vec<CampSpot>,
    pub bushes: Vec<Circle>,
    /// 균열 입구 2개(번호 순)
    pub rift_gates: Vec<Circle>,
    pub rift_arena: Option<Circle>,
}

impl FieldLayout {
    /// 충돌 파일의 배치 표식에서 읽는다. 지형이 없으면 빈 배치(캠프·풀숲·균열 없음).
    pub fn from_map(map: Option<&CollisionMap>) -> FieldLayout {
        let Some(map) = map else { return FieldLayout::default() };
        let mut camps = Vec::new();
        for kind in ["camp_outer", "camp_ruin", "camp_elite"] {
            for area in map.areas(kind) {
                camps.push(CampSpot { x: area.x, y: area.y, radius: area.radius, kinds: camp_kinds(kind).expect("캠프 종류") });
            }
        }
        FieldLayout {
            camps,
            bushes: map.areas("bush").map(Circle::from).collect(),
            rift_gates: map.areas("rift_gate").map(Circle::from).collect(),
            rift_arena: map.areas("rift_arena").next().map(Circle::from),
        }
    }

    /// 그 자리가 들어 있는 풀숲 번호
    pub fn bush_at(&self, x: f32, y: f32) -> Option<usize> {
        self.bushes.iter().position(|b| b.contains(x, y))
    }

    /// 균열이 동작하는가(입구 2개와 전투장이 맵에 있다)
    pub fn has_rift(&self) -> bool {
        self.rift_gates.len() >= 2 && self.rift_arena.is_some()
    }
}
