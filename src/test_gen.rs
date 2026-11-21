//! Deterministic generators for the property tests of the hand-written
//! parsers (`version`, `frontmatter`, `workflow_meta`, `scaffold`, `source`).
//!
//! A seeded SplitMix64 stream, so every run checks the same cases and a
//! failure names the case that reproduces it. No dependency: the parsers are
//! small, and a fixed set of a few thousand cases per property is enough to
//! cover their branches.

/// A SplitMix64 pseudo-random stream.
pub struct Gen {
    state: u64,
}

impl Gen {
    pub fn new(seed: u64) -> Self {
        Gen { state: seed }
    }

    pub fn u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n`. `n` must be nonzero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.u64() % n as u64) as usize
    }

    /// A value in `lo..=hi`.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    /// True with probability `1/n`.
    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// A string of `lo..=hi` chars drawn from `alphabet`.
    pub fn string_from(&mut self, alphabet: &[char], lo: usize, hi: usize) -> String {
        let n = self.range(lo, hi);
        (0..n).map(|_| *self.pick(alphabet)).collect()
    }

    /// A char weighted toward the cases parsers trip on: ASCII punctuation and
    /// whitespace, control characters, multi-byte and astral code points.
    pub fn any_char(&mut self) -> char {
        match self.below(8) {
            0..=2 => (b' ' + self.below(95) as u8) as char,
            3 => *self.pick(&['\t', '\n', '\r', '\0', '\u{7f}', '\u{1b}', '\u{b}', '\u{c}']),
            4 => *self.pick(&['\'', '"', '`', '\\', '#', ':', '$', '{', '}', '/', '*']),
            5 => *self.pick(&[
                '\u{a0}', '\u{3000}', '\u{feff}', '\u{200d}', '\u{2028}', 'é', 'ß',
            ]),
            6 => char::from_u32(self.range(0x80, 0xD7FF) as u32).unwrap_or('x'),
            _ => char::from_u32(self.range(0x1_0000, 0x10_FFFF) as u32).unwrap_or('x'),
        }
    }

    /// A string of `lo..=hi` chars from [`Gen::any_char`].
    pub fn any_string(&mut self, lo: usize, hi: usize) -> String {
        let n = self.range(lo, hi);
        (0..n).map(|_| self.any_char()).collect()
    }
}

/// Run `check` over `cases` generated inputs. Each case gets its own stream,
/// seeded from `seed` and the case index, so a failing case can be rerun alone
/// by its index. The panic message names both.
pub fn cases(seed: u64, cases: u64, mut check: impl FnMut(&mut Gen)) {
    for i in 0..cases {
        let mut g = Gen::new(seed ^ i.wrapping_mul(0xA24B_AED4_963E_E407));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&mut g)));
        if let Err(e) = r {
            eprintln!("property failed: seed {seed:#x}, case {i}");
            std::panic::resume_unwind(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_is_deterministic_and_bounded() {
        let a: Vec<u64> = (0..4).map(|_| Gen::new(7).u64()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]));
        let mut g = Gen::new(1);
        for _ in 0..1000 {
            assert!(g.range(3, 5) >= 3 && g.range(3, 5) <= 5);
            assert!(g.below(4) < 4);
        }
    }
}
