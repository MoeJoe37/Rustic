//! The Bidi Rule for IDNA labels (RFC 5893 section 2), as required by UTS #46
//! CheckBidi.

use super::tables::BIDI_CLASSES;

/// Bidi_Class values that RFC 5893 names; every other class is `Other`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BidiClass {
    L,
    R,
    Al,
    En,
    Es,
    Et,
    An,
    Cs,
    Nsm,
    Bn,
    On,
    Other,
}

pub(super) fn bidi_class(value: char) -> BidiClass {
    let code = u32::from(value);
    // The table starts at U+0000, so the partition point is never zero.
    let index = BIDI_CLASSES.partition_point(|&(start, _)| start <= code) - 1;
    BIDI_CLASSES[index].1
}

/// An RTL label contains an R, AL or AN code point (RFC 5893 section 1.4); a
/// domain with at least one RTL label is a Bidi domain name.
pub(super) fn is_rtl_label(label: &[char]) -> bool {
    label.iter().any(|&value| {
        matches!(
            bidi_class(value),
            BidiClass::R | BidiClass::Al | BidiClass::An
        )
    })
}

/// All six numbered conditions of RFC 5893 section 2. Empty labels have no
/// directionality to check.
pub(super) fn satisfies_bidi_rule(label: &[char]) -> bool {
    use BidiClass::*;
    let Some(&first) = label.first() else {
        return true;
    };
    let classes = || label.iter().map(|&value| bidi_class(value));
    // Condition 3 and 6 look at the last class that is not NSM.
    let last = classes().rev().find(|&class| class != Nsm);
    match bidi_class(first) {
        R | Al => {
            // Conditions 2, 3 and 4.
            classes().all(|class| matches!(class, R | Al | An | En | Es | Cs | Et | On | Bn | Nsm))
                && matches!(last, Some(R | Al | En | An))
                && !(classes().any(|class| class == En) && classes().any(|class| class == An))
        }
        L => {
            // Conditions 5 and 6.
            classes().all(|class| matches!(class, L | En | Es | Cs | Et | On | Bn | Nsm))
                && matches!(last, Some(L | En))
        }
        // Condition 1.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(label: &str) -> bool {
        satisfies_bidi_rule(&label.chars().collect::<Vec<_>>())
    }

    #[test]
    fn classifies_defaults_and_explicit_values() {
        assert_eq!(bidi_class('a'), BidiClass::L);
        assert_eq!(bidi_class('\u{5D0}'), BidiClass::R);
        assert_eq!(bidi_class('\u{627}'), BidiClass::Al);
        assert_eq!(bidi_class('\u{661}'), BidiClass::An);
        assert_eq!(bidi_class('1'), BidiClass::En);
        assert_eq!(bidi_class('\u{300}'), BidiClass::Nsm);
        // Unassigned code point in a block with an R default.
        assert_eq!(bidi_class('\u{5FF}'), BidiClass::R);
        assert_eq!(bidi_class(' '), BidiClass::Other);
    }

    #[test]
    fn applies_rfc_5893_conditions() {
        assert!(check("abc"));
        assert!(check("\u{5D0}\u{5D1}"));
        assert!(check("\u{5D0}1"));
        assert!(check("a\u{300}"));
        assert!(!check("1\u{5D0}"));
        assert!(!check("\u{5D0}a"));
        assert!(!check("a\u{5D0}"));
        assert!(!check("\u{627}1\u{661}"));
        assert!(!check("a-"));
        assert!(check(""));
    }
}
