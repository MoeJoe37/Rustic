//! Syntax errors (ECMA-262 early errors and grammar mismatches).

use std::borrow::Cow;
use std::fmt;

use crate::source::Span;

/// A `SyntaxError` found while scanning or parsing, with the source span
/// it refers to. Messages hold no source text, so they are safe to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    /// Where the error is.
    pub span: Span,
    /// What is wrong.
    pub message: Cow<'static, str>,
}

impl SyntaxError {
    /// An error at `span`.
    pub fn new(span: Span, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SyntaxError at {}: {}", self.span, self.message)
    }
}

impl std::error::Error for SyntaxError {}
