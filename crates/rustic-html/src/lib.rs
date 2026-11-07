//! Original incremental HTML input and tokenization foundations.
//!
//! The tokenizer supports data, tags, ordered attributes, comments, doctypes and
//! processing instructions and named/numeric references in HTML context.
//! RCDATA/RAWTEXT/script/plaintext modes use caller-selected contexts. Tree
//! construction is not implemented; this does not yet parse complete documents.
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
//!
//! The pull driver permits tree-builder feedback before later input is processed:
//!
//! ```
//! use rustic_html::{HtmlTokenizer, Token, TokenizerMode};
//! let mut tokenizer = HtmlTokenizer::default();
//! tokenizer.write(b"<title>Rustic &amp; <b></title>")?;
//! assert!(matches!(tokenizer.next_token()?, Some(Token::StartTag(tag)) if tag.name() == "title"));
//! tokenizer.set_text_mode(TokenizerMode::Rcdata)?;
//! tokenizer.end()?;
//! assert_eq!(tokenizer.next_token()?, Some(Token::Characters("Rustic & <b>".to_owned())));
//! assert_eq!(tokenizer.next_token()?, Some(Token::EndTag("title".to_owned())));
//! assert_eq!(tokenizer.next_token()?, Some(Token::Eof));
//! assert_eq!(tokenizer.next_token()?, None);
//! # Ok::<(), rustic_html::HtmlError>(())
//! ```

#![warn(missing_docs)]

mod entities;
mod error;
mod input;
mod token;
mod tokenizer;

pub use error::{Diagnostic, DiagnosticKind, HtmlError};
pub use input::Utf8Input;
pub use token::{Attribute, Doctype, ProcessingInstruction, Tag, Token};
pub use tokenizer::{HtmlTokenizer, TokenizerLimits, TokenizerMode};
