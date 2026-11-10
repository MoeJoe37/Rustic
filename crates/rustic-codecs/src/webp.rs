//! Bounded WebP decoding: the RIFF container (RFC 9649 section 2), simple
//! lossy (`VP8 `) and lossless (`VP8L`) images, the extended format with
//! `ALPH` alpha (raw or lossless, all four filters) and animation (`ANIM`
//! and `ANMF` with blending and disposal).
//!
//! Animation frames are composited onto a transparent canvas, as browsers
//! do, ignoring the `ANIM` background color. ICC, EXIF and XMP chunks are
//! skipped (TD-81).

use crate::image::{Animation, DecodeLimits, Frame, ImageError, cancelled};
use crate::png::DecodedImage;
use crate::{vp8, vp8l};
use rustic_base::CancellationToken;

fn le24(b: &[u8]) -> usize {
    usize::from(b[0]) | usize::from(b[1]) << 8 | usize::from(b[2]) << 16
}

/// Iterate `(fourcc, payload)` chunks, honouring odd-size padding.
fn chunks(mut data: &[u8]) -> impl Iterator<Item = Result<(&[u8], &[u8]), ImageError>> {
    std::iter::from_fn(move || {
        if data.is_empty() {
            return None;
        }
        if data.len() < 8 {
            data = &[];
            return Some(Err(ImageError::Truncated));
        }
        let fourcc = &data[..4];
        let size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let Some(payload) = data.get(8..8usize.saturating_add(size)) else {
            data = &[];
            return Some(Err(ImageError::Truncated));
        };
        let next = (8 + size + (size & 1)).min(data.len());
        data = &data[next..];
        Some(Ok((fourcc, payload)))
    })
}

/// A decoded still bitstream (one frame's pixels) as straight RGBA.
fn bitstream(
    image: &[u8],
    alpha: Option<&[u8]>,
    budget: usize,
    cancellation: &CancellationToken,
) -> Result<(usize, usize, Vec<u8>), ImageError> {
    if let Some(vp8l_data) = image.strip_prefix(b"VP8L") {
        let payload = chunk_payload(vp8l_data)?;
        let (w, h, argb) = vp8l::decode(payload, budget, cancellation)?;
        let mut rgba = Vec::with_capacity(w * h * 4);
        for p in argb {
            rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
        }
        return Ok((w, h, rgba));
    }
    let payload = chunk_payload(image.strip_prefix(b"VP8 ").ok_or(ImageError::Invalid)?)?;
    let yuv = vp8::decode(payload, budget, cancellation)?;
    let (w, h) = (yuv.width, yuv.height);
    let mut rgba = yuv.to_rgba();
    if let Some(alpha) = alpha {
        let values = decode_alpha(alpha, w, h, budget, cancellation)?;
        for (i, a) in values.into_iter().enumerate() {
            rgba[i * 4 + 3] = a;
        }
    }
    Ok((w, h, rgba))
}

/// The payload of a chunk whose fourcc was already removed.
fn chunk_payload(rest: &[u8]) -> Result<&[u8], ImageError> {
    let size = rest.get(..4).ok_or(ImageError::Truncated)?;
    let size = u32::from_le_bytes([size[0], size[1], size[2], size[3]]) as usize;
    rest.get(4..4usize.saturating_add(size))
        .ok_or(ImageError::Truncated)
}

/// Decode an `ALPH` payload to one alpha byte per pixel.
fn decode_alpha(
    data: &[u8],
    width: usize,
    height: usize,
    budget: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, ImageError> {
    let (&header, rest) = data.split_first().ok_or(ImageError::Truncated)?;
    let compression = header & 3;
    let filter = (header >> 2) & 3;
    let mut alpha = match compression {
        0 => rest
            .get(..width * height)
            .ok_or(ImageError::Truncated)?
            .to_vec(),
        1 => vp8l::decode_headless(rest, width, height, budget, cancellation)?
            .into_iter()
            .map(|p| (p >> 8) as u8)
            .collect(),
        _ => return Err(ImageError::Invalid),
    };
    unfilter_alpha(&mut alpha, width, height, filter);
    Ok(alpha)
}

/// Undo the alpha prediction filter (none, horizontal, vertical, gradient).
fn unfilter_alpha(alpha: &mut [u8], width: usize, height: usize, filter: u8) {
    if filter == 0 {
        return;
    }
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let left = |a: &[u8]| a[i - 1];
            let above = |a: &[u8]| a[i - width];
            let pred = match (x, y) {
                (0, 0) => 0,
                (_, 0) => left(alpha),
                (0, _) => above(alpha),
                _ => match filter {
                    1 => left(alpha),
                    2 => above(alpha),
                    _ => {
                        let g = i32::from(left(alpha)) + i32::from(above(alpha))
                            - i32::from(alpha[i - width - 1]);
                        g.clamp(0, 255) as u8
                    }
                },
            };
            alpha[i] = alpha[i].wrapping_add(pred);
        }
    }
}

