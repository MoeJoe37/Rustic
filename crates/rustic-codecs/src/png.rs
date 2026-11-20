//! Bounded PNG decoding to straight-alpha RGBA8 (PNG Third Edition).
//!
//! All color types, bit depths 1–16, palettes with `tRNS`, the five filters
//! and Adam7 interlacing are supported. Chunk CRCs are verified and unknown
//! critical chunks are rejected. Color management chunks (`gAMA`, `cHRM`,
//! `iCCP`, `sRGB`, `cICP`) are ignored, so samples are treated as sRGB
//! (TD-51); animation (`acTL`) shows only the default image.

use crate::inflate::{InflateError, zlib_decompress};
use rustic_base::CancellationToken;
use std::{error::Error, fmt};

/// Decoding budgets, checked before allocation.
#[derive(Clone, Copy, Debug)]
pub struct PngLimits {
    /// Maximum image width in pixels.
    pub max_width: u32,
    /// Maximum image height in pixels.
    pub max_height: u32,
    /// Maximum decoded RGBA bytes (4 per pixel).
    pub max_bytes: usize,
    /// Maximum chunks read, including ancillary ones.
    pub max_chunks: usize,
}

impl Default for PngLimits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_bytes: 256 * 1024 * 1024,
            max_chunks: 100_000,
        }
    }
}

/// An atomic PNG decoding failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PngDecodeError {
    /// Not a PNG signature.
    Signature,
    /// The file ends early.
    Truncated,
    /// A chunk CRC does not match.
    Crc,
    /// The file violates the PNG format.
    Invalid,
    /// The image data stream is corrupt.
    Compression(InflateError),
    /// Dimensions, decoded bytes or chunk count exceed the limits.
    Limit,
    /// Cooperative cancellation.
    Cancelled,
}

impl fmt::Display for PngDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signature => f.write_str("not a PNG file"),
            Self::Truncated => f.write_str("PNG file ends early"),
            Self::Crc => f.write_str("PNG chunk checksum mismatch"),
            Self::Invalid => f.write_str("PNG file is invalid"),
            Self::Compression(error) => write!(f, "PNG image data: {error}"),
            Self::Limit => f.write_str("PNG image exceeds its decoding limits"),
            Self::Cancelled => f.write_str("PNG decoding cancelled"),
        }
    }
}

impl Error for PngDecodeError {}

/// A decoded image: straight-alpha sRGB RGBA8, rows top to bottom.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width × height × 4` bytes.
    pub pixels: Vec<u8>,
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

