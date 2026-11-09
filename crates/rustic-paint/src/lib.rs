//! M3 paint reference: immutable display lists in paint order, with colors
//! resolved to 8-bit sRGB. `rustic-raster` interprets them into pixels.
//!
//! It paints what the block layout reference produces, in CSS 2.2 Appendix E
//! order with stacking contexts and `z-index`: the canvas background (with
//! root and HTML `body` propagation), block background colors clipped to
//! their `background-clip` box and solid borders, each with its overflow
//! clip and rounded clips, outlines, text, images and opacity groups.
//! Gradients and non-solid border styles fail with typed errors instead of
//! being approximated. No platform graphics API, GPU or system font is used.

#![warn(missing_docs)]

mod clip;
mod color;
mod display;
mod matrix;
mod rounded;

pub use color::{Srgba, srgba};
pub use display::{
    BoxShadow, ClipNode, DisplayCommand, DisplayItem, DisplayList, FilterOp, Fonts, Hit, Image,
    ItemRole, PaintError, PaintLimits, Path, PathPoint, Resources, Shape, TransformNode,
    UnsupportedPaint,
};
pub use matrix::Homography;
pub use rounded::{Corner, RoundedRect};
/// Blend modes of [`DisplayCommand::BeginGroup`], from `rustic-css`.
pub use rustic_css::BlendMode;
