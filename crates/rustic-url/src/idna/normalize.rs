//! Unicode Normalization Form C (UAX #15) for IDNA processing.
//!
//! Decomposition is table-driven except for Hangul syllables, which use the
//! algorithmic mapping from the Unicode Standard section 3.12. Canonical
//! ordering uses a stable sort of each combining-mark run, so cost stays
//! O(n log n) for adversarial runs of marks.

use super::tables::{COMBINING_CLASSES, COMPOSITIONS, DECOMPOSITION_TEXT, DECOMPOSITIONS};

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// Every code point below U+0300 has Canonical_Combining_Class 0 and
/// NFC_Quick_Check=Yes, so such strings are already in NFC.
const NFC_SAFE_BELOW: char = '\u{300}';

pub(super) fn combining_class(value: char) -> u8 {
    let code = u32::from(value);
    let index = COMBINING_CLASSES.partition_point(|&(_, last, _)| last < code);
    match COMBINING_CLASSES.get(index) {
        Some(&(first, _, class)) if first <= code => class,
        _ => 0,
    }
}

pub(super) fn is_nfc(input: &str) -> bool {
    input.chars().all(|value| value < NFC_SAFE_BELOW) || nfc(input) == input
}

pub(super) fn nfc(input: &str) -> String {
    if input.chars().all(|value| value < NFC_SAFE_BELOW) {
        return input.to_owned();
    }
    let mut decomposed = Vec::with_capacity(input.len());
    for value in input.chars() {
        decompose(value, &mut decomposed);
    }
    canonical_order(&mut decomposed);
    compose(&decomposed).into_iter().collect()
}

fn decompose(value: char, output: &mut Vec<char>) {
    let code = u32::from(value);
    if (S_BASE..S_BASE + S_COUNT).contains(&code) {
        let index = code - S_BASE;
        output.extend(
            [
                L_BASE + index / N_COUNT,
                V_BASE + (index % N_COUNT) / T_COUNT,
            ]
            .into_iter()
            .filter_map(char::from_u32),
        );
        let trailing = index % T_COUNT;
        if trailing != 0 {
            output.extend(char::from_u32(T_BASE + trailing));
        }
        return;
    }
    match DECOMPOSITIONS.binary_search_by_key(&code, |&(source, _, _)| source) {
        Ok(index) => {
            let (_, offset, length) = DECOMPOSITIONS[index];
            let start = usize::from(offset);
            output.extend(DECOMPOSITION_TEXT[start..start + usize::from(length)].chars());
        }
        Err(_) => output.push(value),
    }
}

fn canonical_order(values: &mut [char]) {
    let mut index = 0;
    while index < values.len() {
        if combining_class(values[index]) == 0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < values.len() && combining_class(values[index]) != 0 {
            index += 1;
        }
        values[start..index].sort_by_key(|&value| combining_class(value));
    }
}

fn compose(values: &[char]) -> Vec<char> {
    let mut output: Vec<char> = Vec::with_capacity(values.len());
    let mut starter: Option<usize> = None;
    let mut last_class = 0;
    for &value in values {
        let class = combining_class(value);
        if let Some(position) = starter {
            // Not blocked: adjacent to the starter, or every intervening mark
            // has a non-zero class lower than this one (canonical order makes
            // checking the last one sufficient).
            let adjacent = output.len() == position + 1;
            if adjacent || (last_class != 0 && last_class < class) {
                if let Some(composite) = compose_pair(output[position], value) {
                    output[position] = composite;
                    continue;
                }
            }
        }
        if class == 0 {
            starter = Some(output.len());
        }
        output.push(value);
        last_class = class;
    }
    output
}

fn compose_pair(first: char, second: char) -> Option<char> {
    let (first, second) = (u32::from(first), u32::from(second));
    if (L_BASE..L_BASE + L_COUNT).contains(&first) && (V_BASE..V_BASE + V_COUNT).contains(&second) {
        let index = (first - L_BASE) * N_COUNT + (second - V_BASE) * T_COUNT;
        return char::from_u32(S_BASE + index);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&first)
        && (first - S_BASE) % T_COUNT == 0
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&second)
    {
        return char::from_u32(first + (second - T_BASE));
    }
    COMPOSITIONS
        .binary_search_by_key(&(first, second), |&(a, b, _)| (a, b))
        .ok()
        .and_then(|index| char::from_u32(COMPOSITIONS[index].2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_reorders_and_handles_hangul() {
        assert_eq!(nfc("e\u{301}"), "\u{E9}");
        assert_eq!(nfc("a\u{323}\u{302}"), "\u{1EAD}");
        assert_eq!(nfc("a\u{302}\u{323}"), "\u{1EAD}");
        assert_eq!(nfc("\u{1100}\u{1161}\u{11A8}"), "\u{AC01}");
        assert_eq!(nfc("\u{AC01}"), "\u{AC01}");
        // Composition exclusion: U+0958 stays decomposed.
        assert_eq!(nfc("\u{958}"), "\u{915}\u{93C}");
        assert!(is_nfc("plain-ascii"));
        assert!(!is_nfc("e\u{301}"));
    }

    #[test]
    fn long_mark_runs_stay_bounded() {
        let input: String = std::iter::once('a')
            .chain(std::iter::repeat_n(['\u{316}', '\u{301}'], 5_000).flatten())
            .collect();
        let output = nfc(&input);
        assert_eq!(output.chars().count(), input.chars().count() - 1);
        assert!(is_nfc(&output));
    }
}
