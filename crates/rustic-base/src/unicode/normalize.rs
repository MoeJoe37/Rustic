//! Unicode normalization (UAX #15).
//!
//! Decomposition is table-driven except for Hangul syllables, which use the
//! algorithmic mapping from the Unicode Standard section 3.12. Canonical
//! ordering uses a stable sort of each combining-mark run, so cost stays
//! O(n log n) for adversarial runs of marks.

use super::tables::{
    COMBINING_CLASSES, COMPATIBILITY, COMPATIBILITY_TEXT, COMPOSITIONS, DECOMPOSITION_TEXT,
    DECOMPOSITIONS,
};

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

/// The Unicode Canonical_Combining_Class of a code point.
pub fn combining_class(code: u32) -> u8 {
    let index = COMBINING_CLASSES.partition_point(|&(_, last, _)| last < code);
    match COMBINING_CLASSES.get(index) {
        Some(&(first, _, class)) if first <= code => class,
        _ => 0,
    }
}

/// Whether scalar text is already in Unicode Normalization Form C.
pub fn is_nfc(input: &str) -> bool {
    input.chars().all(|value| value < NFC_SAFE_BELOW) || nfc(input) == input
}

/// Normalize scalar text to Unicode Normalization Form C.
pub fn nfc(input: &str) -> String {
    if input.chars().all(|value| value < NFC_SAFE_BELOW) {
        return input.to_owned();
    }
    let mut values = Vec::with_capacity(input.len());
    for value in input.chars() {
        decompose(u32::from(value), false, |value| values.push(value));
    }
    canonical_order(&mut values);
    compose(&mut values);
    values
        .into_iter()
        .map(|value| char::from_u32(value).unwrap())
        .collect()
}

/// One of the four Unicode normalization forms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalizationForm {
    /// Canonical decomposition followed by composition.
    Nfc,
    /// Canonical decomposition.
    Nfd,
    /// Compatibility decomposition followed by composition.
    Nfkc,
    /// Compatibility decomposition.
    Nfkd,
}

/// Normalize UTF-16 with independent bounds on the decomposed workspace and
/// result. Lone surrogates are preserved and act as combining-run boundaries.
/// Returns `None` when a bound would be exceeded.
pub fn normalize_utf16(
    input: impl Iterator<Item = u16>,
    form: NormalizationForm,
    max_decomposed: usize,
    max_result: usize,
) -> Option<Vec<u16>> {
    let compatibility = matches!(form, NormalizationForm::Nfkc | NormalizationForm::Nfkd);
    let mut values = Vec::new();
    for value in char::decode_utf16(input) {
        let value = value
            .map(u32::from)
            .unwrap_or_else(|error| u32::from(error.unpaired_surrogate()));
        let mut exceeded = false;
        decompose(value, compatibility, |code| {
            if values.len() == max_decomposed {
                exceeded = true;
            } else {
                values.push(code);
            }
        });
        if exceeded {
            return None;
        }
    }
    canonical_order(&mut values);
    if matches!(form, NormalizationForm::Nfc | NormalizationForm::Nfkc) {
        compose(&mut values);
    }
    let length = values
        .len()
        .checked_add(values.iter().filter(|&&code| code > 0xFFFF).count())?;
    if length > max_result {
        return None;
    }
    let mut result = Vec::with_capacity(length);
    for value in values {
        if let Some(value) = char::from_u32(value) {
            result.extend_from_slice(value.encode_utf16(&mut [0; 2]));
        } else {
            // Only lone input surrogates bypass char::from_u32 above.
            result.push(u16::try_from(value).expect("lone surrogate"));
        }
    }
    Some(result)
}

fn decompose(code: u32, compatibility: bool, mut emit: impl FnMut(u32)) {
    if (S_BASE..S_BASE + S_COUNT).contains(&code) {
        let index = code - S_BASE;
        emit(L_BASE + index / N_COUNT);
        emit(V_BASE + (index % N_COUNT) / T_COUNT);
        let trailing = index % T_COUNT;
        if trailing != 0 {
            emit(T_BASE + trailing);
        }
        return;
    }
    let compatibility = compatibility
        .then(|| mapped_decomposition(code, COMPATIBILITY, COMPATIBILITY_TEXT))
        .flatten();
    match compatibility.or_else(|| mapped_decomposition(code, DECOMPOSITIONS, DECOMPOSITION_TEXT)) {
        Some(text) => text.chars().map(u32::from).for_each(emit),
        None => emit(code),
    }
}

fn mapped_decomposition(
    code: u32,
    entries: &[(u32, u16, u8)],
    text: &'static str,
) -> Option<&'static str> {
    let index = entries
        .binary_search_by_key(&code, |&(source, _, _)| source)
        .ok()?;
    let (_, offset, length) = entries[index];
    let start = usize::from(offset);
    Some(&text[start..start + usize::from(length)])
}

fn canonical_order(values: &mut [u32]) {
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

fn compose(values: &mut Vec<u32>) {
    let mut len = 0;
    let mut starter = None;
    let mut last_class = 0;
    for read in 0..values.len() {
        let value = values[read];
        let class = combining_class(value);
        if let Some(position) = starter {
            // Canonical ordering makes the last combining class enough to
            // decide whether an intervening mark blocks composition.
            let adjacent = len == position + 1;
            if adjacent || (last_class != 0 && last_class < class) {
                if let Some(composite) = compose_pair(values[position], value) {
                    values[position] = composite;
                    continue;
                }
            }
        }
        if class == 0 {
            starter = Some(len);
        }
        values[len] = value;
        len += 1;
        last_class = class;
    }
    values.truncate(len);
}

fn compose_pair(first: u32, second: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&first) && (V_BASE..V_BASE + V_COUNT).contains(&second) {
        let index = (first - L_BASE) * N_COUNT + (second - V_BASE) * T_COUNT;
        return Some(S_BASE + index);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&first)
        && (first - S_BASE) % T_COUNT == 0
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&second)
    {
        return Some(first + (second - T_BASE));
    }
    COMPOSITIONS
        .binary_search_by_key(&(first, second), |&(a, b, _)| (a, b))
        .ok()
        .map(|index| COMPOSITIONS[index].2)
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

#[cfg(test)]
mod utf16_tests {
    use super::*;

    #[test]
    fn normalizes_all_forms_and_preserves_unpaired_surrogates() {
        use NormalizationForm::*;
        for (form, input, expected) in [
            (Nfc, "e\u{301}", "é"),
            (Nfd, "é", "e\u{301}"),
            (Nfkc, "ǆ", "dž"),
            (Nfkd, "é①ﬁ", "e\u{301}1fi"),
            (Nfc, "\u{1100}\u{1161}\u{11a8}", "각"),
            (Nfd, "각", "\u{1100}\u{1161}\u{11a8}"),
        ] {
            assert_eq!(
                normalize_utf16(input.encode_utf16(), form, 20, 20),
                Some(expected.encode_utf16().collect())
            );
        }
        let input = [0xd800, 0x0301, 0xdc00, 0x0065, 0x0301, 0xd83d, 0xde00];
        assert_eq!(
            normalize_utf16(input.into_iter(), Nfc, 6, 6),
            Some(vec![0xd800, 0x0301, 0xdc00, 0x00e9, 0xd83d, 0xde00])
        );
        assert_eq!(normalize_utf16(input.into_iter(), Nfc, 5, 6), None);
        assert_eq!(normalize_utf16(input.into_iter(), Nfc, 6, 5), None);
    }
}
