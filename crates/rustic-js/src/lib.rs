//! Rustic's original ECMAScript implementation (M4).
//!
//! This crate starts with JS-01: source text as UTF-16 code units
//! ([`Source`]), the lexical grammar ([`Lexer`]) and a parser from scripts
//! and modules to an abstract syntax tree ([`ast`]) with source spans, early
//! errors, strict mode and automatic semicolon insertion. Values, the heap
//! and execution follow in JS-02 onward; nothing here runs code. No existing
//! JavaScript engine is used (ADR 0001).
//!
//! Parsing is bounded: source length is checked up front, nesting depth is
//! limited so content cannot overflow the stack, and cancellation is polled
//! between statements.

#![warn(missing_docs)]

pub mod ast;
mod error;
mod lexer;
mod parser;
mod source;
pub mod test262;
mod token;
pub mod unicode;

pub use error::SyntaxError;
pub use lexer::{Checkpoint, Goal, Lexer};
pub use parser::{ParseError, ParseLimits, parse_module, parse_script};
pub use source::{MAX_SOURCE_UNITS, Position, Source, SourceTooLong, Span};
pub use token::{Punct, TemplatePart, Token, TokenKind};
