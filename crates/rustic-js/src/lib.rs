//! Rustic's original ECMAScript implementation (M4).
//!
//! This crate starts with JS-01: source text as UTF-16 code units
//! ([`Source`]), the lexical grammar ([`Lexer`]) and a parser from scripts
//! and modules to an abstract syntax tree ([`ast`]) with source spans, early
//! errors, strict mode and automatic semicolon insertion. JS-02 adds the
//! [`runtime`] data model: 8-byte values, strings, objects with shapes,
//! conversions, environments and references. JS-03 collects the heap, and
//! JS-04 ([`vm`]) compiles scripts to register bytecode and runs them. No existing
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
pub mod regexp;
pub mod runtime;
mod source;
pub mod test262;
mod token;
pub mod unicode;
pub mod vm;

pub use error::SyntaxError;
pub use lexer::{Checkpoint, Goal, Lexer};
pub use parser::{ParseError, ParseLimits, parse_module, parse_script};
pub use source::{MAX_SOURCE_UNITS, Position, Source, SourceTooLong, Span};
pub use token::{Punct, TemplatePart, Token, TokenKind};
pub use vm::{CodeId, CompileError, EvalError, VmLimits};
