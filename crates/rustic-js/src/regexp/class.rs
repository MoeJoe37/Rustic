//! Sets of characters as sorted, disjoint, non-adjacent ranges.

/// The largest code point.
const MAX: u32 = 0x10FFFF;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CharSet {
    ranges: Vec<(u32, u32)>,
}

impl CharSet {
    pub(crate) fn single(c: u32) -> Self {
        CharSet {
            ranges: vec![(c, c)],
        }
    }

    fn from_ranges(pairs: &[(u32, u32)]) -> Self {
        let mut set = CharSet::default();
        for &(a, b) in pairs {
            set.add_range(a, b);
        }
        set
    }

    /// `\d`
    pub(crate) fn digits() -> Self {
        Self::from_ranges(&[(0x30, 0x39)])
    }

    /// `\s`: WhiteSpace and LineTerminator.
    pub(crate) fn spaces() -> Self {
        Self::from_ranges(&[
            (0x09, 0x0D),
            (0x20, 0x20),
            (0xA0, 0xA0),
            (0x1680, 0x1680),
            (0x2000, 0x200A),
            (0x2028, 0x2029),
            (0x202F, 0x202F),
            (0x205F, 0x205F),
            (0x3000, 0x3000),
            (0xFEFF, 0xFEFF),
        ])
    }

    /// `\w` (WordCharacters); with `u` (or `v`) and `i` it also holds the
    /// two characters that fold into it, `ſ` and the Kelvin sign.
    pub(crate) fn word(unicode_ignore_case: bool) -> Self {
        let mut set = Self::from_ranges(&[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)]);
        if unicode_ignore_case {
            set.add(0x17F);
            set.add(0x212A);
        }
        set
    }

    pub(crate) fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub(crate) fn contains(&self, c: u32) -> bool {
        let i = self.ranges.partition_point(|&(_, hi)| hi < c);
        self.ranges.get(i).is_some_and(|&(lo, _)| lo <= c)
    }

    pub(crate) fn add(&mut self, c: u32) {
        self.add_range(c, c);
    }

    pub(crate) fn add_range(&mut self, lo: u32, hi: u32) {
        debug_assert!(lo <= hi);
        // The first range that touches or follows `lo`.
        let start = self
            .ranges
            .partition_point(|&(_, h)| h.saturating_add(1) < lo);
        let mut end = start;
        let (mut lo, mut hi) = (lo, hi);
        while end < self.ranges.len() && self.ranges[end].0 <= hi.saturating_add(1) {
            lo = lo.min(self.ranges[end].0);
            hi = hi.max(self.ranges[end].1);
            end += 1;
        }
        self.ranges.splice(start..end, [(lo, hi)]);
    }

    pub(crate) fn add_set(&mut self, other: &CharSet) {
        for &(a, b) in &other.ranges {
            self.add_range(a, b);
        }
    }

    pub(crate) fn negated(&self) -> Self {
        let mut ranges = Vec::with_capacity(self.ranges.len() + 1);
        let mut next = 0;
        for &(a, b) in &self.ranges {
            if a > next {
                ranges.push((next, a - 1));
            }
            next = b + 1;
        }
        if next <= MAX {
            ranges.push((next, MAX));
        }
        CharSet { ranges }
    }

    pub(crate) fn intersect(&self, other: &CharSet) -> Self {
        let mut ranges = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.ranges.len() && j < other.ranges.len() {
            let (a, b) = self.ranges[i];
            let (c, d) = other.ranges[j];
            let (lo, hi) = (a.max(c), b.min(d));
            if lo <= hi {
                ranges.push((lo, hi));
            }
            if b < d {
                i += 1;
            } else {
                j += 1;
            }
        }
        CharSet { ranges }
    }

    pub(crate) fn subtract(&self, other: &CharSet) -> Self {
        self.intersect(&other.negated())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_and_complement() {
        let mut s = CharSet::default();
        s.add_range(10, 20);
        s.add_range(30, 40);
        s.add_range(21, 29);
        assert_eq!(s.ranges(), &[(10, 40)]);
        s.add(5);
        assert_eq!(s.ranges(), &[(5, 5), (10, 40)]);
        assert!(s.contains(5) && s.contains(25) && !s.contains(6) && !s.contains(41));
        let n = s.negated();
        assert_eq!(n.ranges(), &[(0, 4), (6, 9), (41, MAX)]);
        assert_eq!(n.negated(), s);
        assert_eq!(
            s.intersect(&CharSet::from_ranges(&[(0, 12)])).ranges(),
            &[(5, 5), (10, 12)]
        );
        assert_eq!(
            s.subtract(&CharSet::single(20)).ranges(),
            &[(5, 5), (10, 19), (21, 40)]
        );
    }
}
