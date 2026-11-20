//! Bounded decoders for untrusted compressed and image data.
//!
//! Every decoder checks declared sizes against caller limits before
//! allocating, bounds total output and work, checks cancellation, and fails
//! atomically with a typed error. No `unsafe`, platform codec or third-party
//! crate is used; formats are implemented from their specifications
//! (RFC 1950/1951/1952, PNG Third Edition, ITU-T T.81 JPEG, GIF89a and the WebP
//! container, lossless and VP8 (RFC 6386, RFC 9649) bitstreams).

#![warn(missing_docs)]

mod gif;
mod gzip;
mod image;
mod inflate;
mod jpeg;
mod png;
mod store;
mod vp8;
mod vp8_tables;
mod vp8l;
mod webp;

pub use gif::decode_gif;
pub use gzip::gzip_decompress;
pub use image::{
    Animation, DecodeLimits, Frame, ImageError, ImageFormat, decode_animation, decode_image, sniff,
};
pub use inflate::{InflateError, inflate, zlib_decompress};
pub use jpeg::decode_jpeg;
pub use png::{DecodedImage, PngDecodeError, PngLimits, decode_png};
pub use store::{
    ImageId, ImageInfo, ImageState, ImageStore, StoreError, StoreLimits, StoreStats, probe,
};
pub use webp::decode_webp;
