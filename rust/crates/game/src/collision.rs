//! 서버 충돌 지형: 클라이언트 맵의 막히는 구조물을 2D 볼록 다각형 + 높이 범위로 들고 있는다.
//!
//! 데이터는 클라이언트 저장소의 `Scripts/ExportServerCollision.py`가 실제 맵 액터에서 뽑은
//! `data/CombatLab.txt`다(손으로 고치지 않는다). 파일 해석과 점-다각형 거리 계산은 클라이언트와 같은
//! C++ 코드(`asc-movement`의 FFI)를 쓰고, 이 파일은 그 위의 서버 전용 판정(공격 엄폐, 몬스터 이동)이다.
//!
//! 서버 시작 때 한 번 읽고 이후에는 읽기만 하므로 여러 방에서 동시에 써도 된다(`Arc<CollisionMap>`).

use asc_movement::{Vec2, World, HALF_HEIGHT};
use asc_protocol::Vec3;
use std::path::Path;

struct Obstacle {
    /// 윗면이 계단 높이(45cm)보다 높다. 공격 엄폐 판정용
    blocking: bool,
    /// 이동에서 단단한 장애물(낮은 받침 포함)
    solid: bool,
    z_min: f32,
    z_max: f32,
    polygon: Vec<Vec2>,
    // 경계 상자(먼 장애물을 빨리 건너뛴다)
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl Obstacle {
    /// 오를 수 없고, 캡슐 높이 범위와 겹치는 장애물만 막는다(머리 위 상인방·제단 덮개는 통과).
    fn relevant(&self, center_z: f32) -> bool {
        self.blocking && self.z_min < center_z + HALF_HEIGHT && self.z_max > center_z - HALF_HEIGHT
    }

    fn outside_bounds(&self, p: Vec2, radius: f32) -> bool {
        p.x < self.min_x - radius || p.x > self.max_x + radius || p.y < self.min_y - radius || p.y > self.max_y + radius
    }
}

/// 맵 배치 표식(`area <kind> <index> <x> <y> <radius>` 줄): 풀숲·캠프·균열 입구·균열 전투장. 막지 않는다.
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub kind: String,
    pub index: u32,
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

impl Area {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        (x - self.x).hypot(y - self.y) <= self.radius
    }
}

pub struct CollisionMap {
    world: World,
    obstacles: Vec<Obstacle>,
    areas: Vec<Area>,
}

