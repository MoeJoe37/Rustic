//! Allocation-free comparisons between UTF-16 attribute values and selector
//! strings.

fn fold(unit: u16, insensitive: bool) -> u16 {
    if insensitive && (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

pub(crate) fn is_html_whitespace(unit: u16) -> bool {
    matches!(unit, 0x09 | 0x0a | 0x0c | 0x0d | 0x20)
}

pub(crate) fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// `units == s`, optionally ASCII case-insensitive.
pub(crate) fn equals(units: &[u16], s: &str, insensitive: bool) -> bool {
    let mut expected = s.encode_utf16();
    for &unit in units {
        match expected.next() {
            Some(e) if fold(e, insensitive) == fold(unit, insensitive) => {}
            _ => return false,
        }
    }
    expected.next().is_none()
}

pub(crate) fn starts_with(units: &[u16], s: &str, insensitive: bool) -> bool {
    let n = utf16_len(s);
    units.len() >= n && equals(&units[..n], s, insensitive)
}

pub(crate) fn ends_with(units: &[u16], s: &str, insensitive: bool) -> bool {
    let n = utf16_len(s);
    units.len() >= n && equals(&units[units.len() - n..], s, insensitive)
}

pub(crate) fn contains(units: &[u16], s: &str, insensitive: bool) -> bool {
    let n = utf16_len(s);
    n <= units.len() && units.windows(n).any(|w| equals(w, s, insensitive))
}

/// One of the whitespace-separated words of `units` equals `s`.
pub(crate) fn includes(units: &[u16], s: &str, insensitive: bool) -> bool {
    units
        .split(|&u| is_html_whitespace(u))
        .any(|word| !word.is_empty() && equals(word, s, insensitive))
}

/// `units == s`, or `units` starts with `s` followed by `-`.
pub(crate) fn dash_match(units: &[u16], s: &str, insensitive: bool) -> bool {
    let n = utf16_len(s);
    equals(units, s, insensitive)
        || (units.len() > n && units[n] == u16::from(b'-') && equals(&units[..n], s, insensitive))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn comparisons_handle_case_and_astral_code_points() {
        assert!(equals(&u("AbC"), "abc", true));
        assert!(!equals(&u("AbC"), "abc", false));
        assert!(!equals(&u("É"), "é", true), "only ASCII folds");
        assert!(starts_with(&u("😀x"), "😀", false));
        assert!(ends_with(&u("x😀"), "😀", false));
        assert!(contains(&u("a😀b"), "😀b", false));
        assert!(!contains(&u("a"), "ab", false));
        assert!(includes(&u(" a\tB\n"), "b", true));
        assert!(!includes(&u("ab"), "a", false));
        assert!(dash_match(&u("en-US"), "en", false));
        assert!(dash_match(&u("en"), "en", false));
        assert!(!dash_match(&u("eng"), "en", false));
    }
}
