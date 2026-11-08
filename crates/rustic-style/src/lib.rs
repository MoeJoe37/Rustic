//! Selector matching and the cascade, and later computed style, for Rustic.
//!
//! Matching works over any [`ElementView`]: a read-only, copyable handle to an
//! element that can reach its parent and sibling elements. [`DomElement`]
//! implements it for `rustic-dom` documents, and [`query_selector_all`] walks a
//! subtree in tree order.
//!
//! [`StyleSheet::parse`] prepares a stylesheet for the cascade, and
//! [`Cascade::specified_style`] (or [`style_tree`] for a whole document) picks
//! each longhand's winning declaration by origin, importance, `style`
//! attribute, specificity and order, resolves the CSS-wide keywords and
//! applies inheritance.
//!
//! The matcher runs right to left with an explicit stack and the usual
//! "restart from the closest descendant/sibling combinator" pruning, so
//! descendant chains never backtrack exponentially and long selectors never
//! recurse. Recursion happens only into `:is()`, `:where()`, `:not()` and
//! `:nth-*(of S)` arguments, whose depth the parser bounds at
//! [`rustic_css::MAX_SELECTOR_NESTING`].
//!
//! ```
//! use rustic_css::{CssParser, NamespaceMap};
//! use rustic_html::HtmlDocumentParser;
//! use rustic_style::{MatchContext, query_selector_all};
//!
//! let mut parser = HtmlDocumentParser::default();
//! parser.feed(b"<ul><li>one<li class=x>two<li>three</ul>")?;
//! let result = parser.finish()?;
//! let document = result.document();
//! let list = CssParser::default()
//!     .selector_list("ul > li:nth-child(odd):not(.x)", &NamespaceMap::new())?
//!     .value?;
//! let found = query_selector_all(document, document.root(), &list, &MatchContext::html())?;
//! assert_eq!(found.len(), 2);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]

mod cascade;
mod dom;
mod matching;
mod sheet;
mod text;
mod view;

pub use cascade::{Cascade, SpecifiedStyle};
pub use dom::{DomElement, ElementStyle, query_selector_all, style_tree};
pub use matching::{MatchContext, matches, matches_complex};
pub use sheet::{
    Origin, StyleDiagnostic, StyleDiagnosticKind, StyleRule, StyleSheet, parse_style_attribute,
};
pub use view::{AttributeRef, ElementView};
