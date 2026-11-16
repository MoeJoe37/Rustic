//! JavaScript/DOM bindings. WEB-02 adds [Web IDL conversions](webidl) to the
//! existing node wrapper identity and shared collection across one document
//! and one JavaScript agent (ADR 0012, ADR 0016).
//!
//! A node's wrapper is a JS host object. Holding a wrapper keeps the node's
//! whole tree alive, a live tree keeps its nodes' wrappers and their
//! node-held values (event listeners, expandos) alive, and the document's
//! own tree is always live. Everything else, including node ↔ wrapper ↔
//! listener cycles, is collected: unreachable trees are destroyed and their
//! wrappers freed.
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

mod heap;
pub mod webidl;

pub use heap::{DomGcError, DomGcStats, DomHeap};
