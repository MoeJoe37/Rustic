//! Deterministic pseudo-random numbers and input fingerprints.

/// SplitMix64 generator: tiny, fast and identical on every platform.
///
/// It is not cryptographic. The same seed always yields the same sequence, so
/// a fuzz run is reproduced exactly from its printed seed and iteration count.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Create a generator from a seed; every seed, including zero, is valid.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Next 64 uniformly distributed bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `0..bound`; a zero bound returns zero.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        // Modulo bias is irrelevant for mutation choice and keeps this portable.
        (self.next_u64() % bound as u64) as usize
    }

    /// True with probability `1 / n` (always true when `n <= 1`).
    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }
}

/// 64-bit FNV-1a fingerprint, used for split plans and reproducer names.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_are_fixed_per_seed() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let first: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        let second: Vec<u64> = (0..4).map(|_| b.next_u64()).collect();
        assert_eq!(first, second);
        assert_ne!(Rng::new(8).next_u64(), first[0]);
        // Pinned value: a change here would silently invalidate recorded seeds.
        assert_eq!(Rng::new(0).next_u64(), 0xe220_a839_7b1d_cdaf);
    }

    #[test]
    fn bounds_hold() {
        let mut rng = Rng::new(1);
        assert_eq!(rng.below(0), 0);
        for bound in 1..50 {
            assert!(rng.below(bound) < bound);
        }
        assert!(rng.one_in(1));
        assert_eq!(fingerprint(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
    }
}
