//! Seeded randomness, bit-identical to the motion lab's `rng.js`.

/// FNV-1a over UTF-16 code units, like `rng.js hashString`.
pub fn hash_string(text: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for unit in text.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// mulberry32, bit-identical to `rng.js Rng`.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u32,
}

impl Rng {
    pub fn from_seed(seed: &str) -> Self {
        let state = hash_string(seed);
        Self {
            state: if state == 0 { 1 } else { state },
        }
    }

    pub fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}
