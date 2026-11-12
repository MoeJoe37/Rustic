//! `Canonicalize(rer, ch)` (§22.2.2.7.3) for case-insensitive patterns,
//! from the standard library's Unicode case mappings.
//!
//! Without `u`/`v` a character canonicalizes to its upper case when that
//! is one character and does not take a non-ASCII character into ASCII.
//! With `u`/`v` the specification uses simple case folding; this module
//! derives it as the lower case of the upper case, which gives the same
//! equivalence classes except for the few characters listed in
//! [`FOLD_EXCEPTIONS`].

use std::sync::OnceLock;

use super::class::CharSet;

/// Characters whose simple case folding differs from the lower case of
/// their upper case (CaseFolding.txt, status C and S).
const FOLD_EXCEPTIONS: &[(u32, u32)] = &[
    // Dotless i has no folding (its upper case is ASCII `I`).
    (0x131, 0x131),
    (0x1FD3, 0x390),
    (0x1FE3, 0x3B0),
    (0xFB05, 0xFB06),
];

fn single(mut it: impl Iterator<Item = char>) -> Option<char> {
    let first = it.next()?;
    it.next().is_none().then_some(first)
}

/// `Canonicalize` of one character.
pub(crate) fn canonicalize(c: u32, unicode: bool) -> u32 {
    if c < 0x80 {
        return if (0x61..=0x7A).contains(&c) {
            if unicode { c } else { c - 0x20 }
        } else if unicode && (0x41..=0x5A).contains(&c) {
            c + 0x20
        } else {
            c
        };
    }
    let Some(ch) = char::from_u32(c) else {
        // A lone surrogate.
        return c;
    };
    if unicode {
        if let Some(&(_, folded)) = FOLD_EXCEPTIONS.iter().find(|(from, _)| *from == c) {
            return folded;
        }
        let upper = single(ch.to_uppercase()).unwrap_or(ch);
        let lower = single(upper.to_lowercase()).unwrap_or(upper);
        u32::from(lower)
    } else {
        match single(ch.to_uppercase()) {
            Some(upper) if (0x80..=0xFFFF).contains(&u32::from(upper)) => u32::from(upper),
            _ => c,
        }
    }
}

/// Every character with a canonical form shared with another character,
/// as (canonical form, character) sorted by form: the non-trivial
/// equivalence classes.
fn classes(unicode: bool) -> &'static [(u32, u32)] {
    static UNICODE: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    static UNITS: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    let cell = if unicode { &UNICODE } else { &UNITS };
    cell.get_or_init(|| {
        let last = if unicode { 0x10FFFF } else { 0xFFFF };
        let mut pairs = Vec::new();
        for c in 0..=last {
            let k = canonicalize(c, unicode);
            if k != c {
                pairs.push((k, c));
            }
        }
        // Each form is in its own class too.
        let mut forms: Vec<u32> = pairs.iter().map(|&(k, _)| k).collect();
        forms.sort_unstable();
        forms.dedup();
        pairs.extend(forms.into_iter().map(|k| (k, k)));
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    })
}

/// `set` closed under case equivalence: a character matches a class
/// case-insensitively exactly when the closure contains it.
pub(crate) fn close(set: &CharSet, unicode: bool) -> CharSet {
    let table = classes(unicode);
    let mut closed = set.clone();
    let mut i = 0;
    while i < table.len() {
        let k = table[i].0;
        let end = i + table[i..].partition_point(|&(f, _)| f == k);
        if table[i..end].iter().any(|&(_, c)| set.contains(c)) {
            for &(_, c) in &table[i..end] {
                closed.add(c);
            }
        }
        i = end;
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_forms() {
        assert_eq!(canonicalize(u32::from('a'), false), u32::from('A'));
        assert_eq!(canonicalize(u32::from('A'), true), u32::from('a'));
        // ſ and the Kelvin sign stay apart from ASCII without `u`...
        assert_eq!(canonicalize(0x17F, false), 0x17F);
        assert_eq!(canonicalize(0x212A, false), 0x212A);
        // ...and fold into it with `u`.
        assert_eq!(canonicalize(0x17F, true), u32::from('s'));
        assert_eq!(canonicalize(0x212A, true), u32::from('k'));
        assert_eq!(canonicalize(0x3C2, true), 0x3C3); // final sigma
        assert_eq!(canonicalize(0x3C2, false), 0x3A3);
        assert_eq!(canonicalize(0x131, true), 0x131);
        assert_eq!(canonicalize(0xD800, false), 0xD800);
        let closed = close(&CharSet::single(u32::from('k')), true);
        assert!(closed.contains(u32::from('K')) && closed.contains(0x212A));
        let closed = close(&CharSet::single(u32::from('k')), false);
        assert!(closed.contains(u32::from('K')) && !closed.contains(0x212A));
    }
}
