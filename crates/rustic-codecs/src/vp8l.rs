//! WebP lossless (VP8L) bitstream decoding, RFC 9649 sections 3–5.
//!
//! Produces ARGB pixels (`0xAARRGGBB`). All four transforms, color caches,
//! meta prefix codes and LZ77 backward references are supported. Sizes of
//! every sub-image, prefix code group and pixel buffer are charged against
//! the caller's byte budget before allocation.

use crate::image::{ImageError, cancelled};
use rustic_base::CancellationToken;

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    count: u32,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            acc: 0,
            count: 0,
        }
    }

    pub(crate) fn bits(&mut self, n: u32) -> Result<u32, ImageError> {
        debug_assert!(n <= 32);
        while self.count < n {
            let byte = *self.data.get(self.pos).ok_or(ImageError::Truncated)?;
            self.acc |= u64::from(byte) << self.count;
            self.pos += 1;
            self.count += 8;
        }
        let value = (self.acc & ((1u64 << n) - 1)) as u32;
        self.acc >>= n;
        self.count -= n;
        Ok(value)
    }

    fn bit(&mut self) -> Result<bool, ImageError> {
        Ok(self.bits(1)? == 1)
    }
}

/// A canonical prefix code decoded one bit at a time (counts per length
/// plus symbols in code order), or a code with a single symbol that reads
/// no bits.
#[derive(Clone, Debug)]
struct Prefix {
    counts: [u16; 16],
    symbols: Vec<u16>,
    single: Option<u16>,
}

