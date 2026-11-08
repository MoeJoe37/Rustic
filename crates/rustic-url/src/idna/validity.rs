//! UTS #46 section 4.1 Validity Criteria for the URL Standard's flags:
//! CheckHyphens=false, CheckJoiners=true, UseSTD3ASCIIRules=false and
//! nontransitional processing. CheckBidi lives in `bidi` because it needs the
//! whole domain.

use super::IdnaError;
use super::mapping;
use super::normalize;
use super::tables::{JOINING_TYPES, MARKS};

const ZERO_WIDTH_NON_JOINER: char = '\u{200C}';
const ZERO_WIDTH_JOINER: char = '\u{200D}';
const VIRAMA_COMBINING_CLASS: u8 = 9;

/// Joining_Type values used by the ContextJ rule; others are non-joining.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum JoiningType {
    Left,
    Dual,
    Right,
    Transparent,
}

fn joining_type(value: char) -> Option<JoiningType> {
    let code = u32::from(value);
    let index = JOINING_TYPES.partition_point(|&(_, last, _)| last < code);
    match JOINING_TYPES.get(index) {
        Some(&(first, _, kind)) if first <= code => Some(kind),
        _ => None,
    }
}

fn is_mark(value: char) -> bool {
    let code = u32::from(value);
    let index = MARKS.partition_point(|&(_, last)| last < code);
    MARKS.get(index).is_some_and(|&(first, _)| first <= code)
}

/// Check one label. `decoded` labels came from Punycode and must also be
/// shown to be in NFC (criterion 1); other labels were normalized already.
pub(super) fn check_label(label: &[char], decoded: bool) -> Result<(), IdnaError> {
    let Some(&first) = label.first() else {
        return Ok(());
    };
    if decoded && !normalize::is_nfc(&label.iter().collect::<String>()) {
        return Err(IdnaError::InvalidLabel);
    }
    // Criterion 4 (CheckHyphens=false), 5 and 6.
    if label.starts_with(&['x', 'n', '-', '-']) || label.contains(&'.') || is_mark(first) {
        return Err(IdnaError::InvalidLabel);
    }
    // Criterion 7.
    if !label.iter().all(|&value| mapping::is_valid(value)) {
        return Err(IdnaError::DisallowedCodePoint);
    }
    // Criterion 8 (CheckJoiners=true).
    for (index, &value) in label.iter().enumerate() {
        if (value == ZERO_WIDTH_NON_JOINER || value == ZERO_WIDTH_JOINER)
            && !satisfies_context_j(label, index)
        {
            return Err(IdnaError::ContextJ);
        }
    }
    Ok(())
}

/// RFC 5892 Appendix A.1 (ZWNJ) and A.2 (ZWJ).
fn satisfies_context_j(label: &[char], index: usize) -> bool {
    use JoiningType::*;
    if index > 0 && normalize::combining_class(label[index - 1]) == VIRAMA_COMBINING_CLASS {
        return true;
    }
    if label[index] == ZERO_WIDTH_JOINER {
        return false;
    }
    // (Joining_Type:{L,D})(Joining_Type:T)* ZWNJ (Joining_Type:T)*(Joining_Type:{R,D})
    let before = label[..index]
        .iter()
        .rev()
        .map(|&value| joining_type(value))
        .find(|&kind| kind != Some(Transparent));
    let after = label[index + 1..]
        .iter()
        .map(|&value| joining_type(value))
        .find(|&kind| kind != Some(Transparent));
    matches!(before, Some(Some(Left | Dual))) && matches!(after, Some(Some(Right | Dual)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(label: &str) -> Result<(), IdnaError> {
        check_label(&label.chars().collect::<Vec<_>>(), true)
    }

    #[test]
    fn applies_label_criteria() {
        assert_eq!(check("example"), Ok(()));
        assert_eq!(check(""), Ok(()));
        assert_eq!(check("xn--a"), Err(IdnaError::InvalidLabel));
        assert_eq!(check("\u{300}a"), Err(IdnaError::InvalidLabel));
        assert_eq!(check("e\u{301}"), Err(IdnaError::InvalidLabel));
        assert_eq!(check("\u{FF21}"), Err(IdnaError::DisallowedCodePoint));
        assert_eq!(check("a\u{80}"), Err(IdnaError::DisallowedCodePoint));
        assert_eq!(check("-a-"), Ok(()));
    }

    #[test]
    fn applies_context_j() {
        // ZWNJ between dual-joining Arabic letters, with a transparent mark.
        assert_eq!(check("\u{628}\u{64E}\u{200C}\u{628}"), Ok(()));
        // ZWNJ and ZWJ after a virama.
        assert_eq!(check("\u{915}\u{94D}\u{200C}"), Ok(()));
        assert_eq!(check("\u{915}\u{94D}\u{200D}"), Ok(()));
        assert_eq!(check("a\u{200C}b"), Err(IdnaError::ContextJ));
        assert_eq!(check("\u{628}\u{200D}\u{628}"), Err(IdnaError::ContextJ));
        assert_eq!(check("\u{200C}"), Err(IdnaError::ContextJ));
    }
}
