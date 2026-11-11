//! Unicode properties ECMAScript source text needs (ECMA-262 §12), generated
//! from the Unicode Character Database by `tools/generate_js_data.py`.

mod tables;

/// The Unicode version of the identifier and whitespace tables.
pub const UNICODE_VERSION: &str = tables::UNICODE_VERSION;

fn in_ranges(ranges: &[(u32, u32)], cp: u32) -> bool {
    let i = ranges.partition_point(|&(_, hi)| hi < cp);
    ranges.get(i).is_some_and(|&(lo, _)| lo <= cp)
}

/// `ID_Start`, plus `$` and `_` (ECMA-262 `IdentifierStartChar`).
pub fn is_identifier_start(cp: u32) -> bool {
    match cp {
        0x24 | 0x5F => true,
        0x41..=0x5A | 0x61..=0x7A => true,
        0..=0x7F => false,
        _ => in_ranges(&tables::ID_START, cp),
    }
}

/// `ID_Continue`, plus `$`, ZWNJ and ZWJ (ECMA-262 `IdentifierPartChar`).
pub fn is_identifier_part(cp: u32) -> bool {
    match cp {
        0x24 | 0x5F | 0x200C | 0x200D => true,
        0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A => true,
        0..=0x7F => false,
        _ => in_ranges(&tables::ID_CONTINUE, cp),
    }
}

/// ECMA-262 `WhiteSpace`: TAB, VT, FF, ZWNBSP and every `Zs` code point.
pub fn is_whitespace(cp: u32) -> bool {
    matches!(cp, 0x09 | 0x0B | 0x0C | 0xFEFF) || in_ranges(&tables::SPACE_SEPARATOR, cp)
}

/// ECMA-262 `LineTerminator`: LF, CR, LS and PS.
pub fn is_line_terminator(cp: u32) -> bool {
    matches!(cp, 0x0A | 0x0D | 0x2028 | 0x2029)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_disjoint_and_cover_known_characters() {
        for table in [
            &tables::ID_START[..],
            &tables::ID_CONTINUE[..],
            &tables::SPACE_SEPARATOR[..],
        ] {
            for pair in table.windows(2) {
                assert!(pair[0].0 <= pair[0].1 && pair[0].1 + 1 < pair[1].0);
            }
        }
        assert!(is_identifier_start(u32::from('é')));
        assert!(is_identifier_start(u32::from('ℵ')));
        assert!(is_identifier_start(0x2_0000)); // CJK Extension B
        assert!(!is_identifier_start(u32::from('1')));
        assert!(is_identifier_part(u32::from('1')));
        assert!(is_identifier_part(0x0301)); // combining acute accent
        assert!(!is_identifier_start(0x0301));
        assert!(!is_identifier_part(u32::from('-')));
        assert!(is_whitespace(0x3000) && is_whitespace(0xA0) && is_whitespace(0xFEFF));
        assert!(!is_whitespace(0x200B)); // ZWSP is Cf, not Zs
        assert!(is_line_terminator(0x2028) && !is_whitespace(0x2028));
    }
}
