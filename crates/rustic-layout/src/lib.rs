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

mod anchor;
mod exclusions;
mod flow;
mod formatting;
mod fragments;
mod geometry;
mod height;
mod inline;
mod intrinsic;
mod lines;
mod margins;
mod overflow;
mod position;
mod replaced;
mod scroll;
mod sizing;
mod sticky;
mod tailoring;
mod text;
mod tree;

pub use flow::{
    BlockFragment, BlockLayout, FlowError, MAX_FLOW_WORK, PositionScheme, UnsupportedFlow,
};
pub use formatting::{
    FormattingBox, FormattingError, FormattingKind, FormattingLimits, FormattingTree, InlineEdges,
};
pub use fragments::{FragmentKind, FragmentNode, FragmentTree};
pub use geometry::{
    CssPoint, CssPx, CssRect, CssSize, DevicePx, DeviceScale, GeometryError, MAX_CSS_PX, View,
};
pub use overflow::{AxisRange, Clip};
pub use replaced::NaturalSize;
pub use scroll::{ScrollPositions, ScrollRange};
pub use sizing::{BlockMeasure, Edges, MeasureError, measure_block_width};
pub use sticky::StickyConstraint;
pub use text::{InlineFragment, LineFragment, PositionedGlyph, TextFragment};
pub use tree::{BoxKind, BoxSnapshot, BoxTree, LayoutError, LayoutLimits};
