//! Selector matching and the cascade, and later computed style, for Rustic.
//!
//! Matching works over any [`ElementView`]: a read-only, copyable handle to an
//! element that can reach its parent and sibling elements. [`DomElement`]
//! implements it for `rustic-dom` documents, and [`query_selector_all`] walks a
//! subtree in tree order.
//!
//! [`StyleSheet::parse`] prepares a stylesheet for the cascade, and
//! [`Cascade::specified_style`] (or [`style_tree`] for a whole document) picks
//! each longhand's winning declaration by origin and importance (including
//! the animation and transition origins), shadow-tree context, `style`
//! attribute, cascade layer, specificity and order, resolves the CSS-wide
//! keywords and applies inheritance.
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
mod computed;
mod dom;
mod html;
mod html_state;
mod invalidation;
mod layer;
mod loading;
mod matching;
mod sheet;
mod text;
mod variables;
mod view;

pub use cascade::{Cascade, SpecifiedStyle, StyleInputs, TreeContext, TreeScope};
pub use computed::{ComputeContext, ComputedStyle};
pub use dom::{DomElement, ElementStyle, query_selector_all, shadow_tree_context, style_tree};
pub use html::{
    HTML_USER_AGENT_STYLESHEET, StyleElementSheet, StyleElementStatus, document_cascade,
    document_cascade_with, dump_computed_styles, html_user_agent_sheet,
};
pub use invalidation::{InvalidationMap, Mutation, RestyleStats, StyledDocument};
pub use layer::{LayerName, LayerSegment};
pub use loading::{
    DEFAULT_MAX_EXTERNAL_BYTES, DEFAULT_MAX_EXTERNAL_SHEETS, DEFAULT_MAX_IMPORT_DEPTH, FetchError,
    ImportReport, NoFetch, SheetStatus, StyleLoadLimits, StyleLoader, StylesheetFetcher,
};
pub use matching::{MatchContext, matches, matches_complex};
pub use sheet::{
    ImportRule, Origin, StyleDiagnostic, StyleDiagnosticKind, StyleRule, StyleSheet,
    parse_style_attribute,
};
pub use variables::CustomProperties;
pub use view::{AttributeRef, ChildRef, ElementView};
