//! Initial M3 layout contracts: checked geometry, principal box construction
//! and non-replaced normal-flow block width measurement.
//!
//! A block-only vertical reference now emits checked used rectangles and
//! includes adjoining/parent-child/empty-block margin collapse. It is not yet
//! a document renderer: anonymous/line boxes, text shaping, general fragments
//! and painting remain unimplemented.
//! The immutable box snapshot consumes M2 computed styles without recascading.

#![warn(missing_docs)]

mod flow;
mod geometry;
mod height;
mod margins;
mod sizing;
mod tree;

pub use flow::{BlockFragment, BlockLayout, FlowError, UnsupportedFlow};
pub use geometry::{
    CssPoint, CssPx, CssRect, CssSize, DevicePx, DeviceScale, GeometryError, MAX_CSS_PX,
};
pub use sizing::{BlockMeasure, Edges, MeasureError, measure_block_width};
pub use tree::{BoxKind, BoxSnapshot, BoxTree, LayoutError, LayoutLimits};
