//! Original URL parsing and origin records for Rustic.
//!
//! Supports special network schemes with ASCII domains, IPv4/IPv6, generic
//! hierarchical URLs and opaque paths. File/blob URLs, Unicode IDNA processing,
//! legacy query encodings and URL setters are not implemented. Parsing does not
//! load resources, authorize navigation, or assign a document's inherited origin.
//!
//! ```
//! use rustic_url::Url;
//! let base = Url::parse("https://example.test/docs/index.html", None)?;
//! let target = Url::parse("../assets/logo.svg", Some(&base))?;
//! assert_eq!(target.serialize(), "https://example.test/assets/logo.svg");
//! assert_eq!(target.origin(), base.origin());
//! # Ok::<(), rustic_url::ParseError>(())
//! ```

#![warn(missing_docs)]

mod encoding;
mod error;
mod host;
mod ipv4;
mod ipv6;
mod origin;
mod parser;
mod record;

pub use error::ParseError;
pub use host::Host;
pub use origin::Origin;
pub use parser::{UrlLimits, UrlParser};
pub use record::{Url, UrlPath};
