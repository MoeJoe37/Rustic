//! Bounded decoders for untrusted compressed and image data.
//!
//! Every decoder checks declared sizes against caller limits before
//! allocating, bounds total output and work, checks cancellation, and fails
//! atomically with a typed error. No `unsafe`, platform codec or third-party
//! crate is used; formats are implemented from their specifications
//! (RFC 1950/1951 and PNG, ISO/IEC 15948 / W3C PNG Third Edition).

#![warn(missing_docs)]

mod inflate;
mod png;

pub use inflate::{InflateError, inflate, zlib_decompress};
pub use png::{DecodedImage, PngDecodeError, PngLimits, decode_png};
