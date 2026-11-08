//! Fatal errors and recoverable syntax diagnostics.

use std::fmt;

use rustic_base::SourceSpan;

/// A fatal condition: no result is returned and nothing partial escapes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CssError {
    /// A limit was zero or otherwise unusable.
    InvalidLimits,
    /// The input exceeds `max_input_bytes`.
    InputTooLarge,
    /// The input produced more than `max_tokens` tokens.
    TokenLimit,
    /// Blocks and functions nest deeper than `max_nesting`.
    NestingLimit,
    /// Cooperative cancellation was requested.
    Cancelled,
    /// Memory for the result could not be reserved.
    AllocationFailed,
}

impl fmt::Display for CssError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLimits => "CSS parser limits must be nonzero",
            Self::InputTooLarge => "CSS input exceeds its byte limit",
            Self::TokenLimit => "CSS input exceeds its token limit",
            Self::NestingLimit => "CSS blocks nest deeper than the limit",
            Self::Cancelled => "CSS parsing was cancelled",
            Self::AllocationFailed => "CSS parser allocation failed",
        })
    }
}

impl std::error::Error for CssError {}

/// A recoverable parse error; parsing continues as the standard specifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    /// A comment was not closed before EOF.
    EofInComment,
    /// A string was not closed before EOF.
    EofInString,
    /// An unescaped newline ended a string (bad-string).
    NewlineInString,
    /// An unquoted URL was not closed before EOF.
    EofInUrl,
    /// An unquoted URL contained a quote, `(`, whitespace or a bad escape.
    BadUrl,
    /// A backslash was followed by a newline outside a string.
    InvalidEscape,
    /// An escape at EOF produced U+FFFD.
    EofInEscape,
    /// A block or function was not closed before EOF.
    EofInBlock,
    /// A qualified rule had no `{}` block before EOF.
    EofInRule,
    /// An at-rule had no `;` or block before EOF.
    EofInAtRule,
    /// Content that is not a valid rule or declaration was dropped.
    InvalidRule,
    /// A declaration was malformed and dropped.
    InvalidDeclaration,
    /// A closing `}` had no matching open block where a rule was expected.
    UnmatchedClose,
    /// An entry point expected one value or rule but found none.
    EmptyInput,
    /// An entry point expected one value or rule but found more.
    ExtraInput,
}

/// A diagnostic with its byte span in the original input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// What was recovered from.
    pub kind: DiagnosticKind,
    /// Where, as byte offsets into the caller's input.
    pub span: SourceSpan,
}
