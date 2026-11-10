//! Initial M3 layout contracts: checked geometry, principal box construction
//! and non-replaced normal-flow block width measurement.
//!
//! A block-only reference emits checked used rectangles with margin collapse,
//! floats and clearance, relative/absolute/fixed positioning and overflow
//! clipping (no scrolling UI). It is not yet a document renderer: flow-box
//! normalization is separate from geometry; line boxes, text shaping, general
//! fragments and painting remain unimplemented.
//! The immutable box snapshot consumes M2 computed styles without recascading.

#![warn(missing_docs)]

mod exclusions;
mod flow;
mod formatting;
mod geometry;
mod height;
mod inline;
mod intrinsic;
mod lines;
mod margins;
mod overflow;
mod position;
mod replaced;
mod sizing;
mod text;
mod tree;

pub use flow::{
    BlockFragment, BlockLayout, FlowError, MAX_FLOW_WORK, PositionScheme, UnsupportedFlow,
};
pub use formatting::{
    FormattingBox, FormattingError, FormattingKind, FormattingLimits, FormattingTree, InlineEdges,
};
pub use geometry::{
    CssPoint, CssPx, CssRect, CssSize, DevicePx, DeviceScale, GeometryError, MAX_CSS_PX, View,
};
pub use overflow::{AxisRange, Clip};
pub use replaced::NaturalSize;
pub use sizing::{BlockMeasure, Edges, MeasureError, measure_block_width};
pub use text::{InlineFragment, LineFragment, PositionedGlyph, TextFragment};
pub use tree::{BoxKind, BoxSnapshot, BoxTree, LayoutError, LayoutLimits};
