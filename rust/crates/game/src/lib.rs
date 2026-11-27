//! 네트워크를 모르는 게임 계산: 충돌 지형, 전투 판정 규칙, 몬스터·검기 시뮬레이션, 매치 시간표(라운드·자기장).
//!
//! C++ 서버의 `src/game`(CollisionMap, CombatRules, Monsters)을 옮긴 것이다. 방(Room)이 이 위에 올라간다.
//! 계산 순서를 C++ 원본과 같게 두어, 같은 입력이면 같은 결과가 나오는지 기준 기록(`tests/golden.rs`)으로 비교한다.

pub mod collision;
pub mod combat_rules;
pub mod match_rules;
pub mod monsters;
pub mod rng;
pub mod room_layout;

pub use collision::CollisionMap;
