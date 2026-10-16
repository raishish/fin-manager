//! A small seeded random number generator, fully specified here.
//!
//! The generator's output must never change because a dependency changed its algorithm,
//! so this is SplitMix64 (Steele, Lea and Flood, 2014) written out in full. Everything
//! is integer arithmetic; the only floating point is comparing a uniform draw against a
//! probability, which IEEE 754 makes identical on every platform.

/// SplitMix64 state.
#[derive(Debug, Clone)]
pub struct Rng(u64);

/// FNV-1a over a label, used to derive independent streams from one seed.
fn fnv1a(label: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in label.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// An independent stream for `label` under `seed`. Each part of the generator draws
    /// from its own stream, so changing one part doesn't reshuffle every other part.
    pub fn derive(seed: u64, label: &str) -> Rng {
        let mut r = Rng(seed ^ fnv1a(label));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`), by multiply-and-shift.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        ((u128::from(self.next_u64()) * u128::from(n)) >> 64) as u64
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        assert!(lo <= hi, "empty range");
        lo + self.below((hi - lo) as u64 + 1) as i64
    }

    /// Triangular-ish draw in `lo..=hi`, peaked in the middle (mean of two uniforms).
    /// Used for prices, which cluster around a typical value.
    pub fn around(&mut self, lo: i64, hi: i64) -> i64 {
        let a = self.range(lo, hi);
        let b = self.range(lo, hi);
        (a + b) / 2
    }

    /// Uniform in `[0, 1)` with 53 bits of precision.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }

    /// Picks from `(item, weight)` pairs in proportion to the weights.
    pub fn pick_weighted<'a, T>(&mut self, items: &'a [(T, u32)]) -> &'a T {
        let total: u64 = items.iter().map(|(_, w)| u64::from(*w)).sum();
        let mut x = self.below(total);
        for (item, w) in items {
            let w = u64::from(*w);
            if x < w {
                return item;
            }
            x -= w;
        }
        unreachable!("weights sum to total")
    }

    /// `n` random decimal digits.
    pub fn digits(&mut self, n: usize) -> String {
        (0..n)
            .map(|_| char::from(b'0' + self.below(10) as u8))
            .collect()
    }

    /// `n` random uppercase letters and digits.
    pub fn alnum(&mut self, n: usize) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        (0..n)
            .map(|_| char::from(CHARS[self.below(CHARS.len() as u64) as usize]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_reference_values() {
        // First outputs of SplitMix64 seeded with 0, from the reference implementation.
        let mut r = Rng::new(0);
        assert_eq!(r.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(r.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn derived_streams_differ_and_repeat() {
        let a: Vec<u64> = (0..4).map(|_| Rng::derive(7, "a").next_u64()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]));
        assert_ne!(
            Rng::derive(7, "a").next_u64(),
            Rng::derive(7, "b").next_u64()
        );
        assert_ne!(
            Rng::derive(7, "a").next_u64(),
            Rng::derive(8, "a").next_u64()
        );
    }

    #[test]
    fn ranges_stay_in_bounds() {
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let x = r.range(-3, 5);
            assert!((-3..=5).contains(&x));
            let y = r.around(100, 200);
            assert!((100..=200).contains(&y));
            assert!(r.unit() < 1.0);
        }
    }
}