pub(crate) fn crc32(kind: &[u8], data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in kind.iter().chain(data) {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

#[derive(Clone, Copy)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
    interlaced: bool,
}

impl Header {
    fn channels(self) -> usize {
        match self.color {
            0 | 3 => 1,
            4 => 2,
            2 => 3,
            _ => 4,
        }
    }
    fn bits_per_pixel(self) -> usize {
        self.channels() * usize::from(self.depth)
    }
    /// Filter unit: bytes per complete pixel, at least one.
    fn filter_bpp(self) -> usize {
        self.bits_per_pixel().div_ceil(8)
    }
    fn row_bytes(self, width: usize) -> Option<usize> {
        width
            .checked_mul(self.bits_per_pixel())
            .map(|b| b.div_ceil(8))
    }
}

const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

fn passes(h: Header) -> Vec<(usize, usize, usize, usize, usize, usize)> {
    let (w, ht) = (h.width as usize, h.height as usize);
    if !h.interlaced {
        return vec![(0, 0, 1, 1, w, ht)];
    }
    ADAM7
        .iter()
        .map(|&(x0, y0, dx, dy)| {
            let pw = if w > x0 { (w - x0).div_ceil(dx) } else { 0 };
            let ph = if ht > y0 { (ht - y0).div_ceil(dy) } else { 0 };
            (x0, y0, dx, dy, pw, ph)
        })
        .collect()
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Undo one row's filter in place, given the previous unfiltered row.
fn unfilter(kind: u8, row: &mut [u8], previous: &[u8], bpp: usize) -> Result<(), PngDecodeError> {
    match kind {
        0 => {}
        1 => {
            for i in bpp..row.len() {
                row[i] = row[i].wrapping_add(row[i - bpp]);
            }
        }
        2 => {
            for (x, &up) in row.iter_mut().zip(previous) {
                *x = x.wrapping_add(up);
            }
        }
        3 => {
            for i in 0..row.len() {
                let left = if i >= bpp { u16::from(row[i - bpp]) } else { 0 };
                row[i] = row[i].wrapping_add(((left + u16::from(previous[i])) / 2) as u8);
            }
        }
        4 => {
            for i in 0..row.len() {
                let (left, corner) = if i >= bpp {
                    (row[i - bpp], previous[i - bpp])
                } else {
                    (0, 0)
                };
                row[i] = row[i].wrapping_add(paeth(left, previous[i], corner));
            }
        }
        _ => return Err(PngDecodeError::Invalid),
    }
    Ok(())
}

/// Sample `index` of an unfiltered row at the header's bit depth.
fn sample(row: &[u8], index: usize, depth: u8) -> u16 {
    match depth {
        16 => u16::from_be_bytes([row[index * 2], row[index * 2 + 1]]),
        8 => u16::from(row[index]),
        _ => {
            let bits = usize::from(depth);
            let bit = index * bits;
            let byte = row[bit / 8];
            let shift = 8 - bits - bit % 8;
            u16::from((byte >> shift) & ((1u8 << bits) - 1))
        }
    }
}

/// Scale a sample to 8 bits with exact rounding.
fn to8(value: u16, depth: u8) -> u8 {
    let max = (1u32 << depth) - 1;
    ((u32::from(value) * 255 + max / 2) / max) as u8
}

/// Decode a PNG file within `limits`.
pub fn decode_png(
    data: &[u8],
    limits: PngLimits,
    cancellation: &CancellationToken,
) -> Result<DecodedImage, PngDecodeError> {
    cancellation
        .check()
        .map_err(|_| PngDecodeError::Cancelled)?;
    if data.get(..8) != Some(b"\x89PNG\r\n\x1a\n".as_slice()) {
        return Err(
            if data.len() < 8 && b"\x89PNG\r\n\x1a\n".starts_with(data) {
                PngDecodeError::Truncated
            } else {
                PngDecodeError::Signature
            },
        );
    }
    let mut pos = 8;
    let mut header: Option<Header> = None;
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut transparent: Option<[u16; 3]> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut idat_state = 0u8; // 0 before, 1 inside, 2 after the IDAT run
    let mut chunks = 0usize;
    loop {
        cancellation
            .check()
            .map_err(|_| PngDecodeError::Cancelled)?;
        chunks += 1;
        if chunks > limits.max_chunks {
            return Err(PngDecodeError::Limit);
        }
        let head = data.get(pos..pos + 8).ok_or(PngDecodeError::Truncated)?;
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        if len > 0x7fff_ffff {
            return Err(PngDecodeError::Invalid);
        }
        let kind: [u8; 4] = [head[4], head[5], head[6], head[7]];
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(PngDecodeError::Invalid);
        }
        let start = pos + 8;
        let end = start
            .checked_add(len as usize)
            .ok_or(PngDecodeError::Truncated)?;
        let body = data.get(start..end).ok_or(PngDecodeError::Truncated)?;
        let crc = data.get(end..end + 4).ok_or(PngDecodeError::Truncated)?;
        if crc32(&kind, body) != u32::from_be_bytes([crc[0], crc[1], crc[2], crc[3]]) {
            return Err(PngDecodeError::Crc);
        }
        pos = end + 4;
        if header.is_none() && &kind != b"IHDR" {
            return Err(PngDecodeError::Invalid);
        }
        if idat_state == 1 && &kind != b"IDAT" {
            idat_state = 2;
        }
        match &kind {
            b"IHDR" => {
                if header.is_some() || body.len() != 13 {
                    return Err(PngDecodeError::Invalid);
                }
                let width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                let height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                let (depth, color) = (body[8], body[9]);
                let valid_depth = match color {
                    0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                    3 => matches!(depth, 1 | 2 | 4 | 8),
                    2 | 4 | 6 => matches!(depth, 8 | 16),
                    _ => false,
                };
                if width == 0
                    || height == 0
                    || width > 0x7fff_ffff
                    || height > 0x7fff_ffff
                    || !valid_depth
                    || body[10] != 0
                    || body[11] != 0
                    || body[12] > 1
                {
                    return Err(PngDecodeError::Invalid);
                }
                if width > limits.max_width
                    || height > limits.max_height
                    || (width as usize)
                        .checked_mul(height as usize)
                        .and_then(|n| n.checked_mul(4))
                        .is_none_or(|n| n > limits.max_bytes)
                {
                    return Err(PngDecodeError::Limit);
                }
                header = Some(Header {
                    width,
                    height,
                    depth,
                    color,
                    interlaced: body[12] == 1,
                });
            }
            b"PLTE" => {
                let h = header.ok_or(PngDecodeError::Invalid)?;
                if idat_state != 0
                    || !palette.is_empty()
                    || body.is_empty()
                    || body.len() % 3 != 0
                    || body.len() / 3 > 256
                    || matches!(h.color, 0 | 4)
                    || (h.color == 3 && body.len() / 3 > 1 << h.depth)
                {
                    return Err(PngDecodeError::Invalid);
                }
                palette = body
                    .chunks_exact(3)
                    .map(|c| [c[0], c[1], c[2], 255])
                    .collect();
            }
            b"tRNS" => {
                let h = header.ok_or(PngDecodeError::Invalid)?;
                if idat_state != 0 || transparent.is_some() {
                    return Err(PngDecodeError::Invalid);
                }
                let word = |i: usize| u16::from_be_bytes([body[i], body[i + 1]]);
                match h.color {
                    0 if body.len() == 2 => transparent = Some([word(0); 3]),
                    2 if body.len() == 6 => transparent = Some([word(0), word(2), word(4)]),
                    3 if !palette.is_empty() && body.len() <= palette.len() => {
                        for (entry, &alpha) in palette.iter_mut().zip(body) {
                            entry[3] = alpha;
                        }
                        // Marks tRNS as seen for palette images.
                        transparent = Some([0; 3]);
                    }
                    _ => return Err(PngDecodeError::Invalid),
                }
            }
            b"IDAT" => {
                if idat_state == 2 || (header.is_some_and(|h| h.color == 3) && palette.is_empty()) {
                    return Err(PngDecodeError::Invalid);
                }
                idat_state = 1;
                idat.extend_from_slice(body);
            }
            b"IEND" => {
                if idat_state == 0 || !body.is_empty() {
                    return Err(PngDecodeError::Invalid);
                }
                break;
            }
            _ if kind[0].is_ascii_uppercase() => return Err(PngDecodeError::Invalid),
            _ => {}
        }
    }
    let h = header.ok_or(PngDecodeError::Invalid)?;
    let layout = passes(h);
    let mut expected = 0usize;
    for &(.., pw, ph) in &layout {
        if pw == 0 || ph == 0 {
            continue;
        }
        let row = h.row_bytes(pw).ok_or(PngDecodeError::Limit)? + 1;
        expected = row
            .checked_mul(ph)
            .and_then(|n| n.checked_add(expected))
            .ok_or(PngDecodeError::Limit)?;
    }
    let raw = zlib_decompress(&idat, expected, cancellation).map_err(|e| match e {
        InflateError::OutputLimit => PngDecodeError::Invalid,
        InflateError::Cancelled => PngDecodeError::Cancelled,
        other => PngDecodeError::Compression(other),
    })?;
    if raw.len() != expected {
        return Err(PngDecodeError::Invalid);
    }
    let (width, height) = (h.width as usize, h.height as usize);
    let mut pixels = vec![0u8; width * height * 4];
    let bpp = h.filter_bpp();
    let channels = h.channels();
    let mut offset = 0;
    for &(x0, y0, dx, dy, pw, ph) in &layout {
        if pw == 0 || ph == 0 {
            continue;
        }
        let row_len = h.row_bytes(pw).ok_or(PngDecodeError::Limit)?;
        let mut previous = vec![0u8; row_len];
        let mut current = vec![0u8; row_len];
        for py in 0..ph {
            cancellation
                .check()
                .map_err(|_| PngDecodeError::Cancelled)?;
            let filter = raw[offset];
            current.copy_from_slice(&raw[offset + 1..offset + 1 + row_len]);
            offset += row_len + 1;
            unfilter(filter, &mut current, &previous, bpp)?;
            let y = y0 + py * dy;
            for px in 0..pw {
                let x = x0 + px * dx;
                let s = |c: usize| sample(&current, px * channels + c, h.depth);
                let rgba = match h.color {
                    0 => {
                        let g = s(0);
                        let a = if transparent.is_some_and(|t| t[0] == g) {
                            0
                        } else {
                            255
                        };
                        let v = to8(g, h.depth);
                        [v, v, v, a]
                    }
                    2 => {
                        let (r, g, b) = (s(0), s(1), s(2));
                        let a = if transparent == Some([r, g, b]) {
                            0
                        } else {
                            255
                        };
                        [to8(r, h.depth), to8(g, h.depth), to8(b, h.depth), a]
                    }
                    3 => *palette
                        .get(usize::from(s(0)))
                        .ok_or(PngDecodeError::Invalid)?,
                    4 => {
                        let v = to8(s(0), h.depth);
                        [v, v, v, to8(s(1), h.depth)]
                    }
                    _ => [
                        to8(s(0), h.depth),
                        to8(s(1), h.depth),
                        to8(s(2), h.depth),
                        to8(s(3), h.depth),
                    ],
                };
                let i = (y * width + x) * 4;
                pixels[i..i + 4].copy_from_slice(&rgba);
            }
            std::mem::swap(&mut previous, &mut current);
        }
    }
    Ok(DecodedImage {
        width: h.width,
        height: h.height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paeth_and_scaling_follow_the_specification() {
        assert_eq!(paeth(10, 20, 15), 15);
        assert_eq!(paeth(10, 20, 30), 10);
        assert_eq!(paeth(30, 20, 10), 30);
        assert_eq!(to8(1, 1), 255);
        assert_eq!(to8(2, 2), 170);
        assert_eq!(to8(0x8080, 16), 128);
        assert_eq!(to8(0x807f, 16), 128);
        assert_eq!(to8(65_535, 16), 255);
        assert_eq!(sample(&[0b1011_0010], 1, 2), 0b11);
        assert_eq!(sample(&[0b1011_0010], 7, 1), 0);
    }

    #[test]
    fn adam7_passes_cover_every_pixel_once() {
        for (w, h) in [(1, 1), (3, 2), (8, 8), (9, 17), (33, 5)] {
            let header = Header {
                width: w,
                height: h,
                depth: 8,
                color: 6,
                interlaced: true,
            };
            let mut seen = vec![0u8; (w * h) as usize];
            for (x0, y0, dx, dy, pw, ph) in passes(header) {
                for py in 0..ph {
                    for px in 0..pw {
                        seen[(y0 + py * dy) * w as usize + x0 + px * dx] += 1;
                    }
                }
            }
            assert!(seen.iter().all(|&n| n == 1), "{w}x{h}");
        }
    }
}
