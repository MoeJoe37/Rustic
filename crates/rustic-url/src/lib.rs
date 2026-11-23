//! Original URL parsing and origin records for Rustic.
//!
//! Supports special network schemes with ASCII and Unicode (UTS #46 IDNA)
//! domains, IPv4/IPv6, `file:` URLs with Windows drive-letter rules, generic
//! hierarchical URLs and opaque paths. [`UrlParser::parse_with_diagnostics`]
//! also reports standard validation errors with byte offsets. Component setters
//! use [`UrlParser::with_component`]. Blob URL registries remain future work.
//! Parsing does not touch the file system,
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

mod diagnostics;
mod encoding;
mod error;
mod form;
mod host;
pub mod idna;
mod ipv4;
mod ipv6;
mod origin;
mod parser;
mod record;

pub use diagnostics::{Diagnostic, MAX_DIAGNOSTICS, ParseReport, ValidationError};
pub use error::ParseError;
pub use form::{parse_form, serialize_form};
pub use host::Host;
pub use origin::Origin;
pub use parser::{UrlComponent, UrlLimits, UrlParser};
pub use record::{Url, UrlPath};
