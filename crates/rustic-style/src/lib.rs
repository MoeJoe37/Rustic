//! Selector matching, and later the cascade and computed style, for Rustic.
//!
//! Matching works over any [`ElementView`]: a read-only, copyable handle to an
//! element that can reach its parent and sibling elements. [`DomElement`]
//! implements it for `rustic-dom` documents, and [`query_selector_all`] walks a
//! subtree in tree order.
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

mod dom;
mod matching;
mod text;
mod view;

pub use dom::{DomElement, query_selector_all};
pub use matching::{MatchContext, matches, matches_complex};
pub use view::{AttributeRef, ElementView};
