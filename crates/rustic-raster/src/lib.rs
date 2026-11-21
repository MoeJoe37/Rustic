//! M3 CPU reference backend: interprets a `rustic-paint` display list into a
//! checked premultiplied RGBA8 surface and encodes it as PNG.
//!
//! Output bytes are identical on every target: coverage and compositing use
//! only IEEE `f64` arithmetic with no platform math calls, and the PNG
//! encoder writes stored deflate blocks. No GPU or platform API is used.

#![warn(missing_docs)]

mod compare;
mod damage;
mod frames;
mod png;
mod raster;
mod scene;

pub use compare::{Difference, compare};
pub use damage::damage;
pub use frames::{FrameStats, Frames};
pub use png::{PngError, encode_png};
pub use raster::{
    DeviceRect, RasterError, RasterLimits, Surface, TILE_SIZE, rasterize, rasterize_reference,
    rasterize_region, repaint, surface_size,
};
pub use scene::{LayerKind, Scene, SceneFrame, SceneLayer, SceneStats};
