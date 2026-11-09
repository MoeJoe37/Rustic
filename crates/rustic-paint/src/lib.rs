//! M3 paint reference: immutable display lists in paint order, with colors
//! resolved to 8-bit sRGB. `rustic-raster` interprets them into pixels.
//!
//! This first slice paints what the block-only layout reference produces:
//! the canvas background (with root and HTML `body` propagation), block
//! background colors clipped to their `background-clip` box, and solid
//! borders. Text, images, gradients, non-solid border styles, opacity groups,
//! stacking contexts and clipping fail with typed errors instead of being
//! approximated. No platform graphics API, GPU or font is used.

#![warn(missing_docs)]

mod color;
mod display;

pub use color::{Srgba, srgba};
pub use display::{
    DisplayItem, DisplayList, ItemRole, PaintError, PaintLimits, Shape, UnsupportedPaint,
};
