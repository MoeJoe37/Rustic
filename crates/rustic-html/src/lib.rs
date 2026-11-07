//! Original incremental HTML input and tokenization foundations.
//!
//! The initial tokenizer supports data, tag names, ordered attributes and
//! self-closing syntax. Character references and markup declarations return
//! explicit unsupported errors. Tree construction and alternate text modes are
//! not implemented. This crate does not yet parse complete HTML documents.
//!
//! ```
//! use rustic_html::{HtmlTokenizer, Token};
//!
//! let mut tokenizer = HtmlTokenizer::default();
//! let mut tokens = tokenizer.feed(b"<B cl")?;
//! tokens.extend(tokenizer.feed(b"ass='sample'>Hello</B>")?);
//! tokens.extend(tokenizer.finish()?);
//! assert!(matches!(&tokens[0], Token::StartTag(tag) if tag.name() == "b"));
//! assert_eq!(tokens[1], Token::Characters("Hello".to_owned()));
//! assert_eq!(tokens.last(), Some(&Token::Eof));
//! # Ok::<(), rustic_html::HtmlError>(())
//! ```

#![warn(missing_docs)]

mod error;
mod input;
mod token;
mod tokenizer;

pub use error::{Diagnostic, DiagnosticKind, HtmlError};
pub use input::Utf8Input;
pub use token::{Attribute, Tag, Token};
pub use tokenizer::{HtmlTokenizer, TokenizerLimits};