/// libwebp's non-premultiplied "source over" for animation frames.
fn blend(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let src_a = u32::from(src[3]);
    match src_a {
        0 => return dst,
        255 => return src,
        _ => {}
    }
    let dst_a = u32::from(dst[3]);
    let dst_factor = (dst_a * (256 - src_a)) >> 8;
    let blend_a = src_a + dst_factor;
    let scale = (1u32 << 24) / blend_a;
    let channel = |s: u8, d: u8| -> u8 {
        let unscaled = u32::from(s) * src_a + u32::from(d) * dst_factor;
        ((u64::from(unscaled) * u64::from(scale)) >> 24) as u8
    };
    [
        channel(src[0], dst[0]),
        channel(src[1], dst[1]),
        channel(src[2], dst[2]),
        blend_a as u8,
    ]
}

/// Decode a WebP image. Still images yield one frame.
pub fn decode_webp(
    bytes: &[u8],
    limits: DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<Animation, ImageError> {
    cancelled(cancellation)?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(ImageError::Signature);
    }
    let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let body = bytes
        .get(12..8usize.saturating_add(riff))
        .ok_or(ImageError::Truncated)?;
    let first = chunks(body).next().ok_or(ImageError::Truncated)??;
    match first.0 {
        b"VP8 " | b"VP8L" => {
            let (w, h, pixels) = bitstream(body, None, limits.max_bytes, cancellation)?;
            limits.check(w as u32, h as u32)?;
            Ok(Animation::still(DecodedImage {
                width: w as u32,
                height: h as u32,
                pixels,
            }))
        }
        b"VP8X" => extended(first.1, body, limits, cancellation),
        _ => Err(ImageError::Invalid),
    }
}

