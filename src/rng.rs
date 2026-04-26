#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn next_f32(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32;
        bits as f32 / 0x01_00_00_00_u32 as f32
    }
}

pub fn rng_for_seed(seed: Option<u64>) -> SeededRng {
    SeededRng::new(seed.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_replays_same_sequence() {
        let mut a = rng_for_seed(Some(42));
        let mut b = rng_for_seed(Some(42));
        assert_eq!(a.next_u64(), b.next_u64());
        assert_eq!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = rng_for_seed(Some(1));
        let mut b = rng_for_seed(Some(2));
        assert_ne!(a.next_u64(), b.next_u64());
    }
}
