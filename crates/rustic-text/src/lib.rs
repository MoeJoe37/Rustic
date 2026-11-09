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
//! CFF/CFF2 outlines, font collections, WOFF/WOFF2, variations, hinting,
//! shaping (GSUB/GPOS/kern), bidi, line breaking and system font discovery
//! are not implemented here (TD-42).

#![warn(missing_docs)]

pub mod bundled;
mod cmap;
mod database;
mod error;
mod face;
mod glyf;
mod metrics;
mod reader;

pub use database::{FaceQuery, FamilyRef, FontDatabase, best_face};
pub use error::{FontError, Tag};
pub use face::{Face, FaceStyle, FontLimits, GlyphId};
pub use glyf::{Bounds, Outline, OutlinePoint};
pub use metrics::{EmRatios, FontMetrics};
