//! Bounded CSS syntax tokenization and parsing for Rustic.
//!
//! Implements CSS Syntax Module Level 3 (W3C Editor's Draft, reviewed
//! 2026-11-08): tokenization with escapes, comments and bad-string/bad-URL
//! recovery; component values; and the stylesheet, rule-list, rule,
//! declaration, block-contents (including CSS Nesting) and component-value
//! entry points. Every node carries a byte span into the caller's input.
//!
//! Fatal limits (input bytes, tokens, nesting, cancellation) return a
//! [`CssError`] with no partial result; recoverable syntax errors are kept as
//! bounded [`Diagnostic`]s while parsing continues. All algorithms are
//! iterative. No selector, property or cascade semantics live here.

#![warn(missing_docs)]

mod error;
mod parser;
mod rules;
mod token;
mod tokenizer;
mod values;

pub use error::{CssError, Diagnostic, DiagnosticKind};
pub use parser::{CssLimits, CssParser, Parsed, Stylesheet, parse_block_contents};
pub use rules::{AtRule, BlockItem, Declaration, QualifiedRule, Rule};
pub use token::{Numeric, Token};
pub use tokenizer::SpannedToken;
pub use values::{BlockKind, ComponentValue, Value};
