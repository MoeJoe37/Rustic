//! M3 CPU reference backend: interprets a `rustic-paint` display list into a
//! checked premultiplied RGBA8 surface and encodes it as PNG.
//!
//! Output bytes are identical on every target: coverage and compositing use
//! only IEEE `f64` arithmetic with no platform math calls, and the PNG
//! encoder writes stored deflate blocks. No GPU or platform API is used.

#![warn(missing_docs)]

mod compare;
mod png;
mod raster;

pub use compare::{Difference, compare};
pub use png::{PngError, encode_png};
pub use raster::{RasterError, RasterLimits, Surface, rasterize};