fn flat(v: Vec3) -> Vec2 {
    Vec2 { x: v.x, y: v.y }
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn sub(a: Vec2, b: Vec2) -> Vec2 {
    Vec2 { x: a.x - b.x, y: a.y - b.y }
}

impl CollisionMap {
    /// 파일을 읽는다. 형식이 틀리면 이유를 돌려준다.
    pub fn load(path: &Path) -> Result<CollisionMap, String> {
        let text = std::fs::read_to_string(path).map_err(|_| format!("{} 파일을 열 수 없음", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// 충돌 파일 내용을 읽는다.
    pub fn parse(text: &str) -> Result<CollisionMap, String> {
        let world = World::parse(text)?;
        let mut obstacles = Vec::with_capacity(world.obstacle_count());
        for o in world.obstacles() {
            // 반시계 볼록 다각형인지 확인한다(판정 코드가 이 가정을 쓴다).
            let n = o.polygon.len();
            for i in 0..n {
                let (a, b, c) = (o.polygon[i], o.polygon[(i + 1) % n], o.polygon[(i + 2) % n]);
                if cross(sub(b, a), sub(c, b)) < -1e-3 {
                    return Err(format!("{}이 반시계 볼록 다각형이 아님", o.name));
                }
            }
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (1e30f32, 1e30f32, -1e30f32, -1e30f32);
            for v in o.polygon {
                min_x = min_x.min(v.x);
                min_y = min_y.min(v.y);
                max_x = max_x.max(v.x);
                max_y = max_y.max(v.y);
            }
            obstacles.push(Obstacle {
                blocking: o.blocking,
                solid: o.solid,
                z_min: o.z_min,
                z_max: o.z_max,
                polygon: o.polygon.to_vec(),
                min_x,
                min_y,
                max_x,
                max_y,
            });
        }
        let mut areas = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let mut words = line.split_whitespace();
            if words.next() != Some("area") {
                continue;
            }
            let fields: Vec<&str> = words.collect();
            let bad = || format!("area 줄 {}", number + 1);
            if fields.len() != 5 {
                return Err(bad());
            }
            let number = |i: usize| fields[i].parse::<f32>().map_err(|_| bad());
            areas.push(Area { kind: fields[0].to_string(), index: fields[1].parse().map_err(|_| bad())?, x: number(2)?, y: number(3)?, radius: number(4)? });
        }
        areas.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.index.cmp(&b.index)));
        Ok(CollisionMap { world, obstacles, areas })
    }

    /// 배치 표식 중 `kind` 종류(번호 순)
    pub fn areas(&self, kind: &str) -> impl Iterator<Item = &Area> {
        let kind = kind.to_string();
        self.areas.iter().filter(move |a| a.kind == kind)
    }

    /// 이동 계산(`asc_movement::step`)에 넘기는 지형.
    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn obstacle_count(&self) -> usize {
        self.obstacles.len()
    }

    fn distance(&self, index: usize, p: Vec2) -> f32 {
        self.world.signed_distance(index, p).0
    }

    /// 이동에서 단단한 장애물(낮은 받침 포함)까지 가장 가까운 거리(안쪽이면 음수). 단단한 장애물이 없으면 아주 큰 값.
    pub fn solid_clearance(&self, p: Vec2) -> f32 {
        let mut best = 1e9f32;
        for (index, o) in self.obstacles.iter().enumerate() {
            if o.solid {
                best = best.min(self.distance(index, p));
            }
        }
        best
    }

    /// 이동에서 단단한 장애물과 반지름 `radius` 원이 겹치는가. 몬스터 이동·생성 위치 검사용.
    pub fn solid_overlap(&self, p: Vec2, radius: f32) -> bool {
        self.obstacles.iter().enumerate().any(|(index, o)| !o.outside_bounds(p, radius) && o.solid && self.distance(index, p) < radius)
    }

    /// 원을 단단한 장애물 밖으로 밀어낸다. 밀렸으면 true.
    pub fn push_out_solid(&self, p: &mut Vec2, radius: f32) -> bool {
        let mut moved = false;
        for _ in 0..4 {
            let mut pushed = false;
            for (index, o) in self.obstacles.iter().enumerate() {
                if o.outside_bounds(*p, radius) || !o.solid {
                    continue;
                }
                let (d, normal) = self.world.signed_distance(index, *p);
                if d >= radius {
                    continue;
                }
                p.x += normal.x * (radius - d);
                p.y += normal.y * (radius - d);
                pushed = true;
                moved = true;
            }
            if !pushed {
                break;
            }
        }
        moved
    }

    fn segment_hits(&self, index: usize, a: Vec2, b: Vec2) -> bool {
        if self.distance(index, a) <= 0.0 || self.distance(index, b) <= 0.0 {
            return true;
        }
        let polygon = &self.obstacles[index].polygon;
        let d = sub(b, a);
        for i in 0..polygon.len() {
            let p = polygon[i];
            let e = sub(polygon[(i + 1) % polygon.len()], p);
            let denominator = cross(d, e);
            if denominator.abs() < 1e-6 {
                continue; // 평행
            }
            let ap = sub(p, a);
            let (t, u) = (cross(ap, e) / denominator, cross(ap, d) / denominator);
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                return true;
            }
        }
        false
    }

    /// 캡슐 중심이 반지름 `radius` 원으로 오를 수 없는 장애물과 겹치는가(머리 위 구조물·낮은 받침 제외).
    pub fn blocked(&self, center: Vec3, radius: f32) -> bool {
        let (p, z) = (flat(center), center.z);
        // 경계 상자 밖은 거리가 반지름보다 멀다(결과는 같고 큰 맵에서 빠르다)
        self.obstacles.iter().enumerate().any(|(index, o)| !o.outside_bounds(p, radius) && o.relevant(z) && self.distance(index, p) < radius)
    }

    /// `from` → `to`로 반지름 `radius` 원을 밀었을 때 장애물에 걸리는가.
    pub fn sweep_blocked(&self, from: Vec3, to: Vec3, radius: f32) -> bool {
        // 10cm 간격으로 원을 놓아 본다. 반지름보다 촘촘하므로 얇은 벽도 건너뛰지 못한다.
        let (from_x, from_y, from_z, to_x, to_y, to_z) = (from.x, from.y, from.z, to.x, to.y, to.z);
        let distance = (to_x - from_x).hypot(to_y - from_y);
        let steps = ((distance / 10.0).ceil() as i32).max(1);
        (1..=steps).any(|i| {
            let t = i as f32 / steps as f32;
            self.blocked(Vec3 { x: from_x + (to_x - from_x) * t, y: from_y + (to_y - from_y) * t, z: from_z + (to_z - from_z) * t }, radius)
        })
    }

    /// 두 점 사이 시야가 막혔는가(공격이 기둥 너머로 닿지 않게). 두 점 중 높은 쪽 높이 범위로 본다.
    pub fn line_blocked(&self, from: Vec3, to: Vec3) -> bool {
        let z = { from.z }.max(to.z);
        let (a, b) = (flat(from), flat(to));
        let (min_x, max_x, min_y, max_y) = (a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y));
        self.obstacles.iter().enumerate().any(|(index, o)| {
            // 선분의 경계 상자와 겹치지 않는 장애물은 맞을 수 없다
            !(o.max_x < min_x || o.min_x > max_x || o.max_y < min_y || o.min_y > max_y) && o.relevant(z) && self.segment_hits(index, a, b)
        })
    }
}

impl std::fmt::Debug for CollisionMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollisionMap").field("obstacles", &self.obstacles.len()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_clockwise_polygon() {
        let clockwise = "version 1\nobstacle Bad 1 0.0 300.0 4 0.0 0.0 0.0 100.0 100.0 100.0 100.0 0.0\n";
        assert!(CollisionMap::parse(clockwise).unwrap_err().contains("Bad"));
    }

    #[test]
    fn missing_file_is_reported() {
        assert!(CollisionMap::load(Path::new("no/such/file.txt")).unwrap_err().contains("열 수 없음"));
    }
}