impl Prefix {
    fn from_lengths(lengths: &[u8]) -> Result<Self, ImageError> {
        let mut counts = [0u16; 16];
        let mut used = 0;
        let mut last = 0;
        for (symbol, &len) in lengths.iter().enumerate() {
            if len > 0 {
                counts[usize::from(len)] += 1;
                used += 1;
                last = symbol as u16;
            }
        }
        if used == 0 {
            return Err(ImageError::Invalid);
        }
        if used == 1 {
            return Ok(Self {
                counts,
                symbols: Vec::new(),
                single: Some(last),
            });
        }
        // The code must be complete: Σ 2^-len = 1.
        let mut left = 1i32;
        for &count in &counts[1..] {
            left = left * 2 - i32::from(count);
            if left < 0 {
                return Err(ImageError::Invalid);
            }
        }
        if left != 0 {
            return Err(ImageError::Invalid);
        }
        let mut offsets = [0u16; 16];
        for len in 1..15 {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; used];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len > 0 {
                let slot = &mut offsets[usize::from(len)];
                symbols[usize::from(*slot)] = symbol as u16;
                *slot += 1;
            }
        }
        Ok(Self {
            counts,
            symbols,
            single: None,
        })
    }

    fn decode(&self, r: &mut BitReader<'_>) -> Result<u16, ImageError> {
        if let Some(symbol) = self.single {
            return Ok(symbol);
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= r.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - count < first {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(ImageError::Invalid)
    }

    fn bytes(&self) -> usize {
        64 + self.symbols.len() * 2
    }
}

const CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

fn read_prefix(r: &mut BitReader<'_>, alphabet: usize) -> Result<Prefix, ImageError> {
    let mut lengths = vec![0u8; alphabet];
    if r.bit()? {
        // Simple code: one or two symbols.
        let two = r.bit()?;
        let first_bits = if r.bit()? { 8 } else { 1 };
        let s0 = r.bits(first_bits)? as usize;
        *lengths.get_mut(s0).ok_or(ImageError::Invalid)? = 1;
        if two {
            let s1 = r.bits(8)? as usize;
            *lengths.get_mut(s1).ok_or(ImageError::Invalid)? = 1;
        }
        // A repeated symbol leaves one used length.
        return Prefix::from_lengths(&lengths);
    }
    let mut code_lengths = [0u8; 19];
    let n = 4 + r.bits(4)? as usize;
    for &slot in &CODE_LENGTH_ORDER[..n] {
        code_lengths[slot] = r.bits(3)? as u8;
    }
    let length_code = Prefix::from_lengths(&code_lengths)?;
    let mut max_symbol = if r.bit()? {
        let nbits = 2 + 2 * r.bits(3)?;
        let max = 2 + r.bits(nbits)? as usize;
        if max > alphabet {
            return Err(ImageError::Invalid);
        }
        max
    } else {
        alphabet
    };
    let mut symbol = 0;
    let mut previous = 8u8;
    while symbol < alphabet {
        if max_symbol == 0 {
            break;
        }
        max_symbol -= 1;
        let code = length_code.decode(r)?;
        if code < 16 {
            lengths[symbol] = code as u8;
            symbol += 1;
            if code != 0 {
                previous = code as u8;
            }
            continue;
        }
        let (repeat, value) = match code {
            16 => (3 + r.bits(2)? as usize, previous),
            17 => (3 + r.bits(3)? as usize, 0),
            _ => (11 + r.bits(7)? as usize, 0),
        };
        if symbol + repeat > alphabet {
            return Err(ImageError::Invalid);
        }
        lengths[symbol..symbol + repeat].fill(value);
        symbol += repeat;
    }
    Prefix::from_lengths(&lengths)
}

/// Green+length+cache, red, blue, alpha and distance codes.
struct Group([Prefix; 5]);

fn read_group(r: &mut BitReader<'_>, cache_size: usize) -> Result<Group, ImageError> {
    Ok(Group([
        read_prefix(r, 256 + 24 + cache_size)?,
        read_prefix(r, 256)?,
        read_prefix(r, 256)?,
        read_prefix(r, 256)?,
        read_prefix(r, 40)?,
    ]))
}

fn prefix_value(r: &mut BitReader<'_>, prefix: u16) -> Result<usize, ImageError> {
    let prefix = u32::from(prefix);
    if prefix < 4 {
        return Ok(prefix as usize + 1);
    }
    let extra = (prefix - 2) >> 1;
    let offset = (2 + (prefix & 1)) << extra;
    Ok((offset + r.bits(extra)?) as usize + 1)
}

const DISTANCE_MAP: [(i8, i8); 120] = [
    (0, 1),
    (1, 0),
    (1, 1),
    (-1, 1),
    (0, 2),
    (2, 0),
    (1, 2),
    (-1, 2),
    (2, 1),
    (-2, 1),
    (2, 2),
    (-2, 2),
    (0, 3),
    (3, 0),
    (1, 3),
    (-1, 3),
    (3, 1),
    (-3, 1),
    (2, 3),
    (-2, 3),
    (3, 2),
    (-3, 2),
    (0, 4),
    (4, 0),
    (1, 4),
    (-1, 4),
    (4, 1),
    (-4, 1),
    (3, 3),
    (-3, 3),
    (2, 4),
    (-2, 4),
    (4, 2),
    (-4, 2),
    (0, 5),
    (3, 4),
    (-3, 4),
    (4, 3),
    (-4, 3),
    (5, 0),
    (1, 5),
    (-1, 5),
    (5, 1),
    (-5, 1),
    (2, 5),
    (-2, 5),
    (5, 2),
    (-5, 2),
    (4, 4),
    (-4, 4),
    (3, 5),
    (-3, 5),
    (5, 3),
    (-5, 3),
    (0, 6),
    (6, 0),
    (1, 6),
    (-1, 6),
    (6, 1),
    (-6, 1),
    (2, 6),
    (-2, 6),
    (6, 2),
    (-6, 2),
    (4, 5),
    (-4, 5),
    (5, 4),
    (-5, 4),
    (3, 6),
    (-3, 6),
    (6, 3),
    (-6, 3),
    (0, 7),
    (7, 0),
    (1, 7),
    (-1, 7),
    (5, 5),
    (-5, 5),
    (7, 1),
    (-7, 1),
    (4, 6),
    (-4, 6),
    (6, 4),
    (-6, 4),
    (2, 7),
    (-2, 7),
    (7, 2),
    (-7, 2),
    (3, 7),
    (-3, 7),
    (7, 3),
    (-7, 3),
    (5, 6),
    (-5, 6),
    (6, 5),
    (-6, 5),
    (8, 0),
    (4, 7),
    (-4, 7),
    (7, 4),
    (-7, 4),
    (8, 1),
    (8, 2),
    (6, 6),
    (-6, 6),
    (8, 3),
    (5, 7),
    (-5, 7),
    (7, 5),
    (-7, 5),
    (8, 4),
    (6, 7),
    (-6, 7),
    (7, 6),
    (-7, 6),
    (8, 5),
    (7, 7),
    (-7, 7),
    (8, 6),
    (8, 7),
];

enum Transform {
    Predictor { bits: u32, data: Vec<u32> },
    Color { bits: u32, data: Vec<u32> },
    SubtractGreen,
    Indexing { bits: u32, table: Vec<u32> },
}

/// Decoding context: the reader plus the byte budget and cancellation.
pub(crate) struct Lossless<'a, 'c> {
    pub(crate) r: BitReader<'a>,
    pub(crate) budget: usize,
    pub(crate) cancellation: &'c CancellationToken,
}

fn sub_size(size: usize, bits: u32) -> usize {
    size.div_ceil(1 << bits)
}

impl Lossless<'_, '_> {
    fn charge(&mut self, bytes: usize) -> Result<(), ImageError> {
        self.budget = self.budget.checked_sub(bytes).ok_or(ImageError::Limit)?;
        Ok(())
    }

    /// Decode a full image stream of the given size (with transforms) into
    /// ARGB pixels.
    pub(crate) fn image(&mut self, width: usize, height: usize) -> Result<Vec<u32>, ImageError> {
        let mut transforms = Vec::new();
        let mut seen = [false; 4];
        let mut xsize = width;
        while self.r.bit()? {
            let kind = self.r.bits(2)? as usize;
            if std::mem::replace(&mut seen[kind], true) {
                return Err(ImageError::Invalid);
            }
            let transform = match kind {
                0 | 1 => {
                    let bits = self.r.bits(3)? + 2;
                    let data = self.entropy_image(sub_size(xsize, bits), sub_size(height, bits))?;
                    if kind == 0 {
                        Transform::Predictor { bits, data }
                    } else {
                        Transform::Color { bits, data }
                    }
                }
                2 => Transform::SubtractGreen,
                _ => {
                    let size = self.r.bits(8)? as usize + 1;
                    let mut table = self.entropy_image(size, 1)?;
                    for i in 1..table.len() {
                        table[i] = add_pixels(table[i], table[i - 1]);
                    }
                    let bits = match size {
                        0..=2 => 3,
                        3..=4 => 2,
                        5..=16 => 1,
                        _ => 0,
                    };
                    xsize = sub_size(xsize, bits);
                    Transform::Indexing { bits, table }
                }
            };
            transforms.push(transform);
        }
        let mut pixels = self.coded_image(xsize, height, true)?;
        for transform in transforms.iter().rev() {
            cancelled(self.cancellation)?;
            pixels = match transform {
                Transform::Predictor { bits, data } => {
                    predict(&mut pixels, width, height, *bits, data);
                    pixels
                }
                Transform::Color { bits, data } => {
                    uncolor(&mut pixels, width, *bits, data);
                    pixels
                }
                Transform::SubtractGreen => {
                    for p in &mut pixels {
                        let g = (*p >> 8) & 0xff;
                        let r = (((*p >> 16) & 0xff) + g) & 0xff;
                        let b = ((*p & 0xff) + g) & 0xff;
                        *p = (*p & 0xff00_ff00) | r << 16 | b;
                    }
                    pixels
                }
                Transform::Indexing { bits, table } => {
                    self.charge(width * height * 4)?;
                    unindex(&pixels, width, height, *bits, table)
                }
            };
        }
        Ok(pixels)
    }

    /// A sub-image: no transforms or meta prefix codes.
    fn entropy_image(&mut self, width: usize, height: usize) -> Result<Vec<u32>, ImageError> {
        self.coded_image(width, height, false)
    }

    fn coded_image(
        &mut self,
        width: usize,
        height: usize,
        main: bool,
    ) -> Result<Vec<u32>, ImageError> {
        let total = width.checked_mul(height).ok_or(ImageError::Limit)?;
        self.charge(total.checked_mul(4).ok_or(ImageError::Limit)?)?;
        let cache_bits = if self.r.bit()? {
            let bits = self.r.bits(4)?;
            if !(1..=11).contains(&bits) {
                return Err(ImageError::Invalid);
            }
            bits
        } else {
            0
        };
        let cache_size = if cache_bits > 0 {
            1usize << cache_bits
        } else {
            0
        };
        let mut meta = None;
        let mut groups_count = 1;
        if main && self.r.bit()? {
            let bits = self.r.bits(3)? + 2;
            let image = self.entropy_image(sub_size(width, bits), sub_size(height, bits))?;
            groups_count = image
                .iter()
                .map(|p| ((p >> 8) & 0xffff) as usize + 1)
                .max()
                .unwrap_or(1);
            meta = Some((bits, sub_size(width, bits), image));
        }
        // Each group needs room for its codes before they are read.
        self.charge(groups_count.saturating_mul(5 * 64))?;
        let mut groups = Vec::with_capacity(groups_count);
        for _ in 0..groups_count {
            let group = read_group(&mut self.r, cache_size)?;
            self.charge(group.0.iter().map(Prefix::bytes).sum())?;
            groups.push(group);
        }
        self.charge(cache_size * 4)?;
        let mut cache = vec![0u32; cache_size];
        let mut pixels = vec![0u32; total];
        let mut pos = 0;
        let mut cached = 0;
        let group_at = |pos: usize| -> usize {
            match &meta {
                Some((bits, mw, image)) => {
                    let (x, y) = (pos % width, pos / width);
                    ((image[(y >> bits) * mw + (x >> bits)] >> 8) & 0xffff) as usize
                }
                None => 0,
            }
        };
        while pos < total {
            if pos % (1 << 16) == 0 {
                cancelled(self.cancellation)?;
            }
            let codes = &groups[group_at(pos)].0;
            let s = usize::from(codes[0].decode(&mut self.r)?);
            if s < 256 {
                let red = u32::from(codes[1].decode(&mut self.r)?);
                let blue = u32::from(codes[2].decode(&mut self.r)?);
                let alpha = u32::from(codes[3].decode(&mut self.r)?);
                pixels[pos] = alpha << 24 | red << 16 | (s as u32) << 8 | blue;
                pos += 1;
            } else if s < 256 + 24 {
                let length = prefix_value(&mut self.r, (s - 256) as u16)?;
                let dist_symbol = codes[4].decode(&mut self.r)?;
                let dist_code = prefix_value(&mut self.r, dist_symbol)?;
                let dist = if dist_code > 120 {
                    dist_code - 120
                } else {
                    let (dx, dy) = DISTANCE_MAP[dist_code - 1];
                    let d = isize::from(dx) + isize::from(dy) * width as isize;
                    d.max(1) as usize
                };
                if dist > pos || length > total - pos {
                    return Err(ImageError::Invalid);
                }
                for i in pos..pos + length {
                    pixels[i] = pixels[i - dist];
                }
                pos += length;
            } else {
                let index = s - 256 - 24;
                // Bring the cache up to date before reading from it.
                while cached < pos {
                    insert(&mut cache, cache_bits, pixels[cached]);
                    cached += 1;
                }
                pixels[pos] = *cache.get(index).ok_or(ImageError::Invalid)?;
                pos += 1;
            }
        }
        Ok(pixels)
    }
}

fn insert(cache: &mut [u32], bits: u32, argb: u32) {
    if bits > 0 {
        let index = (0x1e35_a7bdu32.wrapping_mul(argb) >> (32 - bits)) as usize;
        cache[index] = argb;
    }
}

fn add_pixels(a: u32, b: u32) -> u32 {
    let ag = (a & 0xff00_ff00).wrapping_add(b & 0xff00_ff00) & 0xff00_ff00;
    let rb = (a & 0x00ff_00ff).wrapping_add(b & 0x00ff_00ff) & 0x00ff_00ff;
    ag | rb
}

fn channel(p: u32, shift: u32) -> i32 {
    ((p >> shift) & 0xff) as i32
}

fn average2(a: u32, b: u32) -> u32 {
    (((a ^ b) & 0xfefe_fefe) >> 1) + (a & b)
}

fn per_channel(f: impl Fn(u32) -> i32) -> u32 {
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        out |= (f(shift).clamp(0, 255) as u32) << shift;
    }
    out
}

