//! A small deterministic generator (SplitMix64). Every random choice of a
//! chaos run comes from one seed through this type, so a failure replays
//! exactly from `--seed`. It is not cryptographic and does not try to be.

/// The seeded generator.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n == 0` gives 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        // Rejection sampling keeps it unbiased.
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + self.below((hi - lo) as u64 + 1) as i64
    }

    /// True with probability `permille / 1000`.
    pub fn chance(&mut self, permille: u32) -> bool {
        self.below(1000) < u64::from(permille)
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> Option<&'a T> {
        if xs.is_empty() {
            None
        } else {
            Some(&xs[self.below(xs.len() as u64) as usize])
        }
    }

    /// An index chosen by weight (zero weights are never chosen).
    pub fn weighted(&mut self, weights: &[u32]) -> Option<usize> {
        let total: u64 = weights.iter().map(|w| u64::from(*w)).sum();
        if total == 0 {
            return None;
        }
        let mut at = self.below(total);
        for (i, w) in weights.iter().enumerate() {
            let w = u64::from(*w);
            if at < w {
                return Some(i);
            }
            at -= w;
        }
        None
    }

    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            xs.swap(i, j);
        }
    }

    /// An independent stream for a named part of a run (an actor, a fault
    /// lane), so adding draws to one part never shifts another's.
    pub fn fork(&self, label: &str) -> Rng {
        let mut h = self.0 ^ 0xCBF2_9CE4_8422_2325;
        for b in label.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        Rng(Rng(h).next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream_and_known_values() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
        // SplitMix64 reference value for seed 0.
        assert_eq!(Rng::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn bounds_and_weights() {
        let mut r = Rng::new(7);
        for _ in 0..1000 {
            assert!(r.below(10) < 10);
            let v = r.range(-3, 3);
            assert!((-3..=3).contains(&v));
        }
        let mut hits = [0u32; 3];
        for _ in 0..3000 {
            hits[r.weighted(&[1, 0, 2]).unwrap()] += 1;
        }
        assert_eq!(hits[1], 0);
        assert!(hits[2] > hits[0]);
        assert!(r.weighted(&[0, 0]).is_none());
    }

    #[test]
    fn forks_are_independent_of_draw_order() {
        let base = Rng::new(9);
        let mut f1 = base.fork("actor-1");
        let mut base2 = base.clone();
        let _ = base2.next_u64();
        // A fork depends on the label and the seed state, not on how many
        // draws other forks made.
        let mut f1_again = base.fork("actor-1");
        assert_eq!(f1.next_u64(), f1_again.next_u64());
        assert_ne!(base.fork("a").next_u64(), base.fork("b").next_u64());
    }
}
