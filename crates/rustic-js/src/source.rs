//! ECMAScript source text as UTF-16 code units, with spans and positions.
//!
//! ECMA-262 §11.1 defines source text as a sequence of code points, but
//! string values, `String.prototype` indexes and web-facing offsets are in
//! UTF-16 code units, so Rustic keeps source as code units and decodes code
//! points (pairing surrogates) while scanning. Spans are UTF-16 offsets.

use std::fmt;

/// A half-open range of UTF-16 code unit offsets in one [`Source`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    /// Offset of the first code unit.
    pub start: u32,
    /// Offset one past the last code unit.
    pub end: u32,
}

impl Span {
    /// The span from `start` to `end`.
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// The smallest span covering `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }

    /// Length in code units.
    pub fn len(self) -> u32 {
        self.end - self.start
    }

    /// Whether the span is empty.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A one-based line and column. Columns count UTF-16 code units from the
/// start of the line, as browsers report them for script errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// One-based line; CR LF, LF, CR, LS and PS each end one line.
    pub line: u32,
    /// One-based UTF-16 column.
    pub column: u32,
}

/// Why source text was refused before scanning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceTooLong {
    /// The limit that was exceeded, in code units.
    pub limit: usize,
}

impl fmt::Display for SourceTooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "source text exceeds the limit of {} UTF-16 code units",
            self.limit
        )
    }
}

impl std::error::Error for SourceTooLong {}

/// Most code units any source may hold, so every offset fits in a `u32`.
pub const MAX_SOURCE_UNITS: usize = u32::MAX as usize - 1;

/// Immutable ECMAScript source text.
#[derive(Clone, PartialEq, Eq)]
pub struct Source {
    units: Vec<u16>,
    /// Offsets where each line after the first starts.
    line_starts: Vec<u32>,
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source")
            .field("units", &self.units.len())
            .field("lines", &(self.line_starts.len() + 1))
            .finish()
    }
}

impl Source {
    /// Source text from UTF-16 code units (which may hold lone surrogates,
    /// as script text decoded by the HTML loader can). Fails when it is
    /// longer than `limit` code units or [`MAX_SOURCE_UNITS`].
    pub fn from_utf16(units: Vec<u16>, limit: usize) -> Result<Self, SourceTooLong> {
        let limit = limit.min(MAX_SOURCE_UNITS);
        if units.len() > limit {
            return Err(SourceTooLong { limit });
        }
        let mut line_starts = Vec::new();
        let mut i = 0;
        while i < units.len() {
            let unit = units[i];
            i += 1;
            match unit {
                0x0D if units.get(i) == Some(&0x0A) => {
                    i += 1;
                    line_starts.push(offset(i));
                }
                0x0A | 0x0D | 0x2028 | 0x2029 => line_starts.push(offset(i)),
                _ => {}
            }
        }
        Ok(Self { units, line_starts })
    }

    /// Source text from a Rust string, checked against `limit` code units.
    pub fn from_str_limited(text: &str, limit: usize) -> Result<Self, SourceTooLong> {
        let limit = limit.min(MAX_SOURCE_UNITS);
        // Each byte yields at most one code unit, so an early bound check
        // avoids encoding oversized text.
        let mut units = Vec::with_capacity(text.len().min(limit.saturating_add(1)));
        for unit in text.encode_utf16() {
            if units.len() == limit {
                return Err(SourceTooLong { limit });
            }
            units.push(unit);
        }
        Self::from_utf16(units, limit)
    }

    /// The code units.
    pub fn units(&self) -> &[u16] {
        &self.units
    }

    /// Number of code units.
    pub fn len(&self) -> usize {
        self.units.len()
    }

    /// Whether the source is empty.
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// The code units of `span`, clamped to the source.
    pub fn slice(&self, span: Span) -> &[u16] {
        let end = (span.end as usize).min(self.units.len());
        let start = (span.start as usize).min(end);
        &self.units[start..end]
    }

    /// The text of `span`, with lone surrogates replaced by U+FFFD.
    pub fn text(&self, span: Span) -> String {
        String::from_utf16_lossy(self.slice(span))
    }

    /// The line and column of code unit `offset` (clamped to the end).
    pub fn position(&self, offset: u32) -> Position {
        let offset = offset.min(self.units.len() as u32);
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let start = if line == 0 {
            0
        } else {
            self.line_starts[line - 1]
        };
        Position {
            line: line as u32 + 1,
            column: offset - start + 1,
        }
    }
}

/// `i` as an offset; callers only pass indexes into a source checked
/// against [`MAX_SOURCE_UNITS`].
fn offset(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_every_line_terminator_and_utf16_columns() {
        let source = Source::from_str_limited("a\r\nb\rc\nd\u{2028}e\u{2029}😀f", 100).unwrap();
        let at = |i| {
            let p = source.position(i);
            (p.line, p.column)
        };
        assert_eq!(at(0), (1, 1));
        assert_eq!(at(3), (2, 1)); // b after CR LF
        assert_eq!(at(5), (3, 1));
        assert_eq!(at(7), (4, 1));
        assert_eq!(at(9), (5, 1));
        assert_eq!(at(11), (6, 1));
        assert_eq!(at(13), (6, 3)); // f after a surrogate pair
        assert_eq!(at(1000), (6, 4));
    }

    #[test]
    fn length_limits_are_exact() {
        assert!(Source::from_str_limited("abc", 3).is_ok());
        assert_eq!(
            Source::from_str_limited("abcd", 3),
            Err(SourceTooLong { limit: 3 })
        );
        assert!(Source::from_utf16(vec![0xD800; 4], 4).is_ok());
        assert!(Source::from_utf16(vec![0xD800; 5], 4).is_err());
    }
}
