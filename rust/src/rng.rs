//! Small self-contained xoshiro256++ RNG (stable across versions; serializable).
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut sm = seed;
        let mut next = move || {
            sm = sm.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        };
        Rng { s: [next(), next(), next(), next()] }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[0]
            .wrapping_add(self.s[3])
            .rotate_left(23)
            .wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    pub fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f64()
    }

    /// Uniform in [0, n). n must be > 0.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform integer in [a, b] inclusive.
    pub fn randint(&mut self, a: i32, b: i32) -> i32 {
        a + self.below((b - a + 1) as usize) as i32
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    /// Weighted index choice.
    pub fn weighted(&mut self, weights: &[f64]) -> usize {
        let total: f64 = weights.iter().sum();
        let mut x = self.f64() * total;
        for (i, w) in weights.iter().enumerate() {
            if x < *w {
                return i;
            }
            x -= *w;
        }
        weights.len() - 1
    }
}
