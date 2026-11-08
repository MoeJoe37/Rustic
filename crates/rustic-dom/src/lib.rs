//! Document-owned node storage for Rustic's parser and future host bindings.
//!
//! This is an internal tree core, not a complete implementation of browser DOM
//! APIs. Detached nodes remain allocated until explicitly destroyed. Destruction
//! is an engine operation, not a substitute for the future tracing collector.
//!
//! ```
//! use rustic_dom::{Document, DocumentLimits, DomString, NodeKind, QualifiedName};
//! let mut document = Document::new(DocumentLimits::default())?;
//! let element = document.create_element(QualifiedName::html("html")?)?;
//! document.append_child(document.root(), element)?;
//! let text = document.create_text("Hello, Rustic")?;
//! document.append_child(element, text)?;
//! document.append_utf8_character_data(text, " 🚀")?;
//! let NodeKind::Text(value) = document.node(text)?.kind() else { unreachable!() };
//! assert_eq!(value, &DomString::from("Hello, Rustic 🚀"));
//! assert_eq!(document.node(text)?.parent(), Some(element));
//! # Ok::<(), rustic_dom::DomError>(())
//! ```

#![warn(missing_docs)]

mod document;
mod dump;
mod error;
mod name;
mod node;
mod storage;
mod string;
mod traversal;

pub use document::tracing::{
    CollectionStats, HostHeap, HostHeapError, HostHeapLimits, HostObjectId, Traced,
};
pub use document::{Adopted, Document, DocumentLimits, InvariantViolation, ReadyState};
pub use dump::DumpError;
pub use error::DomError;
pub use name::{HTML_NAMESPACE, MATHML_NAMESPACE, QualifiedName, SVG_NAMESPACE};
pub use node::{
    Attribute, DocumentType, Element, Node, NodeKind, ProcessingInstruction, ShadowRootMode,
};
pub use rustic_base::{DocumentId, NodeId};
pub use string::DomString;
pub use traversal::Walk;
