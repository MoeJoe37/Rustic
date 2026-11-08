use std::fmt;
use std::ops::Range;

/// A half-open byte-offset range in one source buffer.
///
/// This does not encode UTF-16 offsets or identify a source revision. The caller
/// must retain the matching buffer; slicing checks its actual bounds/boundaries.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceSpan {
    start: usize,
    end: usize,
}

impl SourceSpan {
    /// Create a span with `start <= end`; the buffer may be validated later.
    pub const fn new(start: usize, end: usize) -> Result<Self, InvalidSpan> {
        if start > end {
            Err(InvalidSpan)
        } else {
            Ok(Self { start, end })
        }
    }

    /// Byte offsets suitable for checked buffer access.
    pub const fn range(self) -> Range<usize> {
        self.start..self.end
    }

    /// Number of bytes in the span.
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// Whether the span contains zero bytes.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Slice bytes, returning `None` if the span exceeds this buffer.
    pub fn slice_bytes(self, source: &[u8]) -> Option<&[u8]> {
        source.get(self.range())
    }

    /// Slice UTF-8 only at valid character boundaries and within this buffer.
    pub fn slice_str(self, source: &str) -> Option<&str> {
        source.get(self.range())
    }
}

/// A source span's start offset exceeds its end offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidSpan;

impl fmt::Display for InvalidSpan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("source span start exceeds end")
    }
}

impl std::error::Error for InvalidSpan {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reversed_out_of_bounds_and_split_utf8_spans() {
        assert_eq!(SourceSpan::new(4, 3), Err(InvalidSpan));
        let text = "aé";
        assert_eq!(SourceSpan::new(1, 3).unwrap().slice_str(text), Some("é"));
        assert_eq!(SourceSpan::new(1, 2).unwrap().slice_str(text), None);
        assert_eq!(
            SourceSpan::new(1, 2).unwrap().slice_bytes(text.as_bytes()),
            Some(&[0xc3][..])
        );
        assert_eq!(
            SourceSpan::new(0, usize::MAX).unwrap().slice_bytes(b"x"),
            None
        );
        assert!(SourceSpan::new(3, 3).unwrap().is_empty());
    }
}