fn select(l: u32, t: u32, tl: u32) -> u32 {
    let mut pl = 0;
    let mut pt = 0;
    for shift in [0, 8, 16, 24] {
        pl += (channel(t, shift) - channel(tl, shift)).abs();
        pt += (channel(l, shift) - channel(tl, shift)).abs();
    }
    if pl < pt { l } else { t }
}

fn predictor(mode: u32, l: u32, t: u32, tr: u32, tl: u32) -> u32 {
    match mode {
        1 => l,
        2 => t,
        3 => tr,
        4 => tl,
        5 => average2(average2(l, tr), t),
        6 => average2(l, tl),
        7 => average2(l, t),
        8 => average2(tl, t),
        9 => average2(t, tr),
        10 => average2(average2(l, tl), average2(t, tr)),
        11 => select(l, t, tl),
        12 => per_channel(|s| channel(l, s) + channel(t, s) - channel(tl, s)),
        13 => {
            let a = average2(l, t);
            per_channel(|s| channel(a, s) + (channel(a, s) - channel(tl, s)) / 2)
        }
        _ => 0xff00_0000,
    }
}

fn predict(pixels: &mut [u32], width: usize, height: usize, bits: u32, data: &[u32]) {
    let bw = sub_size(width, bits);
    for y in 0..height {
        for x in 0..width {
            let pos = y * width + x;
            let pred = if pos == 0 {
                0xff00_0000
            } else if y == 0 {
                pixels[pos - 1]
            } else if x == 0 {
                pixels[pos - width]
            } else {
                let mode = (data[(y >> bits) * bw + (x >> bits)] >> 8) & 0xf;
                predictor(
                    mode,
                    pixels[pos - 1],
                    pixels[pos - width],
                    pixels[pos - width + 1],
                    pixels[pos - width - 1],
                )
            };
            pixels[pos] = add_pixels(pixels[pos], pred);
        }
    }
}

