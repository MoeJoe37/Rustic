//! Original incremental HTML input, tokenization and document construction.
//!
//! The tokenizer supports data, tags, ordered attributes, comments, doctypes and
//! processing instructions and named/numeric references in HTML context.
//! RCDATA/RAWTEXT/script/plaintext modes use caller-selected contexts. The inert
//! document parser implements complete WHATWG document tree construction with
//! scripting disabled; fragment parsing and script integration are not provided.
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
//! Build a bounded local document with implied HTML/head/body elements:
//!
//! ```
//! use rustic_html::{DocumentMode, HtmlDocumentParser};
//! let mut parser = HtmlDocumentParser::default();
//! parser.feed(b"<!doctype html><title>Rustic &amp; trees</title><p>Hello")?;
//! let result = parser.finish()?;
//! assert_eq!(result.mode(), DocumentMode::NoQuirks);
//! assert!(result.tree_diagnostics().is_empty());
//! assert!(result.document().dump_tree(result.document().root(), 4096)?.contains("Hello"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
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
mod parser;
mod token;
mod tokenizer;

pub use error::{Diagnostic, DiagnosticKind, HtmlError};
pub use input::Utf8Input;
pub use parser::{
    ContextNamespace, DocumentMode, FragmentContext, HtmlDocumentParser, HtmlFragmentParser,
    ParseError, ParsedDocument, ParsedFragment, ParserLimits, TreeDiagnostic, TreeDiagnosticKind,
    UnsupportedFeature,
};
pub use token::{Attribute, Doctype, ProcessingInstruction, Tag, Token};
pub use tokenizer::{HtmlTokenizer, TokenizerLimits, TokenizerMode};
