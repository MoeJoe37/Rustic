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
//! CFF/CFF2 outlines, font collections, WOFF/WOFF2, variations, hinting,
//! shaping (GSUB/GPOS/kern) and system font discovery are not implemented
//! here (TD-42); segmentation is untailored (TD-47).

#![warn(missing_docs)]

pub mod bidi;
pub mod bundled;
mod cmap;
mod database;
mod error;
mod face;
mod glyf;
mod metrics;
mod reader;
pub mod segment;
pub mod shape;
pub mod unicode;

pub use database::{FaceQuery, FamilyRef, FontDatabase, best_face};
pub use error::{FontError, Tag};
pub use face::{Face, FaceStyle, FontLimits, GlyphId};
pub use glyf::{Bounds, Outline, OutlinePoint};
pub use metrics::{EmRatios, FontMetrics};