fn delta(t: u32, c: u32) -> i32 {
    (i32::from(t as u8 as i8) * i32::from(c as u8 as i8)) >> 5
}

fn uncolor(pixels: &mut [u32], width: usize, bits: u32, data: &[u32]) {
    let bw = sub_size(width, bits);
    for (pos, p) in pixels.iter_mut().enumerate() {
        let (x, y) = (pos % width, pos / width);
        let m = data[(y >> bits) * bw + (x >> bits)];
        let (g2r, g2b, r2b) = (m & 0xff, (m >> 8) & 0xff, (m >> 16) & 0xff);
        let green = (*p >> 8) & 0xff;
        let red = ((((*p >> 16) & 0xff) as i32 + delta(g2r, green)) & 0xff) as u32;
        let blue = (((*p & 0xff) as i32 + delta(g2b, green) + delta(r2b, red)) & 0xff) as u32;
        *p = (*p & 0xff00_ff00) | red << 16 | blue;
    }
}

fn unindex(packed: &[u32], width: usize, height: usize, bits: u32, table: &[u32]) -> Vec<u32> {
    let packed_width = sub_size(width, bits);
    let per = 1usize << bits;
    let index_bits = 8 >> bits;
    let mask = (1u32 << index_bits) - 1;
    let mut out = vec![0u32; width * height];
    for y in 0..height {
        for x in 0..width {
            let p = packed[y * packed_width + x / per];
            let index = ((p >> 8) >> ((x % per) as u32 * index_bits)) & mask;
            out[y * width + x] = table.get(index as usize).copied().unwrap_or(0);
        }
    }
    out
}

