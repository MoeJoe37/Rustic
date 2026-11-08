//! UTS #46 section 5 IDNA Mapping Table lookup.

use super::tables::{IDNA_MAPPINGS, IDNA_STATUS};

/// A code point's IDNA status. `Mapped` addresses UTF-8 replacement text in
/// the generated pool by byte offset and byte length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Status {
    Valid,
    Ignored,
    Mapped(u16, u8),
    Deviation,
    Disallowed,
}

pub(super) fn status(value: char) -> Status {
    let code = u32::from(value);
    // The table starts at U+0000, so the partition point is never zero.
    let index = IDNA_STATUS.partition_point(|&(start, _)| start <= code) - 1;
    IDNA_STATUS[index].1
}

/// Whether a code point may appear in a label after processing. Deviation
/// characters stay valid because the URL Standard uses nontransitional
/// processing.
pub(super) fn is_valid(value: char) -> bool {
    matches!(status(value), Status::Valid | Status::Deviation)
}

/// UTS #46 processing step 1 (Map), with Transitional_Processing=false.
/// Disallowed code points are kept so that validation reports them.
pub(super) fn map(input: &str, output: &mut String) {
    for value in input.chars() {
        if value.is_ascii() {
            output.push(value.to_ascii_lowercase());
            continue;
        }
        match status(value) {
            Status::Valid | Status::Deviation | Status::Disallowed => output.push(value),
            Status::Ignored => {}
            Status::Mapped(offset, length) => {
                let start = usize::from(offset);
                output.push_str(&IDNA_MAPPINGS[start..start + usize::from(length)]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_cover_table_edges() {
        assert_eq!(status('\0'), Status::Valid);
        assert_eq!(status('a'), Status::Valid);
        assert!(matches!(status('A'), Status::Mapped(_, 1)));
        assert_eq!(status('\u{AD}'), Status::Ignored);
        assert_eq!(status('\u{DF}'), Status::Deviation);
        assert_eq!(status('\u{80}'), Status::Disallowed);
        assert_eq!(status('\u{10FFFF}'), Status::Disallowed);
    }

    #[test]
    fn maps_case_compatibility_and_ignored_code_points() {
        let mut output = String::new();
        map("A\u{AD}\u{FF21}\u{2160}\u{DF}", &mut output);
        assert_eq!(output, "aai\u{DF}");
    }
}
