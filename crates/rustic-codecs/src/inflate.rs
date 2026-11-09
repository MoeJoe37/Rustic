//! RFC 1951 DEFLATE and RFC 1950 zlib decoding with an output bound.
//!
//! Huffman codes are decoded canonically one bit at a time (at most 15 steps
//! per symbol), so work is linear in the input and output sizes.

use rustic_base::CancellationToken;
use std::{error::Error, fmt};

/// A decompression failure; no partial output is returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InflateError {
    /// The stream ended early.
    Truncated,
    /// The stream violates RFC 1950/1951 (bad header, code, length or distance).
    Invalid,
    /// The zlib Adler-32 checksum does not match the output.
    Checksum,
    /// Output would exceed the caller's limit.
    OutputLimit,
    /// Cooperative cancellation.
    Cancelled,
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "compressed data ends early",
            Self::Invalid => "compressed data is invalid",
            Self::Checksum => "compressed data checksum mismatch",
            Self::OutputLimit => "decompressed data exceeds its limit",
            Self::Cancelled => "decompression cancelled",
        })
    }
}

impl Error for InflateError {}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    count: u32,
}

impl Bits<'_> {
    fn need(&mut self, n: u32) -> Result<u32, InflateError> {
        debug_assert!(n <= 16);
        while self.count < n {
            let byte = *self.data.get(self.pos).ok_or(InflateError::Truncated)?;
            self.pos += 1;
            self.buf |= u32::from(byte) << self.count;
            self.count += 8;
        }
        let value = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.count -= n;
        Ok(value)
    }

    fn align(&mut self) {
        self.buf = 0;
        self.count = 0;
    }
}

/// A canonical Huffman code: counts per length and symbols by code order.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Build from code lengths. Returns the code and how many codes are
    /// left unused (0 when complete); over-subscribed codes are invalid.
    fn new(lengths: &[u8]) -> Result<(Self, i32), InflateError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        let mut left = 1i32;
        for &count in &counts[1..] {
            left = (left << 1) - i32::from(count);
            if left < 0 {
                return Err(InflateError::Invalid);
            }
        }
        let mut offsets = [0u16; 16];
        for len in 1..15 {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(offsets[usize::from(l)])] = symbol as u16;
                offsets[usize::from(l)] += 1;
            }
        }
        Ok((Self { counts, symbols }, left))
    }

    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, InflateError> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.need(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(InflateError::Invalid)
    }

    /// Accept an incomplete code only when it is a single one-bit code
    /// (RFC 1951 §3.2.7 allows one distance code; zlib accepts this shape).
    fn checked(lengths: &[u8]) -> Result<Self, InflateError> {
        let (code, left) = Self::new(lengths)?;
        let used = lengths.iter().filter(|&&l| l != 0).count();
        if left > 0 && !(used == 1 && code.counts[1] == 1) && used != 0 {
            return Err(InflateError::Invalid);
        }
        Ok(code)
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

struct Output<'a> {
    data: Vec<u8>,
    limit: usize,
    cancellation: &'a CancellationToken,
}

impl Output<'_> {
    fn push(&mut self, byte: u8) -> Result<(), InflateError> {
        if self.data.len() >= self.limit {
            return Err(InflateError::OutputLimit);
        }
        if self.data.len() % 65_536 == 0 {
            self.cancellation
                .check()
                .map_err(|_| InflateError::Cancelled)?;
        }
        self.data.push(byte);
        Ok(())
    }
}

fn codes(
    bits: &mut Bits<'_>,
    out: &mut Output<'_>,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), InflateError> {
    loop {
        let symbol = lit.decode(bits)?;
        match symbol {
            0..=255 => out.push(symbol as u8)?,
            256 => return Ok(()),
            257..=285 => {
                let i = usize::from(symbol - 257);
                let length =
                    usize::from(LENGTH_BASE[i]) + bits.need(u32::from(LENGTH_EXTRA[i]))? as usize;
                let d = usize::from(dist.decode(bits)?);
                if d >= 30 {
                    return Err(InflateError::Invalid);
                }
                let distance =
                    usize::from(DIST_BASE[d]) + bits.need(u32::from(DIST_EXTRA[d]))? as usize;
                if distance > out.data.len() {
                    return Err(InflateError::Invalid);
                }
                for _ in 0..length {
                    let byte = out.data[out.data.len() - distance];
                    out.push(byte)?;
                }
            }
            _ => return Err(InflateError::Invalid),
        }
    }
}

