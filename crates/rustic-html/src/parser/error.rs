use std::fmt;

use rustic_dom::DomError;

use crate::HtmlError;

/// Construction families deliberately rejected until their tree rules exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnsupportedFeature {
    /// HTML tree construction does not yet insert processing instructions.
    ProcessingInstructions,
}

/// Fatal parser condition, distinct from recoverable HTML syntax errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// Byte decoding, tokenization, token budgets or cancellation failed.
    Tokenizer(HtmlError),
    /// A checked document operation or allocation failed.
    Document(DomError),
    /// Required tree-construction behavior is outside the implemented subset.
    Unsupported(UnsupportedFeature),
    /// Input has closed, but the completed tree has not been transferred.
    InputClosed,
    /// A successful EOF already transferred the document to the caller.
    AlreadyFinished,
    /// A previous fatal condition discarded this parser's partial document.
    Poisoned,
}

impl From<HtmlError> for ParseError {
    fn from(value: HtmlError) -> Self {
        Self::Tokenizer(value)
    }
}

impl From<DomError> for ParseError {
    fn from(value: DomError) -> Self {
        Self::Document(value)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tokenizer(error) => write!(f, "HTML tokenizer: {error}"),
            Self::Document(error) => write!(f, "HTML document: {error}"),
            Self::Unsupported(feature) => {
                write!(f, "unsupported HTML tree construction: {feature:?}")
            }
            Self::InputClosed => f.write_str("HTML document parser input is closed"),
            Self::AlreadyFinished => f.write_str("HTML document parser already finished"),
            Self::Poisoned => f.write_str("HTML document parser previously failed"),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tokenizer(error) => Some(error),
            Self::Document(error) => Some(error),
            _ => None,
        }
    }
}

/// Recovery categories for the implemented insertion modes, not WHATWG error IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeDiagnosticKind {
    /// Non-whitespace content or EOF occurred before a doctype.
    MissingDoctype,
    /// A doctype did not match the standard HTML/legacy-compat syntax.
    NonconformingDoctype,
    /// A token was misplaced, ignored or processed in a recovery mode.
    UnexpectedToken,
    /// A non-void HTML start tag had an unacknowledged self-closing flag.
    UnacknowledgedSelfClosing,
    /// A body NUL character was ignored.
    UnexpectedNull,
    /// EOF interrupted a text-only element.
    EofInText,
    /// A closing body/html token or EOF encountered disallowed open elements.
    UnclosedElements,
}

/// A tree recovery at a zero-based tokenizer-delivery index (EOF included).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeDiagnostic {
    /// Recovery category.
    pub kind: TreeDiagnosticKind,
    /// Delivered-token index; not a byte, scalar or UTF-16 source offset.
    pub token_index: usize,
}