fn extended(
    vp8x: &[u8],
    body: &[u8],
    limits: DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<Animation, ImageError> {
    if vp8x.len() < 10 {
        return Err(ImageError::Invalid);
    }
    let animated = vp8x[0] & 0x02 != 0;
    let width = le24(&vp8x[4..7]) + 1;
    let height = le24(&vp8x[7..10]) + 1;
    let canvas_bytes = limits.check(width as u32, height as u32)?;
    let mut alpha: Option<&[u8]> = None;
    let mut loop_count = 0;
    let mut canvas = vec![0u8; canvas_bytes];
    let mut frames = Vec::new();
    let mut used = 0usize;
    // Bytes of the body after each chunk, to hand bitstream chunks over
    // with their headers.
    let mut offset = 0;
    for chunk in chunks(body) {
        let (fourcc, payload) = chunk?;
        let start = offset;
        offset += 8 + payload.len() + (payload.len() & 1);
        match fourcc {
            b"ALPH" if !animated => alpha = Some(payload),
            b"VP8 " | b"VP8L" if !animated => {
                let budget = limits.max_bytes.saturating_sub(canvas_bytes);
                let (w, h, pixels) = bitstream(&body[start..], alpha, budget, cancellation)?;
                if (w, h) != (width, height) {
                    return Err(ImageError::Invalid);
                }
                return Ok(Animation::still(DecodedImage {
                    width: width as u32,
                    height: height as u32,
                    pixels,
                }));
            }
            b"ANIM" if animated => {
                if payload.len() < 6 {
                    return Err(ImageError::Invalid);
                }
                loop_count = u32::from(u16::from_le_bytes([payload[4], payload[5]]));
            }
            b"ANMF" if animated => {
                cancelled(cancellation)?;
                if frames.len() >= limits.max_frames {
                    break;
                }
                let needed = used.checked_add(canvas_bytes).ok_or(ImageError::Limit)?;
                if needed > limits.max_bytes {
                    if frames.is_empty() {
                        return Err(ImageError::Limit);
                    }
                    break;
                }
                used = needed;
                let frame = anmf(payload, &mut canvas, (width, height), &limits, cancellation)?;
                frames.push(frame);
            }
            _ => {}
        }
    }
    if frames.is_empty() {
        return Err(if animated {
            ImageError::Invalid
        } else {
            ImageError::Truncated
        });
    }
    Ok(Animation { frames, loop_count })
}

fn anmf(
    payload: &[u8],
    canvas: &mut [u8],
    (width, height): (usize, usize),
    limits: &DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<Frame, ImageError> {
    if payload.len() < 16 {
        return Err(ImageError::Invalid);
    }
    let x0 = le24(&payload[0..3]) * 2;
    let y0 = le24(&payload[3..6]) * 2;
    let fw = le24(&payload[6..9]) + 1;
    let fh = le24(&payload[9..12]) + 1;
    let delay_ms = le24(&payload[12..15]) as u32;
    let flags = payload[15];
    let (no_blend, dispose) = (flags & 0x02 != 0, flags & 0x01 != 0);
    if x0 + fw > width || y0 + fh > height {
        return Err(ImageError::Invalid);
    }
    let data = &payload[16..];
    let mut alpha = None;
    let mut decoded = None;
    let mut offset = 0;
    for chunk in chunks(data) {
        let (fourcc, inner) = chunk?;
        let start = offset;
        offset += 8 + inner.len() + (inner.len() & 1);
        match fourcc {
            b"ALPH" => alpha = Some(inner),
            b"VP8 " | b"VP8L" => {
                let budget = limits.max_bytes.saturating_sub(width * height * 4);
                decoded = Some(bitstream(&data[start..], alpha, budget, cancellation)?);
                break;
            }
            _ => {}
        }
    }
    let (w, h, pixels) = decoded.ok_or(ImageError::Invalid)?;
    if (w, h) != (fw, fh) {
        return Err(ImageError::Invalid);
    }
    for y in 0..fh {
        for x in 0..fw {
            let s = (y * fw + x) * 4;
            let d = ((y0 + y) * width + x0 + x) * 4;
            let src = [pixels[s], pixels[s + 1], pixels[s + 2], pixels[s + 3]];
            let out = if no_blend {
                src
            } else {
                blend(
                    src,
                    [canvas[d], canvas[d + 1], canvas[d + 2], canvas[d + 3]],
                )
            };
            canvas[d..d + 4].copy_from_slice(&out);
        }
    }
    let shown = canvas.to_vec();
    if dispose {
        for y in y0..y0 + fh {
            canvas[(y * width + x0) * 4..(y * width + x0 + fw) * 4].fill(0);
        }
    }
    Ok(Frame {
        image: DecodedImage {
            width: width as u32,
            height: height as u32,
            pixels: shown,
        },
        delay_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_filters_invert_their_predictions() {
        // Gradient over a 3×2 block: every residual zero after the first
        // row reproduces clamp(left + above − above-left).
        let mut a = vec![10, 5, 5, 3, 0, 0];
        unfilter_alpha(&mut a, 3, 2, 3);
        assert_eq!(a, vec![10, 15, 20, 13, 18, 23]);
        let mut h = vec![1, 1, 1, 1, 1, 1];
        unfilter_alpha(&mut h, 3, 2, 1);
        assert_eq!(h, vec![1, 2, 3, 2, 3, 4]);
        let mut v = vec![1, 1, 1, 1, 1, 1];
        unfilter_alpha(&mut v, 3, 2, 2);
        assert_eq!(v, vec![1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn blending_is_source_over_in_straight_alpha() {
        assert_eq!(blend([9, 9, 9, 0], [1, 2, 3, 4]), [1, 2, 3, 4]);
        assert_eq!(blend([9, 8, 7, 255], [1, 2, 3, 4]), [9, 8, 7, 255]);
        let [r, _, _, a] = blend([255, 0, 0, 128], [0, 0, 255, 255]);
        assert_eq!(a, 255);
        assert!((127..=129).contains(&r), "{r}");
    }
}