fn fixed() -> Result<(Huffman, Huffman), InflateError> {
    let mut lengths = [0u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    Ok((Huffman::new(&lengths)?.0, Huffman::new(&[5u8; 30])?.0))
}

fn dynamic(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), InflateError> {
    let nlen = bits.need(5)? as usize + 257;
    let ndist = bits.need(5)? as usize + 1;
    let ncode = bits.need(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(InflateError::Invalid);
    }
    let mut code_lengths = [0u8; 19];
    for &i in &CODE_LENGTH_ORDER[..ncode] {
        code_lengths[i] = bits.need(3)? as u8;
    }
    let (lencode, left) = Huffman::new(&code_lengths)?;
    if left != 0 {
        return Err(InflateError::Invalid);
    }
    let mut lengths = [0u8; 316];
    let mut index = 0;
    while index < nlen + ndist {
        let symbol = lencode.decode(bits)?;
        if symbol < 16 {
            lengths[index] = symbol as u8;
            index += 1;
            continue;
        }
        let (value, repeat) = match symbol {
            16 => {
                let previous = *lengths[..index].last().ok_or(InflateError::Invalid)?;
                (previous, 3 + bits.need(2)? as usize)
            }
            17 => (0, 3 + bits.need(3)? as usize),
            _ => (0, 11 + bits.need(7)? as usize),
        };
        if index + repeat > nlen + ndist {
            return Err(InflateError::Invalid);
        }
        lengths[index..index + repeat].fill(value);
        index += repeat;
    }
    if lengths[256] == 0 {
        // No end-of-block code: the block could never end.
        return Err(InflateError::Invalid);
    }
    Ok((
        Huffman::checked(&lengths[..nlen])?,
        Huffman::checked(&lengths[nlen..nlen + ndist])?,
    ))
}

fn raw(
    data: &[u8],
    max_output: usize,
    cancellation: &CancellationToken,
) -> Result<(Vec<u8>, usize), InflateError> {
    cancellation.check().map_err(|_| InflateError::Cancelled)?;
    let mut bits = Bits {
        data,
        pos: 0,
        buf: 0,
        count: 0,
    };
    let mut out = Output {
        data: Vec::new(),
        limit: max_output,
        cancellation,
    };
    loop {
        let last = bits.need(1)? == 1;
        match bits.need(2)? {
            0 => {
                bits.align();
                let header = data
                    .get(bits.pos..bits.pos + 4)
                    .ok_or(InflateError::Truncated)?;
                let len = u16::from_le_bytes([header[0], header[1]]);
                let nlen = u16::from_le_bytes([header[2], header[3]]);
                if len != !nlen {
                    return Err(InflateError::Invalid);
                }
                bits.pos += 4;
                let block = data
                    .get(bits.pos..bits.pos + usize::from(len))
                    .ok_or(InflateError::Truncated)?;
                if out.data.len() + block.len() > out.limit {
                    return Err(InflateError::OutputLimit);
                }
                cancellation.check().map_err(|_| InflateError::Cancelled)?;
                out.data.extend_from_slice(block);
                bits.pos += block.len();
            }
            1 => {
                let (lit, dist) = fixed()?;
                codes(&mut bits, &mut out, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = dynamic(&mut bits)?;
                codes(&mut bits, &mut out, &lit, &dist)?;
            }
            _ => return Err(InflateError::Invalid),
        }
        if last {
            return Ok((out.data, bits.pos));
        }
    }
}

/// Decode a raw DEFLATE stream, producing at most `max_output` bytes.
/// Trailing bytes after the final block are ignored.
pub fn inflate(
    data: &[u8],
    max_output: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, InflateError> {
    raw(data, max_output, cancellation).map(|(out, _)| out)
}

/// Decode a zlib stream (no preset dictionary) and verify its Adler-32.
pub fn zlib_decompress(
    data: &[u8],
    max_output: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, InflateError> {
    let header = data.get(..2).ok_or(InflateError::Truncated)?;
    let (cmf, flg) = (header[0], header[1]);
    if cmf & 0x0f != 8 || cmf >> 4 > 7 || (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err(InflateError::Invalid);
    }
    if flg & 0x20 != 0 {
        return Err(InflateError::Invalid);
    }
    let (out, used) = raw(&data[2..], max_output, cancellation)?;
    let trailer = data
        .get(2 + used..2 + used + 4)
        .ok_or(InflateError::Truncated)?;
    let expected = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    if adler32(&out) != expected {
        return Err(InflateError::Checksum);
    }
    Ok(out)
}

pub(crate) fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    const DYNAMIC: [u8; 172] = [
        0x78, 0xda, 0x75, 0x8e, 0xb9, 0x12, 0x82, 0x30, 0x00, 0x44, 0x7f, 0x65, 0x19, 0x8f, 0xf1,
        0x1a, 0xc6, 0xfb, 0x18, 0x1b, 0x4b, 0x4b, 0x0b, 0x6a, 0x9d, 0x20, 0x21, 0x44, 0x43, 0x02,
        0x21, 0x5c, 0x29, 0xfc, 0x76, 0x83, 0x63, 0xa9, 0xf5, 0xdb, 0x7d, 0xbb, 0x41, 0x42, 0x91,
        0x97, 0xfc, 0xfe, 0x44, 0xa8, 0x55, 0x2d, 0x11, 0xab, 0x06, 0x8f, 0x32, 0xcd, 0x0a, 0xa8,
        0x8a, 0x6a, 0x18, 0x87, 0x05, 0xb1, 0x2d, 0x22, 0xc5, 0x7c, 0x5c, 0x88, 0xcb, 0xa5, 0x2d,
        0x42, 0x17, 0xaa, 0xb9, 0x49, 0x10, 0xf3, 0x8a, 0x3a, 0x64, 0xa9, 0x84, 0xe0, 0x79, 0xa9,
        0xb4, 0xeb, 0xb2, 0xc2, 0xc3, 0x59, 0xd5, 0xa8, 0x68, 0xc3, 0x25, 0x13, 0xed, 0x57, 0x1f,
        0x91, 0xd8, 0xc0, 0xd2, 0x50, 0x93, 0xe2, 0x33, 0x70, 0x44, 0x91, 0x25, 0x5c, 0x36, 0x50,
        0x31, 0x42, 0xd1, 0x89, 0xf3, 0x92, 0x68, 0x63, 0x67, 0x8e, 0x46, 0x8c, 0x76, 0x33, 0x95,
        0xaa, 0x7d, 0xcc, 0x17, 0xcb, 0xd5, 0x7a, 0xb3, 0xdd, 0xed, 0x0f, 0x78, 0x79, 0xa7, 0x5e,
        0x7f, 0x70, 0x1d, 0x4e, 0x46, 0xe3, 0xdb, 0x14, 0xc1, 0xdf, 0xe7, 0x84, 0x11, 0x2e, 0x7f,
        0xfc, 0x7f, 0x03, 0x8c, 0x35, 0x51, 0x56,
    ];

    fn z(data: &[u8]) -> Result<Vec<u8>, InflateError> {
        zlib_decompress(data, 1 << 20, &CancellationToken::default())
    }

    #[test]
    fn stored_fixed_and_dynamic_blocks_decode() {
        // zlib.compress(b"hello hello hello hello", level 9): fixed Huffman
        // with a back-reference.
        let fixed = [
            0x78, 0xda, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0x01, 0x68, 0x03,
            0x08, 0xb1,
        ];
        assert_eq!(z(&fixed).unwrap(), b"hello hello hello hello");
        // A stored block.
        let stored = [
            0x78, 0x01, 0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c', 0x02, 0x4d, 0x01, 0x27,
        ];
        assert_eq!(z(&stored).unwrap(), b"abc");
        // zlib level 9 on 236 bytes of text: a dynamic Huffman block.
        let text: &[u8] = b"The quick brown fox jumps over the lazy dog. Pack my box with five dozen liquor jugs! How vexingly quick daft zebras jump; sphinx of black quartz, judge my vow. 0123456789 ~!@#$%^&*()_+ The quick brown fox jumps again over the lazy dog.";
        assert_eq!(z(&DYNAMIC).unwrap(), text);
        assert_eq!((DYNAMIC[2] >> 1) & 3, 2);
        // Empty input compressed by zlib.
        assert_eq!(
            z(&[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]).unwrap(),
            b""
        );
    }

    #[test]
    fn checksum_header_and_truncation_errors_are_typed() {
        let stored = [
            0x78, 0x01, 0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c', 0x02, 0x4d, 0x01, 0x28,
        ];
        assert_eq!(z(&stored), Err(InflateError::Checksum));
        assert_eq!(z(&stored[..9]), Err(InflateError::Truncated));
        assert_eq!(z(&[0x78, 0x02]), Err(InflateError::Invalid));
        assert_eq!(z(&[0x79, 0x9c]), Err(InflateError::Invalid));
        // Block type 3 is reserved.
        assert_eq!(
            inflate(&[0x07], 10, &CancellationToken::default()),
            Err(InflateError::Invalid)
        );
        // A distance reaching before the output start.
        assert_eq!(
            inflate(&[0x03, 0x02, 0x00], 10, &CancellationToken::default()),
            Err(InflateError::Invalid)
        );
    }

    #[test]
    fn output_limit_and_cancellation_stop_bombs() {
        // 1 MiB of zeros compresses to about a kilobyte.
        let bomb = {
            // Fixed block: literal 0, then repeated length-258 distance-1 copies.
            let mut out = Vec::new();
            let mut acc = 0u64;
            let mut n = 0;
            let put = |v: u64, bits: u32, acc: &mut u64, n: &mut u32, out: &mut Vec<u8>| {
                *acc |= v << *n;
                *n += bits;
                while *n >= 8 {
                    out.push(*acc as u8);
                    *acc >>= 8;
                    *n -= 8;
                }
            };
            put(0b011, 3, &mut acc, &mut n, &mut out); // BFINAL=1, BTYPE=01
            // Literal 0: code 00110000 (8 bits, MSB first).
            put(0b0000_1100, 8, &mut acc, &mut n, &mut out);
            for _ in 0..4000 {
                // Length 258: symbol 285, code 11000101 (MSB first).
                put(0b1010_0011, 8, &mut acc, &mut n, &mut out);
                // Distance 1: symbol 0, 5-bit code 00000.
                put(0, 5, &mut acc, &mut n, &mut out);
            }
            // End of block: symbol 256, 7-bit code 0000000.
            put(0, 7, &mut acc, &mut n, &mut out);
            put(0, 7, &mut acc, &mut n, &mut out);
            out
        };
        let full = inflate(&bomb, usize::MAX, &CancellationToken::default()).unwrap();
        assert_eq!(full.len(), 1 + 4000 * 258);
        assert!(full.iter().all(|&b| b == 0));
        assert_eq!(
            inflate(&bomb, full.len() - 1, &CancellationToken::default()),
            Err(InflateError::OutputLimit)
        );
        assert_eq!(
            inflate(&bomb, full.len(), &CancellationToken::default()).unwrap(),
            full
        );
        let cancel = CancellationToken::default();
        cancel.cancel();
        assert_eq!(
            inflate(&bomb, usize::MAX, &cancel),
            Err(InflateError::Cancelled)
        );
    }

    #[test]
    fn adler_matches_reference_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let big = vec![0xffu8; 100_000];
        // Reference computed with the unchunked definition.
        let (mut a, mut b) = (1u64, 0u64);
        for &x in &big {
            a = (a + u64::from(x)) % 65_521;
            b = (b + a) % 65_521;
        }
        assert_eq!(u64::from(adler32(&big)), (b << 16) | a);
    }
}
