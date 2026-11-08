//! Initial M3 layout contracts: checked geometry, principal box construction
//! and non-replaced normal-flow block width measurement.
//!
//! This is not yet a document renderer. Anonymous/line boxes, vertical flow,
//! margin collapse, fragments, text shaping and painting remain unimplemented.
//! The immutable box snapshot consumes M2 computed styles without recascading.

#![warn(missing_docs)]

mod geometry;
mod sizing;
mod tree;

pub use geometry::{
    CssPoint, CssPx, CssRect, CssSize, DevicePx, DeviceScale, GeometryError, MAX_CSS_PX,
};
pub use sizing::{BlockMeasure, Edges, MeasureError, measure_block_width};
pub use tree::{BoxKind, BoxSnapshot, BoxTree, LayoutError, LayoutLimits};
