//! From-scratch font loading for Rustic (REN-03a): a bounds-checked
//! sfnt/TrueType parser, CSS font matching and the metrics behind the
//! font-relative CSS units `ex`, `ch`, `cap` and `ic`.
//!
//! [`Face::parse`] validates the table directory and the `head`, `hhea`,
//! `maxp`, `hmtx`, `cmap` (formats 4 and 12), `name`, optional `OS/2`, and
//! `loca` tables when a font loads; `glyf` outlines are parsed on demand,
//! with composite glyphs flattened iteratively under [`FontLimits`]. Every
//! read is checked: hostile fonts produce a [`FontError`], never a panic.
//!
//! [`segment`] provides UAX #29 grapheme clusters and UAX #14 line break
//! opportunities, and [`bidi`] the UAX #9 bidi algorithm, from generated
//! Unicode 17.0.0 tables ([`unicode`]); see docs/TEXT.md.
//!
//! CFF/CFF2 and WOFF formats and OpenType shaping use reviewed low-level
//! adapters (ADR 0014). Discovery is explicit host-granted local I/O. See
//! docs/TEXT.md and TD-85/86/87 for limits and remaining native/font scope.

#![warn(missing_docs)]

pub mod bidi;
pub mod bundled;
mod cff;
mod cmap;
mod containers;
pub use containers::{collection_face, collection_len};
mod discovery;
pub mod hyphen;
pub mod native;
pub use discovery::{DiscoveryError, DiscoveryLimits, DiscoveryReport, system_font_roots};
mod database;
mod error;
mod face;
mod glyf;
mod metrics;
mod reader;
mod scopes;
pub mod segment;
pub mod shape;
pub mod unicode;

pub use database::{FaceQuery, FamilyRef, FontDatabase, best_face};
pub use error::{FontError, Tag};
pub use face::{Face, FaceDescriptors, FaceStyle, FontLimits, GlyphId};
pub use glyf::{Bounds, Outline, OutlinePoint};
pub use metrics::{EmRatios, FontMetrics};
