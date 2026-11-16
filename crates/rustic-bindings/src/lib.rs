//! JavaScript/DOM bindings with [Web IDL conversions](webidl), realm-scoped
//! node wrappers and shared collection across an agent's registered
//! documents (ADR 0026).
//!
//! A node's wrapper is a JS host object. Holding a wrapper keeps the node's
//! whole tree alive. A live tree keeps its nodes' wrappers and node-held
//! values (such as event listeners) alive. A document's own tree is always
//! live. Joint collection frees unreachable trees and wrappers, including
//! node, wrapper and listener cycles across documents.
//!
//! ```
//! use rustic_bindings::DomHeap;
//! use rustic_dom::{Document, DocumentLimits, QualifiedName};
//! use rustic_js::runtime::{Agent, Roots, Value};
//!
//! let mut agent = Agent::new().unwrap();
//! let mut document = Document::new(DocumentLimits::default()).unwrap();
//! let mut dom = DomHeap::new(&document);
//! let div = document.create_element(QualifiedName::html("div").unwrap()).unwrap();
//! let wrapper = dom.wrap(&mut agent, &document, div).unwrap();
//! assert_eq!(dom.wrap(&mut agent, &document, div).unwrap(), wrapper);
//!
//! // Script holds the wrapper, so the detached element survives.
//! let held = [Value::object(wrapper)];
//! dom.collect(&mut agent, &mut document, Roots { values: &held, envs: &[], codes: &[] }).unwrap();
//! assert!(document.node(div).is_ok());
//!
//! // Nothing holds it any more.
//! let stats = dom.collect(&mut agent, &mut document, Roots::default()).unwrap();
//! assert_eq!(stats.nodes_destroyed, 1);
//! assert!(document.node(div).is_err());
//! ```

#![warn(missing_docs)]

pub mod dom_exception;
mod heap;
pub mod metadata;
pub mod page;
pub mod webidl;

pub use heap::{DomGcError, DomGcStats, DomHeap};
