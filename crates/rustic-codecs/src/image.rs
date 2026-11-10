//! Format sniffing, shared limits and errors, and format-independent
//! decoding entry points for the JPEG, GIF and WebP decoders.

use crate::png::{DecodedImage, PngDecodeError, PngLimits, decode_png};
use rustic_base::CancellationToken;
use std::{error::Error, fmt};

/// Budgets shared by the JPEG, GIF and WebP decoders, checked before any
/// allocation that depends on untrusted sizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeLimits {
    /// Maximum canvas width in pixels.
    pub max_width: u32,
    /// Maximum canvas height in pixels.
    pub max_height: u32,
    /// Maximum bytes of decoded output and large intermediate buffers.
    /// For animations this bounds the sum over all frames.
    pub max_bytes: usize,
    /// Maximum frames decoded from an animation; later frames are ignored.
    pub max_frames: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_bytes: 256 * 1024 * 1024,
            max_frames: 1024,
        }
    }
}

impl DecodeLimits {
    /// Check a canvas size against the limits and return its RGBA byte
    /// count.
    pub(crate) fn check(&self, width: u32, height: u32) -> Result<usize, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::Invalid);
        }
        if width > self.max_width || height > self.max_height {
            return Err(ImageError::Limit);
        }
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(ImageError::Limit)?;
        if bytes > self.max_bytes {
            return Err(ImageError::Limit);
        }
        Ok(bytes)
    }
}

/// An atomic JPEG, GIF or WebP decoding failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    /// The data does not start with the format's signature.
    Signature,
    /// The data ends early.
    Truncated,
    /// The data violates the format.
    Invalid,
    /// A valid feature this decoder does not implement (for example
    /// arithmetic-coded or lossless JPEG).
    Unsupported,
    /// Dimensions, decoded bytes or work exceed the limits.
    Limit,
    /// Cooperative cancellation.
    Cancelled,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Signature => "unrecognized image signature",
            Self::Truncated => "image data ends early",
            Self::Invalid => "image data is invalid",
            Self::Unsupported => "image uses an unsupported feature",
            Self::Limit => "image exceeds its decoding limits",
            Self::Cancelled => "image decoding cancelled",
        })
    }
}

impl Error for ImageError {}

impl From<PngDecodeError> for ImageError {
    fn from(error: PngDecodeError) -> Self {
        match error {
            PngDecodeError::Signature => Self::Signature,
            PngDecodeError::Truncated => Self::Truncated,
            PngDecodeError::Limit => Self::Limit,
            PngDecodeError::Cancelled => Self::Cancelled,
            PngDecodeError::Crc | PngDecodeError::Invalid | PngDecodeError::Compression(_) => {
                Self::Invalid
            }
        }
    }
}

pub(crate) fn cancelled(token: &CancellationToken) -> Result<(), ImageError> {
    token.check().map_err(|_| ImageError::Cancelled)
}

/// A raster image format recognised by its signature.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ImageFormat {
    /// PNG (`image/png`).
    Png,
    /// JPEG (`image/jpeg`).
    Jpeg,
    /// GIF (`image/gif`).
    Gif,
    /// WebP (`image/webp`).
    WebP,
}

impl ImageFormat {
    /// The format's MIME essence.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
        }
    }
}

/// Identify an image format from its leading bytes, following the image
/// type pattern matching algorithm of the WHATWG MIME Sniffing Standard
/// (restricted to the formats Rustic decodes).
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 14 && &bytes[..4] == b"RIFF" && &bytes[8..14] == b"WEBPVP" {
        Some(ImageFormat::WebP)
    } else {
        None
    }
}

/// One fully composited frame of an animation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    /// The whole canvas after this frame is drawn.
    pub image: DecodedImage,
    /// How long the frame stays on screen, in milliseconds.
    pub delay_ms: u32,
}

/// A decoded, possibly animated, image. Still images have one frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Animation {
    /// Composited frames in display order (at least one).
    pub frames: Vec<Frame>,
    /// Times to play the animation; `0` means forever.
    pub loop_count: u32,
}

impl Animation {
    pub(crate) fn still(image: DecodedImage) -> Self {
        Self {
            frames: vec![Frame { image, delay_ms: 0 }],
            loop_count: 0,
        }
    }

    /// Total decoded bytes held by all frames.
    pub fn byte_len(&self) -> usize {
        self.frames.iter().map(|f| f.image.pixels.len()).sum()
    }
}

/// Decode the first (or only) frame of a PNG, JPEG, GIF or WebP image to
/// straight-alpha RGBA8. The format is chosen by [`sniff`].
pub fn decode_image(
    bytes: &[u8],
    limits: DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<DecodedImage, ImageError> {
    match sniff(bytes).ok_or(ImageError::Signature)? {
        ImageFormat::Png => Ok(decode_png(bytes, png_limits(limits), cancellation)?),
        ImageFormat::Jpeg => crate::jpeg::decode_jpeg(bytes, limits, cancellation),
        ImageFormat::Gif => {
            let limits = DecodeLimits {
                max_frames: 1,
                ..limits
            };
            first(crate::gif::decode_gif(bytes, limits, cancellation)?)
        }
        ImageFormat::WebP => {
            let limits = DecodeLimits {
                max_frames: 1,
                ..limits
            };
            first(crate::webp::decode_webp(bytes, limits, cancellation)?)
        }
    }
}

/// Decode every frame of an image (up to `limits.max_frames`); still
/// formats yield one frame.
pub fn decode_animation(
    bytes: &[u8],
    limits: DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<Animation, ImageError> {
    match sniff(bytes).ok_or(ImageError::Signature)? {
        ImageFormat::Gif => crate::gif::decode_gif(bytes, limits, cancellation),
        ImageFormat::WebP => crate::webp::decode_webp(bytes, limits, cancellation),
        _ => decode_image(bytes, limits, cancellation).map(Animation::still),
    }
}

fn png_limits(limits: DecodeLimits) -> PngLimits {
    PngLimits {
        max_width: limits.max_width,
        max_height: limits.max_height,
        max_bytes: limits.max_bytes,
        ..PngLimits::default()
    }
}

fn first(animation: Animation) -> Result<DecodedImage, ImageError> {
    animation
        .frames
        .into_iter()
        .next()
        .map(|f| f.image)
        .ok_or(ImageError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffing_follows_the_mime_sniffing_patterns() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some(ImageFormat::Png));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0"), Some(ImageFormat::Jpeg));
        assert_eq!(sniff(b"GIF89a"), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"GIF87a"), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::WebP));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b"GIF88a"), None);
        assert_eq!(sniff(b""), None);
        assert_eq!(ImageFormat::WebP.mime(), "image/webp");
    }

    #[test]
    fn limits_reject_empty_huge_and_overflowing_canvases() {
        let limits = DecodeLimits {
            max_width: 100,
            max_height: 100,
            max_bytes: 400,
            max_frames: 1,
        };
        assert_eq!(limits.check(10, 10), Ok(400));
        assert_eq!(limits.check(0, 10), Err(ImageError::Invalid));
        assert_eq!(limits.check(101, 1), Err(ImageError::Limit));
        assert_eq!(limits.check(11, 10), Err(ImageError::Limit));
        let wide = DecodeLimits {
            max_width: u32::MAX,
            max_height: u32::MAX,
            max_bytes: usize::MAX,
            max_frames: 1,
        };
        assert_eq!(wide.check(u32::MAX, u32::MAX), Err(ImageError::Limit));
    }
}
