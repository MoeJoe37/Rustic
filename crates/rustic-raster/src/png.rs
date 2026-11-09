//! Minimal deterministic PNG encoder (RGBA8, no interlace, filter 0).
//!
//! Image data is a zlib stream of stored (uncompressed) deflate blocks, so
//! files are large but byte-identical everywhere and need no compressor.
//! Compression is later work (TD-41).

use crate::raster::{Surface, unpremultiply};
use std::{error::Error, fmt};

/// PNG output would exceed `max_bytes` or a format field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PngError;

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PNG output exceeds its size limit")
    }
}

impl Error for PngError {}

const fn crc_table() -> [u32; 256] {
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
}

const CRC_TABLE: [u32; 256] = crc_table();

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for part in parts {
        for &b in *part {
            c = CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xffff_ffff
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<(), PngError> {
    let len = u32::try_from(data.len()).map_err(|_| PngError)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    Ok(())
}

/// Encode a surface as straight-alpha RGBA8 PNG, failing before allocation
/// if the file would exceed `max_bytes`.
pub fn encode_png(surface: &Surface, max_bytes: usize) -> Result<Vec<u8>, PngError> {
    let width = surface.width() as usize;
    let height = surface.height() as usize;
    let row = width
        .checked_mul(4)
        .and_then(|n| n.checked_add(1))
        .ok_or(PngError)?;
    let raw = row.checked_mul(height).ok_or(PngError)?;
    let blocks = raw.div_ceil(65_535).max(1);
    // zlib header + per-block headers + data + Adler-32.
    let zlib = blocks
        .checked_mul(5)
        .and_then(|n| n.checked_add(raw))
        .and_then(|n| n.checked_add(6))
        .ok_or(PngError)?;
    if u32::try_from(zlib).is_err() {
        return Err(PngError);
    }
    // Signature, IHDR (25), IDAT framing (12), IEND (12).
    let total = zlib.checked_add(8 + 25 + 12 + 12).ok_or(PngError)?;
    if total > max_bytes {
        return Err(PngError);
    }
    let mut data = Vec::with_capacity(raw);
    let pixels = surface.premultiplied();
    for y in 0..height {
        data.push(0);
        for p in pixels[y * width * 4..(y + 1) * width * 4].chunks_exact(4) {
            let c = unpremultiply([p[0], p[1], p[2], p[3]]);
            data.extend_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    }
    let mut z = Vec::with_capacity(zlib);
    // CMF: deflate, 32 KiB window; FLG: fastest, check bits make it % 31 == 0.
    z.extend_from_slice(&[0x78, 0x01]);
    let mut chunks = data.chunks(65_535).peekable();
    if data.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(block) = chunks.next() {
        let last = chunks.peek().is_none();
        let len = block.len() as u16;
        z.push(u8::from(last));
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&surface.width().to_be_bytes());
    ihdr.extend_from_slice(&surface.height().to_be_bytes());
    // 8-bit depth, color type 6 (RGBA), deflate, filter 0, no interlace.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr)?;
    chunk(&mut out, b"IDAT", &z)?;
    chunk(&mut out, b"IEND", &[])?;
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::RasterLimits;

    #[test]
    fn crc_and_adler_match_published_check_values() {
        assert_eq!(crc32(&[b"123456789"]), 0xcbf4_3926);
        assert_eq!(crc32(&[b"IEND"]), 0xae42_6082);
    }

    #[test]
    fn one_pixel_png_has_the_exact_expected_bytes() {
        let surface = Surface::new(1, 1, RasterLimits::default()).unwrap();
        let png = encode_png(&surface, usize::MAX).unwrap();
        // Signature, IHDR, then IDAT holding one filter byte and one pixel.
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        let idat = &png[33..];
        assert_eq!(&idat[..8], &[0, 0, 0, 16, b'I', b'D', b'A', b'T']);
        assert_eq!(
            &idat[8..24],
            &[
                0x78, 0x01, 1, 5, 0, 0xfa, 0xff, 0, 0, 0, 0, 0, 0, 0x05, 0, 0x01
            ]
        );
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]));
        assert_eq!(encode_png(&surface, png.len() - 1), Err(PngError));
        assert_eq!(encode_png(&surface, png.len()).unwrap(), png);
    }

    #[test]
    fn large_images_split_into_maximal_stored_blocks() {
        // 128x128 RGBA = 65,664 filtered bytes: two stored blocks.
        let surface = Surface::new(128, 128, RasterLimits::default()).unwrap();
        let png = encode_png(&surface, usize::MAX).unwrap();
        let idat = &png[33 + 8..];
        assert_eq!(&idat[2..7], &[0, 0xff, 0xff, 0, 0]);
        let second = 2 + 5 + 65_535;
        assert_eq!(&idat[second..second + 5], &[1, 129, 0, 126, 255]);
    }
}
