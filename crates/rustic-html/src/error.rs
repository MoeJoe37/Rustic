use std::fmt;

/// Fatal resource, lifecycle or explicitly unsupported tokenizer condition.
///
/// Fatal errors poison the stream. Earlier successful feed results remain valid;
/// a failing call returns no partial result and cannot be retried on that stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtmlError {
    /// Original input byte budget was exceeded.
    InputLimit,
    /// Total emitted token count, including EOF, exceeded the budget.
    TokenLimit,
    /// Current token/tag data exceeds the configured byte budget.
    TokenBytesLimit,
    /// Too many attributes were encountered in a tag, including duplicates.
    AttributeLimit,
    /// The bounded diagnostic collection is full.
    DiagnosticLimit,
    /// Text chunk size is zero or exceeds the token byte limit.
    InvalidLimits,
    /// Cooperative cancellation was requested.
    Cancelled,
    /// Finish was already called successfully.
    AlreadyFinished,
    /// A prior fatal error made further input invalid.
    Poisoned,
    /// Character-reference tokenization is not implemented yet.
    UnsupportedCharacterReference,
    /// Comments, doctypes, bogus comments and other declarations are not implemented.
    UnsupportedDeclaration,
}

impl fmt::Display for HtmlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InputLimit => "HTML input byte limit exceeded",
            Self::TokenLimit => "HTML token count limit exceeded",
            Self::TokenBytesLimit => "HTML token data limit exceeded",
            Self::AttributeLimit => "HTML attribute count limit exceeded",
            Self::DiagnosticLimit => "HTML diagnostic count limit exceeded",
            Self::InvalidLimits => "invalid HTML tokenizer limits",
            Self::Cancelled => "HTML processing cancelled",
            Self::AlreadyFinished => "HTML stream is already finished",
            Self::Poisoned => "HTML stream previously failed",
            Self::UnsupportedCharacterReference => "HTML character references are not implemented",
            Self::UnsupportedDeclaration => "HTML declarations/comments are not implemented",
        })
    }
}

impl std::error::Error for HtmlError {}

/// Recoverable errors in the initial tokenizer state subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    /// Null input in a tokenizer state with prescribed recovery.
    UnexpectedNull,
    /// A tag's attribute name was repeated; the first value is retained.
    DuplicateAttribute,
    /// End-tag attributes are ignored after parsing them.
    EndTagAttributes,
    /// Self-closing syntax on an end tag is ignored.
    EndTagSolidus,
    /// Invalid first character after a less-than sign.
    InvalidTagStart,
    /// Empty end-tag name before a greater-than sign.
    MissingEndTagName,
    /// Unexpected equals sign at the start of an attribute name.
    UnexpectedEquals,
    /// Character with prescribed recovery in an attribute name/value.
    UnexpectedAttributeCharacter,
    /// A closing attribute quote was followed by another name without whitespace.
    MissingAttributeWhitespace,
    /// A value was missing after an equals sign.
    MissingAttributeValue,
    /// A slash was not followed by the end of a tag.
    UnexpectedSolidus,
    /// EOF followed a tag-open sequence.
    EofBeforeTagName,
    /// EOF interrupted an unfinished tag, which was discarded.
    EofInTag,
}

/// Diagnostic location in the normalized scalar-value stream.
///
/// `normalized_offset` is neither an original byte offset nor a UTF-16 offset.
/// Original-byte span mapping and line/column diagnostics remain separate work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// The recoverable condition.
    pub kind: DiagnosticKind,
    /// Zero-based normalized code-point offset, or stream length at EOF.
    pub normalized_offset: usize,
}
