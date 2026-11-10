//! Bounded GIF decoding (GIF87a and GIF89a) to composited RGBA8 frames.
//!
//! Supports global and local color tables, LZW image data, interlacing,
//! Graphic Control Extensions (delay, transparency and the three disposal
//! methods) and the NETSCAPE2.0 loop count. The canvas starts transparent,
//! as in browsers, rather than filled with the background color. A file
//! that stops after at least one complete frame (no trailer, or a cut in a
//! later frame) yields the complete frames; image data that ends before
//! every pixel is set leaves the rest of that frame unchanged. Pixels whose
//! index lies outside the color table are opaque black.

use crate::image::{Animation, DecodeLimits, Frame, ImageError, cancelled};
use crate::png::DecodedImage;
use rustic_base::CancellationToken;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, ImageError> {
        let b = *self.data.get(self.pos).ok_or(ImageError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16, ImageError> {
        Ok(u16::from(self.u8()?) | u16::from(self.u8()?) << 8)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ImageError> {
        let end = self.pos.checked_add(n).ok_or(ImageError::Truncated)?;
        let slice = self.data.get(self.pos..end).ok_or(ImageError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }
    /// Read a chain of data sub-blocks, calling `f` with each one.
    fn sub_blocks(
        &mut self,
        mut f: impl FnMut(&'a [u8]) -> Result<(), ImageError>,
    ) -> Result<(), ImageError> {
        loop {
            let len = self.u8()?;
            if len == 0 {
                return Ok(());
            }
            let block = self.bytes(usize::from(len))?;
            f(block)?;
        }
    }
    fn palette(&mut self, packed: u8) -> Result<&'a [u8], ImageError> {
        self.bytes(3 << ((packed & 7) + 1))
    }
}

#[derive(Clone, Copy, Default)]
struct Control {
    disposal: u8,
    transparent: Option<u8>,
    delay_cs: u16,
}

/// LZW decoder state for one image's data stream (GIF89a appendix F).
struct Lzw {
    min_size: u8,
    prefix: Vec<u16>,
    suffix: Vec<u8>,
    first: Vec<u8>,
    stack: Vec<u8>,
    next: u16,
    width: u8,
    previous: Option<u16>,
    bits: u32,
    nbits: u8,
    done: bool,
}

const NO_CODE: u16 = u16::MAX;

impl Lzw {
    fn new(min_size: u8) -> Self {
        let mut lzw = Self {
            min_size,
            prefix: vec![NO_CODE; 4096],
            suffix: vec![0; 4096],
            first: vec![0; 4096],
            stack: Vec::with_capacity(4096),
            next: 0,
            width: 0,
            previous: None,
            bits: 0,
            nbits: 0,
            done: false,
        };
        for code in 0..(1u16 << min_size) {
            lzw.suffix[usize::from(code)] = code as u8;
            lzw.first[usize::from(code)] = code as u8;
        }
        lzw.reset();
        lzw
    }

    fn reset(&mut self) {
        self.next = (1 << self.min_size) + 2;
        self.width = self.min_size + 1;
        self.previous = None;
    }

    /// Feed one sub-block, emitting color indices through `out`, which
    /// returns `false` once the frame is full.
    fn feed(&mut self, block: &[u8], out: &mut impl FnMut(u8) -> bool) -> Result<(), ImageError> {
        let clear = 1u16 << self.min_size;
        for &byte in block {
            if self.done {
                return Ok(());
            }
            self.bits |= u32::from(byte) << self.nbits;
            self.nbits += 8;
            while !self.done && self.nbits >= self.width {
                let code = (self.bits & ((1 << self.width) - 1)) as u16;
                self.bits >>= self.width;
                self.nbits -= self.width;
                if code == clear {
                    self.reset();
                    continue;
                }
                if code == clear + 1 {
                    self.done = true;
                    break;
                }
                let Some(previous) = self.previous else {
                    if code >= clear {
                        return Err(ImageError::Invalid);
                    }
                    self.previous = Some(code);
                    if !out(code as u8) {
                        self.done = true;
                    }
                    continue;
                };
                let first = if code < self.next {
                    self.first[usize::from(code)]
                } else if code == self.next {
                    self.first[usize::from(previous)]
                } else {
                    return Err(ImageError::Invalid);
                };
                if self.next < 4096 {
                    let n = usize::from(self.next);
                    self.prefix[n] = previous;
                    self.suffix[n] = first;
                    self.first[n] = self.first[usize::from(previous)];
                    self.next += 1;
                    if self.next == 1 << self.width && self.width < 12 {
                        self.width += 1;
                    }
                }
                // Walk the chain into the stack, then emit it reversed.
                self.stack.clear();
                let mut c = code;
                while c != NO_CODE {
                    self.stack.push(self.suffix[usize::from(c)]);
                    c = self.prefix[usize::from(c)];
                }
                while let Some(index) = self.stack.pop() {
                    if !out(index) {
                        self.done = true;
                        break;
                    }
                }
                self.previous = Some(code);
            }
        }
        Ok(())
    }
}

/// Row order of an interlaced image: passes start at rows 0, 4, 2, 1 with
/// steps 8, 8, 4, 2.
fn interlaced_row(index: usize, height: usize) -> usize {
    let mut index = index;
    for (start, step) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
        let rows = if start >= height {
            0
        } else {
            (height - start).div_ceil(step)
        };
        if index < rows {
            return start + index * step;
        }
        index -= rows;
    }
    height
}

/// Decode a GIF image, compositing each frame onto the canvas.
pub fn decode_gif(
    bytes: &[u8],
    limits: DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<Animation, ImageError> {
    cancelled(cancellation)?;
    let mut r = Reader {
        data: bytes,
        pos: 0,
    };
    let signature = r.bytes(6).map_err(|_| ImageError::Signature)?;
    if signature != b"GIF87a" && signature != b"GIF89a" {
        return Err(ImageError::Signature);
    }
    let width = r.u16()?;
    let height = r.u16()?;
    let packed = r.u8()?;
    let _background = r.u8()?;
    let _aspect = r.u8()?;
    let canvas_bytes = limits.check(u32::from(width), u32::from(height))?;
    let global = if packed & 0x80 != 0 {
        Some(r.palette(packed)?)
    } else {
        None
    };
    let (w, h) = (usize::from(width), usize::from(height));
    let mut canvas = vec![0u8; canvas_bytes];
    let mut frames: Vec<Frame> = Vec::new();
    let mut control = Control::default();
    let mut loop_count = 1;
    let mut used = 0usize;
    loop {
        cancelled(cancellation)?;
        if frames.len() >= limits.max_frames {
            break;
        }
        let introducer = match r.u8() {
            Ok(b) => b,
            Err(_) if !frames.is_empty() => break,
            Err(e) => return Err(e),
        };
        match introducer {
            0x3b => break,
            0x21 => {
                let label = r.u8();
                let step = label.and_then(|label| match label {
                    0xf9 => {
                        let mut block = Vec::new();
                        r.sub_blocks(|b| {
                            if block.is_empty() {
                                block.extend_from_slice(b);
                            }
                            Ok(())
                        })?;
                        if block.len() < 4 {
                            return Err(ImageError::Invalid);
                        }
                        control = Control {
                            disposal: (block[0] >> 2) & 7,
                            transparent: (block[0] & 1 != 0).then_some(block[3]),
                            delay_cs: u16::from(block[1]) | u16::from(block[2]) << 8,
                        };
                        Ok(())
                    }
                    0xff => {
                        let mut index = 0;
                        let mut netscape = false;
                        r.sub_blocks(|b| {
                            if index == 0 {
                                netscape = b == b"NETSCAPE2.0" || b == b"ANIMEXTS1.0";
                            } else if netscape && b.len() >= 3 && b[0] == 1 {
                                loop_count = u32::from(b[1]) | u32::from(b[2]) << 8;
                            }
                            index += 1;
                            Ok(())
                        })
                    }
                    _ => r.sub_blocks(|_| Ok(())),
                });
                match step {
                    Ok(()) => {}
                    Err(ImageError::Truncated) if !frames.is_empty() => break,
                    Err(e) => return Err(e),
                }
            }
            0x2c => {
                // The output of this frame must fit the byte budget.
                let needed = used.checked_add(canvas_bytes).ok_or(ImageError::Limit)?;
                if needed > limits.max_bytes {
                    if frames.is_empty() {
                        return Err(ImageError::Limit);
                    }
                    break;
                }
                let pixels = match frame(&mut r, &mut canvas, (w, h), global, control, cancellation)
                {
                    Ok(pixels) => pixels,
                    Err(ImageError::Truncated) if !frames.is_empty() => break,
                    Err(e) => return Err(e),
                };
                used = needed;
                frames.push(Frame {
                    image: DecodedImage {
                        width: u32::from(width),
                        height: u32::from(height),
                        pixels,
                    },
                    delay_ms: u32::from(control.delay_cs) * 10,
                });
                control = Control::default();
            }
            _ => {
                if frames.is_empty() {
                    return Err(ImageError::Invalid);
                }
                break;
            }
        }
    }
    if frames.is_empty() {
        return Err(ImageError::Truncated);
    }
    Ok(Animation { frames, loop_count })
}

/// Decode one image descriptor and its data onto `canvas`, returning the
/// frame as displayed; `canvas` is left disposed for the next frame.
fn frame(
    r: &mut Reader<'_>,
    canvas: &mut [u8],
    (w, h): (usize, usize),
    global: Option<&[u8]>,
    control: Control,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, ImageError> {
    let left = usize::from(r.u16()?);
    let top = usize::from(r.u16()?);
    let fw = usize::from(r.u16()?);
    let fh = usize::from(r.u16()?);
    let packed = r.u8()?;
    let local = if packed & 0x80 != 0 {
        Some(r.palette(packed)?)
    } else {
        None
    };
    let interlaced = packed & 0x40 != 0;
    let palette = local.or(global).ok_or(ImageError::Invalid)?;
    let min_size = r.u8()?;
    if !(1..=11).contains(&min_size) {
        return Err(ImageError::Invalid);
    }
    // Restore-to-previous needs the canvas before this frame.
    let saved = (control.disposal == 3).then(|| canvas.to_vec());
    let total = fw * fh;
    let mut index = 0usize;
    let mut emit = |color: u8| -> bool {
        if index >= total {
            return false;
        }
        let row = if interlaced {
            interlaced_row(index / fw, fh)
        } else {
            index / fw
        };
        let (x, y) = (left + index % fw, top + row);
        index += 1;
        if x < w && y < h && control.transparent != Some(color) {
            let at = (y * w + x) * 4;
            let c = usize::from(color) * 3;
            match palette.get(c..c + 3) {
                Some(rgb) => canvas[at..at + 3].copy_from_slice(rgb),
                None => canvas[at..at + 3].fill(0),
            }
            canvas[at + 3] = 255;
        }
        index < total
    };
    let mut lzw = Lzw::new(min_size);
    let mut blocks = 0usize;
    r.sub_blocks(|block| {
        blocks += 1;
        if blocks % 256 == 0 {
            cancelled(cancellation)?;
        }
        lzw.feed(block, &mut emit)
    })?;
    let shown = canvas.to_vec();
    match (control.disposal, saved) {
        (3, Some(saved)) => canvas.copy_from_slice(&saved),
        (2, _) => {
            for y in top..(top + fh).min(h) {
                for x in left..(left + fw).min(w) {
                    canvas[(y * w + x) * 4..][..4].fill(0);
                }
            }
        }
        _ => {}
    }
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interlaced_rows_visit_every_row_once() {
        for height in 1..40 {
            let mut rows: Vec<_> = (0..height).map(|i| interlaced_row(i, height)).collect();
            assert_eq!(rows[0], 0);
            rows.sort_unstable();
            assert_eq!(rows, (0..height).collect::<Vec<_>>(), "height {height}");
        }
    }
}
