//! 몬스터 증원 위치·종류를 고르는 난수. 씨앗이 같으면 결과가 같다(SplitMix64).
//!
//! C++ 서버는 `std::mt19937`과 표준 분포를 썼는데, 분포 구현이 컴파일러마다 달라 원래도 MSVC와 GCC의 결과가 달랐다.
//! 그래서 같은 수열을 재현하지 않는다. 증원 위치는 규칙(거리·간격·장애물)만 지키면 된다.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed as u64)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `[low, high)` 범위의 고른 실수
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        let unit = (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32;
        low + (high - low) * unit
    }

    /// `0..count` 범위의 고른 정수. `count`는 1 이상이어야 한다.
    pub fn index(&mut self, count: usize) -> usize {
        (((self.next_u64() >> 32) * count as u64) >> 32) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence_and_values_stay_in_range() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        let mut seen = [false; 5];
        for _ in 0..2000 {
            let value = a.range(650.0, 1250.0);
            assert_eq!(value, b.range(650.0, 1250.0));
            assert!((650.0..1250.0).contains(&value));
            let index = a.index(5);
            assert_eq!(index, b.index(5));
            seen[index] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(a.index(1), 0);
    }
}
