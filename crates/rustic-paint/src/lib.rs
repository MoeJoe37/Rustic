//! M3 paint reference: immutable display lists in paint order, with colors
//! resolved to 8-bit sRGB. `rustic-raster` interprets them into pixels.
//!
//! It paints what the block layout reference produces, in CSS 2.2 Appendix E
//! order with stacking contexts and `z-index`: the canvas background (with
//! root and HTML `body` propagation), block background colors clipped to
//! their `background-clip` box and solid borders, each with its overflow
//! clip, and opacity groups. Text, images, gradients and non-solid border
//! styles fail with typed errors instead of being approximated. No platform graphics API, GPU or font is used.

#![warn(missing_docs)]

mod color;
mod display;

pub use color::{Srgba, srgba};
pub use display::{
    DisplayCommand, DisplayItem, DisplayList, ItemRole, PaintError, PaintLimits, Shape,
    UnsupportedPaint,
};