/// Decode a VP8L chunk payload (with its 5-byte header) to ARGB pixels.
pub(crate) fn decode(
    data: &[u8],
    budget: usize,
    cancellation: &CancellationToken,
) -> Result<(usize, usize, Vec<u32>), ImageError> {
    let mut r = BitReader::new(data);
    if r.bits(8)? != 0x2f {
        return Err(ImageError::Invalid);
    }
    let width = r.bits(14)? as usize + 1;
    let height = r.bits(14)? as usize + 1;
    let _alpha_hint = r.bit()?;
    if r.bits(3)? != 0 {
        return Err(ImageError::Invalid);
    }
    let mut decoder = Lossless {
        r,
        budget,
        cancellation,
    };
    let pixels = decoder.image(width, height)?;
    Ok((width, height, pixels))
}

/// Decode a headerless image stream of known size (the ALPH chunk's
/// lossless alpha data).
pub(crate) fn decode_headless(
    data: &[u8],
    width: usize,
    height: usize,
    budget: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u32>, ImageError> {
    let mut decoder = Lossless {
        r: BitReader::new(data),
        budget,
        cancellation,
    };
    decoder.image(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_codes_must_be_complete() {
        assert!(Prefix::from_lengths(&[1, 1]).is_ok());
        assert!(Prefix::from_lengths(&[1, 2, 2]).is_ok());
        assert!(Prefix::from_lengths(&[1, 2]).is_err());
        assert!(Prefix::from_lengths(&[1, 1, 1]).is_err());
        assert!(Prefix::from_lengths(&[0, 0]).is_err());
        assert_eq!(Prefix::from_lengths(&[0, 3]).unwrap().single, Some(1));
    }

    #[test]
    fn prefix_values_follow_the_lz77_coding() {
        for (prefix, expected_min) in [(0u16, 1usize), (3, 4), (4, 5), (5, 7), (6, 9)] {
            let mut r = BitReader::new(&[0; 4]);
            assert_eq!(prefix_value(&mut r, prefix).unwrap(), expected_min);
        }
        let mut r = BitReader::new(&[0xff; 4]);
        // Prefix 6: 2 extra bits, offset 8, value 8 + 3 + 1.
        assert_eq!(prefix_value(&mut r, 6).unwrap(), 12);
    }

    #[test]
    fn predictor_modes_follow_the_specification() {
        let (l, t, tr, tl) = (0x10203040, 0x20406080, 0x01020304, 0x08080808);
        assert_eq!(predictor(0, l, t, tr, tl), 0xff00_0000);
        assert_eq!(predictor(7, l, t, tr, tl), 0x18304860);
        assert_eq!(predictor(12, l, t, tr, tl), 0x2858_88b8);
        assert_eq!(predictor(11, l, t, tr, tl), t);
        assert_eq!(add_pixels(0xff01_02ff, 0x0101_0101), 0x0002_0300);
    }
}
