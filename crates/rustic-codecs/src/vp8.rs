//! VP8 key-frame decoding for lossy WebP (RFC 6386), to Y'CbCr 4:2:0
//! planes, plus the conversion to RGBA used by browsers.
//!
//! Implements the boolean entropy decoder, frame header, segmentation,
//! intra prediction (16×16, 4×4 and chroma modes), token decoding with
//! probability updates, dequantization, the inverse WHT and DCT, and the
//! normal and simple loop filters. Intra prediction reads unfiltered
//! pixels and the loop filter runs over the finished frame in macroblock
//! order, which is equivalent to the reference decoder's delayed filter.
//! Inter frames never occur in WebP and are rejected.

use crate::image::{ImageError, cancelled};
use crate::vp8_tables::{
    AC_QUANT, COEFF_PROBS, COEFF_UPDATE_PROBS, DC_QUANT, KF_BMODE_PROBS, PCAT6,
};
use rustic_base::CancellationToken;

/// The boolean entropy decoder of RFC 6386 section 7. Reading past the end
/// supplies zero bytes, as the reference decoder does.
struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    value: u32,
    range: u32,
    bit_count: u32,
}

impl<'a> BoolDecoder<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut d = Self {
            data,
            pos: 0,
            value: 0,
            range: 255,
            bit_count: 0,
        };
        d.value = (d.next_byte() << 8) | d.next_byte();
        d
    }

    fn next_byte(&mut self) -> u32 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        u32::from(byte)
    }

    fn bool(&mut self, prob: u8) -> bool {
        let split = 1 + (((self.range - 1) * u32::from(prob)) >> 8);
        let big_split = split << 8;
        let bit = if self.value >= big_split {
            self.range -= split;
            self.value -= big_split;
            true
        } else {
            self.range = split;
            false
        };
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                self.value |= self.next_byte();
            }
        }
        bit
    }

    fn literal(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.bool(128));
        }
        v
    }

    fn flag(&mut self) -> bool {
        self.bool(128)
    }

    /// An optional signed value: flag, magnitude, sign.
    fn signed(&mut self, n: u32) -> i32 {
        if !self.flag() {
            return 0;
        }
        let v = self.literal(n) as i32;
        if self.flag() { -v } else { v }
    }

    /// Read a value from a tree (RFC 6386 section 8.1): positive entries
    /// index nodes, non-positive entries are negated leaves.
    fn tree(&mut self, tree: &[i8], probs: &[u8]) -> u8 {
        let mut i = 0usize;
        loop {
            let next = tree[i + usize::from(self.bool(probs[i >> 1]))];
            if next <= 0 {
                return (-next) as u8;
            }
            i = next as usize;
        }
    }
}

// Macroblock luma modes.
const DC_PRED: u8 = 0;
const V_PRED: u8 = 1;
const H_PRED: u8 = 2;
const TM_PRED: u8 = 3;
const B_PRED: u8 = 4;

// Subblock modes, in RFC 6386 order.
const B_DC: u8 = 0;
const B_TM: u8 = 1;
const B_VE: u8 = 2;
const B_HE: u8 = 3;
const B_LD: u8 = 4;
const B_RD: u8 = 5;
const B_VR: u8 = 6;
const B_VL: u8 = 7;
const B_HD: u8 = 8;
const B_HU: u8 = 9;

const KF_YMODE_TREE: [i8; 8] = [
    -(B_PRED as i8),
    2,
    4,
    6,
    -(DC_PRED as i8),
    -(V_PRED as i8),
    -(H_PRED as i8),
    -(TM_PRED as i8),
];
const KF_YMODE_PROBS: [u8; 4] = [145, 156, 163, 128];
const UV_MODE_TREE: [i8; 6] = [
    -(DC_PRED as i8),
    2,
    -(V_PRED as i8),
    4,
    -(H_PRED as i8),
    -(TM_PRED as i8),
];
const KF_UV_MODE_PROBS: [u8; 3] = [142, 114, 183];
const BMODE_TREE: [i8; 18] = [
    -(B_DC as i8),
    2,
    -(B_TM as i8),
    4,
    -(B_VE as i8),
    6,
    8,
    12,
    -(B_HE as i8),
    10,
    -(B_RD as i8),
    -(B_VR as i8),
    -(B_LD as i8),
    14,
    -(B_VL as i8),
    16,
    -(B_HD as i8),
    -(B_HU as i8),
];
const SEGMENT_TREE: [i8; 6] = [2, 4, 0, -1, -2, -3];

const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
const BANDS: [usize; 17] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7, 0];
const PCAT: [&[u8]; 4] = [
    &[173, 148, 140],
    &[176, 155, 140, 135],
    &[180, 157, 141, 134, 130],
    &PCAT6,
];

type Probs = [[[[u8; 11]; 3]; 8]; 4];

#[derive(Clone, Copy, Default)]
struct Quant {
    y1: [i32; 2],
    y2: [i32; 2],
    uv: [i32; 2],
}

#[derive(Clone, Copy, Default)]
struct FilterInfo {
    limit: i32,
    ilevel: i32,
    hev: i32,
    inner: bool,
}

/// Decoded Y'CbCr planes, cropped dimensions and padded strides.
pub(crate) struct Yuv {
    pub(crate) width: usize,
    pub(crate) height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    y_stride: usize,
    uv_stride: usize,
}

fn read_coeffs(
    d: &mut BoolDecoder<'_>,
    probs: &[[[u8; 11]; 3]; 8],
    ctx: usize,
    dq: [i32; 2],
    first: usize,
    out: &mut [i32; 16],
) -> usize {
    let mut n = first;
    let mut p = &probs[BANDS[n]][ctx];
    while n < 16 {
        if !d.bool(p[0]) {
            return n;
        }
        while !d.bool(p[1]) {
            n += 1;
            if n == 16 {
                return 16;
            }
            p = &probs[BANDS[n]][0];
        }
        let v: i32;
        let next_ctx;
        if !d.bool(p[2]) {
            v = 1;
            next_ctx = 1;
        } else {
            v = if !d.bool(p[3]) {
                if !d.bool(p[4]) {
                    2
                } else {
                    3 + i32::from(d.bool(p[5]))
                }
            } else if !d.bool(p[6]) {
                if !d.bool(p[7]) {
                    5 + i32::from(d.bool(159))
                } else {
                    7 + 2 * i32::from(d.bool(165)) + i32::from(d.bool(145))
                }
            } else {
                let bit1 = usize::from(d.bool(p[8]));
                let bit0 = usize::from(d.bool(p[9 + bit1]));
                let cat = 2 * bit1 + bit0;
                let mut v = 0;
                for &prob in PCAT[cat] {
                    v = 2 * v + i32::from(d.bool(prob));
                }
                v + 3 + (8 << cat)
            };
            next_ctx = 2;
        }
        let signed = if d.bool(128) { -v } else { v };
        out[ZIGZAG[n]] = signed * dq[usize::from(n > 0)];
        n += 1;
        if n < 16 {
            p = &probs[BANDS[n]][next_ctx];
        }
    }
    16
}

/// Inverse Walsh–Hadamard transform of the Y2 block (RFC 6386 14.3).
fn iwht(input: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let a1 = input[i] + input[12 + i];
        let b1 = input[4 + i] + input[8 + i];
        let c1 = input[4 + i] - input[8 + i];
        let d1 = input[i] - input[12 + i];
        t[i] = a1 + b1;
        t[4 + i] = c1 + d1;
        t[8 + i] = a1 - b1;
        t[12 + i] = d1 - c1;
    }
    let mut out = [0i32; 16];
    for i in 0..4 {
        let a1 = t[4 * i] + t[4 * i + 3];
        let b1 = t[4 * i + 1] + t[4 * i + 2];
        let c1 = t[4 * i + 1] - t[4 * i + 2];
        let d1 = t[4 * i] - t[4 * i + 3];
        out[4 * i] = (a1 + b1 + 3) >> 3;
        out[4 * i + 1] = (c1 + d1 + 3) >> 3;
        out[4 * i + 2] = (a1 - b1 + 3) >> 3;
        out[4 * i + 3] = (d1 - c1 + 3) >> 3;
    }
    out
}

/// Inverse DCT of one 4×4 block (RFC 6386 14.4), added to `dst`.
fn idct_add(input: &[i32; 16], dst: &mut [u8], at: usize, stride: usize) {
    // cos(π/8)·√2 − 1 and sin(π/8)·√2 in Q16; i64 keeps corrupt
    // coefficients from overflowing.
    let c1m = |v: i32| ((i64::from(v) * 20091) >> 16) as i32 + v;
    let s1m = |v: i32| ((i64::from(v) * 35468) >> 16) as i32;
    let mut t = [0i32; 16];
    for i in 0..4 {
        let (i0, i4, i8, i12) = (input[i], input[4 + i], input[8 + i], input[12 + i]);
        let a1 = i0 + i8;
        let b1 = i0 - i8;
        let c1 = s1m(i4) - c1m(i12);
        let d1 = c1m(i4) + s1m(i12);
        t[i] = a1 + d1;
        t[12 + i] = a1 - d1;
        t[4 + i] = b1 + c1;
        t[8 + i] = b1 - c1;
    }
    for row in 0..4 {
        let r = &t[row * 4..row * 4 + 4];
        let a1 = r[0] + r[2];
        let b1 = r[0] - r[2];
        let c1 = s1m(r[1]) - c1m(r[3]);
        let d1 = c1m(r[1]) + s1m(r[3]);
        let values = [
            (a1 + d1 + 4) >> 3,
            (b1 + c1 + 4) >> 3,
            (b1 - c1 + 4) >> 3,
            (a1 - d1 + 4) >> 3,
        ];
        for (x, value) in values.into_iter().enumerate() {
            let p = &mut dst[at + row * stride + x];
            *p = (i32::from(*p) + value).clamp(0, 255) as u8;
        }
    }
}

/// A prediction work area: `size+1` rows of `size+5` samples. Row 0 holds
/// the top-left sample and the row above (plus 4 above-right samples);
/// column 0 holds the left column.
struct Work {
    data: [u8; 21 * 17],
    stride: usize,
}

impl Work {
    fn at(&self, x: isize, y: isize) -> i32 {
        i32::from(self.data[((y + 1) as usize) * self.stride + (x + 1) as usize])
    }
    fn set(&mut self, x: usize, y: usize, v: i32) {
        self.data[(y + 1) * self.stride + x + 1] = v.clamp(0, 255) as u8;
    }
}

fn avg2(a: i32, b: i32) -> i32 {
    (a + b + 1) >> 1
}

fn avg3(a: i32, b: i32, c: i32) -> i32 {
    (a + 2 * b + c + 2) >> 2
}

/// Whole-block prediction for 16×16 luma or 8×8 chroma.
fn predict_block(w: &mut Work, size: usize, mode: u8, has_top: bool, has_left: bool) {
    let n = size as isize;
    match mode {
        DC_PRED => {
            let shift = size.trailing_zeros();
            let top: i32 = (0..n).map(|x| w.at(x, -1)).sum();
            let left: i32 = (0..n).map(|y| w.at(-1, y)).sum();
            let dc = match (has_top, has_left) {
                (true, true) => (top + left + n as i32) >> (shift + 1),
                (true, false) => (top + (n as i32 >> 1)) >> shift,
                (false, true) => (left + (n as i32 >> 1)) >> shift,
                (false, false) => 128,
            };
            for y in 0..size {
                for x in 0..size {
                    w.set(x, y, dc);
                }
            }
        }
        V_PRED => {
            for y in 0..size {
                for x in 0..size {
                    let v = w.at(x as isize, -1);
                    w.set(x, y, v);
                }
            }
        }
        H_PRED => {
            for y in 0..size {
                let v = w.at(-1, y as isize);
                for x in 0..size {
                    w.set(x, y, v);
                }
            }
        }
        _ => {
            let p = w.at(-1, -1);
            for y in 0..size {
                let l = w.at(-1, y as isize);
                for x in 0..size {
                    let a = w.at(x as isize, -1);
                    w.set(x, y, l + a - p);
                }
            }
        }
    }
}

/// 4×4 subblock prediction at `(bx, by)` inside the luma work area.
fn predict_sub(w: &mut Work, bx: usize, by: usize, mode: u8) {
    let (ox, oy) = ((bx * 4) as isize, (by * 4) as isize);
    let top = |w: &Work, i: isize| w.at(ox + i, oy - 1);
    let left = |w: &Work, i: isize| w.at(ox - 1, oy + i);
    let p = w.at(ox - 1, oy - 1);
    let a: [i32; 8] = std::array::from_fn(|i| top(w, i as isize));
    let l: [i32; 4] = std::array::from_fn(|i| left(w, i as isize));
    let mut out = [[0i32; 4]; 4]; // out[y][x]
    match mode {
        B_DC => {
            let dc = (a[..4].iter().sum::<i32>() + l.iter().sum::<i32>() + 4) >> 3;
            out = [[dc; 4]; 4];
        }
        B_TM => {
            for (y, row) in out.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    *v = l[y] + a[x] - p;
                }
            }
        }
        B_VE => {
            let row = [
                avg3(p, a[0], a[1]),
                avg3(a[0], a[1], a[2]),
                avg3(a[1], a[2], a[3]),
                avg3(a[2], a[3], a[4]),
            ];
            out = [row; 4];
        }
        B_HE => {
            let col = [
                avg3(p, l[0], l[1]),
                avg3(l[0], l[1], l[2]),
                avg3(l[1], l[2], l[3]),
                avg3(l[2], l[3], l[3]),
            ];
            for (y, row) in out.iter_mut().enumerate() {
                *row = [col[y]; 4];
            }
        }
        B_LD => {
            for (y, row) in out.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    let i = x + y;
                    *v = if i == 6 {
                        avg3(a[6], a[7], a[7])
                    } else {
                        avg3(a[i], a[i + 1], a[i + 2])
                    };
                }
            }
        }
        B_RD => {
            // Edge from bottom-left up to top-right: L3 L2 L1 L0 P A0..A3.
            let e = [l[3], l[2], l[1], l[0], p, a[0], a[1], a[2], a[3]];
            for (y, row) in out.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    let i = 3 - y + x; // centre index in e
                    *v = avg3(e[i], e[i + 1], e[i + 2]);
                }
            }
        }
        B_VR => {
            let (i, j, k) = (l[0], l[1], l[2]);
            out[0] = [
                avg2(p, a[0]),
                avg2(a[0], a[1]),
                avg2(a[1], a[2]),
                avg2(a[2], a[3]),
            ];
            out[1] = [
                avg3(i, p, a[0]),
                avg3(p, a[0], a[1]),
                avg3(a[0], a[1], a[2]),
                avg3(a[1], a[2], a[3]),
            ];
            out[2] = [avg3(j, i, p), out[0][0], out[0][1], out[0][2]];
            out[3] = [avg3(k, j, i), out[1][0], out[1][1], out[1][2]];
        }
        B_VL => {
            out[0] = [
                avg2(a[0], a[1]),
                avg2(a[1], a[2]),
                avg2(a[2], a[3]),
                avg2(a[3], a[4]),
            ];
            out[1] = [
                avg3(a[0], a[1], a[2]),
                avg3(a[1], a[2], a[3]),
                avg3(a[2], a[3], a[4]),
                avg3(a[3], a[4], a[5]),
            ];
            out[2] = [out[0][1], out[0][2], out[0][3], avg3(a[4], a[5], a[6])];
            out[3] = [out[1][1], out[1][2], out[1][3], avg3(a[5], a[6], a[7])];
        }
        B_HD => {
            let (i, j, k, ll) = (l[0], l[1], l[2], l[3]);
            out[0] = [
                avg2(i, p),
                avg3(i, p, a[0]),
                avg3(p, a[0], a[1]),
                avg3(a[0], a[1], a[2]),
            ];
            out[1] = [avg2(j, i), avg3(j, i, p), out[0][0], out[0][1]];
            out[2] = [avg2(k, j), avg3(k, j, i), out[1][0], out[1][1]];
            out[3] = [avg2(ll, k), avg3(ll, k, j), out[2][0], out[2][1]];
        }
        _ => {
            // B_HU
            let (i, j, k, ll) = (l[0], l[1], l[2], l[3]);
            out[0] = [avg2(i, j), avg3(i, j, k), avg2(j, k), avg3(j, k, ll)];
            out[1] = [avg2(j, k), avg3(j, k, ll), avg2(k, ll), avg3(k, ll, ll)];
            out[2] = [avg2(k, ll), avg3(k, ll, ll), ll, ll];
            out[3] = [ll; 4];
        }
    }
    for (y, row) in out.iter().enumerate() {
        for (x, &v) in row.iter().enumerate() {
            w.set(bx * 4 + x, by * 4 + y, v);
        }
    }
}

struct Header {
    segmentation: bool,
    update_map: bool,
    absolute: bool,
    seg_quant: [i32; 4],
    seg_filter: [i32; 4],
    seg_probs: [u8; 3],
    simple_filter: bool,
    level: i32,
    sharpness: i32,
    lf_delta: bool,
    ref_delta0: i32,
    mode_delta0: i32,
}

/// Decode a VP8 key frame (the payload of a `VP8 ` chunk).
pub(crate) fn decode(
    data: &[u8],
    budget: usize,
    cancellation: &CancellationToken,
) -> Result<Yuv, ImageError> {
    if data.len() < 10 {
        return Err(ImageError::Truncated);
    }
    let tag = u32::from(data[0]) | u32::from(data[1]) << 8 | u32::from(data[2]) << 16;
    if tag & 1 != 0 {
        // Inter frames cannot occur in WebP.
        return Err(ImageError::Invalid);
    }
    if (tag >> 1) & 7 > 3 || &data[3..6] != b"\x9d\x01\x2a" {
        return Err(ImageError::Invalid);
    }
    let first_size = (tag >> 5) as usize;
    let width = usize::from(u16::from_le_bytes([data[6], data[7]]) & 0x3fff);
    let height = usize::from(u16::from_le_bytes([data[8], data[9]]) & 0x3fff);
    if width == 0 || height == 0 {
        return Err(ImageError::Invalid);
    }
    let (mbw, mbh) = (width.div_ceil(16), height.div_ceil(16));
    let y_stride = mbw * 16;
    let uv_stride = mbw * 8;
    // Planes, per-macroblock state and the RGBA output.
    let need = y_stride * mbh * 16 * 3 / 2 + mbw * mbh * 16 + width * height * 4;
    if need > budget {
        return Err(ImageError::Limit);
    }
    let rest = &data[10..];
    let first = rest.get(..first_size).ok_or(ImageError::Truncated)?;
    let mut d = BoolDecoder::new(first);
    let _color_space = d.flag();
    let _clamping = d.flag();
    let mut h = Header {
        segmentation: d.flag(),
        update_map: false,
        absolute: false,
        seg_quant: [0; 4],
        seg_filter: [0; 4],
        seg_probs: [255; 3],
        simple_filter: false,
        level: 0,
        sharpness: 0,
        lf_delta: false,
        ref_delta0: 0,
        mode_delta0: 0,
    };
    if h.segmentation {
        h.update_map = d.flag();
        let update_data = d.flag();
        if update_data {
            h.absolute = d.flag();
            for q in &mut h.seg_quant {
                *q = d.signed(7);
            }
            for f in &mut h.seg_filter {
                *f = d.signed(6);
            }
        }
        if h.update_map {
            for p in &mut h.seg_probs {
                *p = if d.flag() { d.literal(8) as u8 } else { 255 };
            }
        }
    }
    h.simple_filter = d.flag();
    h.level = d.literal(6) as i32;
    h.sharpness = d.literal(3) as i32;
    h.lf_delta = d.flag();
    if h.lf_delta && d.flag() {
        for i in 0..4 {
            let v = d.signed(6);
            if i == 0 {
                h.ref_delta0 = v;
            }
        }
        for i in 0..4 {
            let v = d.signed(6);
            if i == 0 {
                h.mode_delta0 = v;
            }
        }
    }
    let partitions = 1usize << d.literal(2);
    let base_q = d.literal(7) as i32;
    let deltas: [i32; 5] = std::array::from_fn(|_| d.signed(4));
    let [y1_dc, y2_dc, y2_ac, uv_dc, uv_ac] = deltas;
    let _refresh_probs = d.flag();
    let mut probs: Probs = COEFF_PROBS;
    for (i, plane) in probs.iter_mut().enumerate() {
        for (j, band) in plane.iter_mut().enumerate() {
            for (k, ctx) in band.iter_mut().enumerate() {
                for (l, p) in ctx.iter_mut().enumerate() {
                    if d.bool(COEFF_UPDATE_PROBS[i][j][k][l]) {
                        *p = d.literal(8) as u8;
                    }
                }
            }
        }
    }
    let skip_prob = if d.flag() {
        Some(d.literal(8) as u8)
    } else {
        None
    };

    // Token partitions.
    let sizes_at = first_size;
    let sizes_len = 3 * (partitions - 1);
    let sizes = rest
        .get(sizes_at..sizes_at + sizes_len)
        .ok_or(ImageError::Truncated)?;
    let mut parts = Vec::with_capacity(partitions);
    let mut at = sizes_at + sizes_len;
    for p in 0..partitions {
        let size = if p + 1 < partitions {
            let s = &sizes[3 * p..3 * p + 3];
            usize::from(s[0]) | usize::from(s[1]) << 8 | usize::from(s[2]) << 16
        } else {
            rest.len().checked_sub(at).ok_or(ImageError::Truncated)?
        };
        let part = rest.get(at..at + size).ok_or(ImageError::Truncated)?;
        parts.push(BoolDecoder::new(part));
        at += size;
    }

    // Quantizers and filter strengths per segment.
    let quant: [Quant; 4] = std::array::from_fn(|s| {
        let mut q = base_q;
        if h.segmentation {
            q = if h.absolute {
                h.seg_quant[s]
            } else {
                q + h.seg_quant[s]
            };
        }
        let idx = |delta: i32, max: i32| (q + delta).clamp(0, max) as usize;
        Quant {
            y1: [DC_QUANT[idx(y1_dc, 127)], AC_QUANT[idx(0, 127)]],
            y2: [
                DC_QUANT[idx(y2_dc, 127)] * 2,
                (AC_QUANT[idx(y2_ac, 127)] * 155 / 100).max(8),
            ],
            uv: [DC_QUANT[idx(uv_dc, 117)], AC_QUANT[idx(uv_ac, 127)]],
        }
    });
    let filters: [[FilterInfo; 2]; 4] = std::array::from_fn(|s| {
        let mut base = h.level;
        if h.segmentation {
            base = if h.absolute {
                h.seg_filter[s]
            } else {
                base + h.seg_filter[s]
            };
        }
        std::array::from_fn(|i4| {
            let mut level = base;
            if h.lf_delta {
                level += h.ref_delta0;
                if i4 == 1 {
                    level += h.mode_delta0;
                }
            }
            let level = level.clamp(0, 63);
            if level == 0 {
                return FilterInfo::default();
            }
            let mut ilevel = level;
            if h.sharpness > 0 {
                ilevel >>= if h.sharpness > 4 { 2 } else { 1 };
                ilevel = ilevel.min(9 - h.sharpness);
            }
            let ilevel = ilevel.max(1);
            FilterInfo {
                limit: 2 * level + ilevel,
                ilevel,
                hev: if level >= 40 {
                    2
                } else if level >= 15 {
                    1
                } else {
                    0
                },
                inner: i4 == 1,
            }
        })
    });

    let mut y_plane = vec![0u8; y_stride * mbh * 16];
    let mut u_plane = vec![0u8; uv_stride * mbh * 8];
    let mut v_plane = vec![0u8; uv_stride * mbh * 8];
    let mut info = vec![FilterInfo::default(); mbw * mbh];
    // Subblock modes of the row above (4 per macroblock) for contexts.
    let mut above_modes = vec![B_DC; mbw * 4];
    // Non-zero flags above: 4 Y, 2 U, 2 V, 1 Y2 per macroblock.
    let mut above_nz = vec![[false; 9]; mbw];
    for mby in 0..mbh {
        cancelled(cancellation)?;
        let mut left_modes = [B_DC; 4];
        let mut left_nz = [false; 9];
        let part = mby % partitions;
        for mbx in 0..mbw {
            let segment = if h.update_map {
                usize::from(d.tree(&SEGMENT_TREE, &h.seg_probs))
            } else {
                0
            };
            let skip = skip_prob.is_some_and(|p| d.bool(p));
            let ymode = d.tree(&KF_YMODE_TREE, &KF_YMODE_PROBS);
            let mut bmodes = [B_DC; 16];
            if ymode == B_PRED {
                for by in 0..4 {
                    for bx in 0..4 {
                        let above = if by == 0 {
                            above_modes[mbx * 4 + bx]
                        } else {
                            bmodes[(by - 1) * 4 + bx]
                        };
                        let left = if bx == 0 {
                            left_modes[by]
                        } else {
                            bmodes[by * 4 + bx - 1]
                        };
                        bmodes[by * 4 + bx] = d.tree(
                            &BMODE_TREE,
                            &KF_BMODE_PROBS[usize::from(above)][usize::from(left)],
                        );
                    }
                }
            } else {
                let implied = match ymode {
                    V_PRED => B_VE,
                    H_PRED => B_HE,
                    TM_PRED => B_TM,
                    _ => B_DC,
                };
                bmodes = [implied; 16];
            }
            for i in 0..4 {
                above_modes[mbx * 4 + i] = bmodes[12 + i];
                left_modes[i] = bmodes[i * 4 + 3];
            }
            let uvmode = d.tree(&UV_MODE_TREE, &KF_UV_MODE_PROBS);

            // Residual tokens.
            let q = quant[segment];
            let mut coeffs = [[0i32; 16]; 24];
            let mut nonzero = false;
            let anz = &mut above_nz[mbx];
            if skip {
                for i in 0..8 {
                    anz[i] = false;
                    left_nz[i] = false;
                }
                if ymode != B_PRED {
                    anz[8] = false;
                    left_nz[8] = false;
                }
            } else {
                let td = &mut parts[part];
                let mut first = 0;
                let mut y_plane_type = 3;
                if ymode != B_PRED {
                    let mut dc = [0i32; 16];
                    let ctx = usize::from(anz[8]) + usize::from(left_nz[8]);
                    let nz = read_coeffs(td, &probs[1], ctx, q.y2, 0, &mut dc);
                    anz[8] = nz > 0;
                    left_nz[8] = nz > 0;
                    let dcs = iwht(&dc);
                    for (i, &v) in dcs.iter().enumerate() {
                        coeffs[i][0] = v;
                        nonzero |= v != 0;
                    }
                    first = 1;
                    y_plane_type = 0;
                }
                for by in 0..4 {
                    for bx in 0..4 {
                        let ctx = usize::from(anz[bx]) + usize::from(left_nz[by]);
                        let block = &mut coeffs[by * 4 + bx];
                        let nz = read_coeffs(td, &probs[y_plane_type], ctx, q.y1, first, block);
                        let flag = nz > first;
                        anz[bx] = flag;
                        left_nz[by] = flag;
                        nonzero |= nz > 1 || block[0] != 0;
                    }
                }
                for plane in 0..2 {
                    for by in 0..2 {
                        for bx in 0..2 {
                            let a = 4 + plane * 2 + bx;
                            let l = 4 + plane * 2 + by;
                            let ctx = usize::from(anz[a]) + usize::from(left_nz[l]);
                            let block = &mut coeffs[16 + plane * 4 + by * 2 + bx];
                            let nz = read_coeffs(td, &probs[2], ctx, q.uv, 0, block);
                            anz[a] = nz > 0;
                            left_nz[l] = nz > 0;
                            nonzero |= nz > 1 || block[0] != 0;
                        }
                    }
                }
            }
            let mut fi = filters[segment][usize::from(ymode == B_PRED)];
            fi.inner |= nonzero;
            info[mby * mbw + mbx] = fi;

            reconstruct(
                &mut y_plane,
                &mut u_plane,
                &mut v_plane,
                (y_stride, uv_stride, mbw),
                (mbx, mby),
                (ymode, &bmodes, uvmode),
                &coeffs,
            );
        }
    }
    // A zero frame-level strength disables the filter, as in libvpx.
    if h.level > 0 {
        for mby in 0..mbh {
            cancelled(cancellation)?;
            for mbx in 0..mbw {
                let fi = info[mby * mbw + mbx];
                if fi.limit == 0 {
                    continue;
                }
                if h.simple_filter {
                    simple_filter(&mut y_plane, y_stride, mbx, mby, fi);
                } else {
                    normal_filter(&mut y_plane, y_stride, 16, mbx, mby, fi);
                    normal_filter(&mut u_plane, uv_stride, 8, mbx, mby, fi);
                    normal_filter(&mut v_plane, uv_stride, 8, mbx, mby, fi);
                }
            }
        }
    }
    Ok(Yuv {
        width,
        height,
        y: y_plane,
        u: u_plane,
        v: v_plane,
        y_stride,
        uv_stride,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "plane bundle kept flat for clarity"
)]
fn reconstruct(
    y_plane: &mut [u8],
    u_plane: &mut [u8],
    v_plane: &mut [u8],
    (y_stride, uv_stride, mbw): (usize, usize, usize),
    (mbx, mby): (usize, usize),
    (ymode, bmodes, uvmode): (u8, &[u8; 16], u8),
    coeffs: &[[i32; 16]; 24],
) {
    // Luma work area: 17 rows × 21 columns.
    let mut w = Work {
        data: [0; 21 * 17],
        stride: 21,
    };
    let (x0, y0) = (mbx * 16, mby * 16);
    for i in 0..20isize {
        let v = if mby == 0 {
            127
        } else if i < 16 || mbx + 1 < mbw {
            i32::from(y_plane[(y0 - 1) * y_stride + x0 + i as usize])
        } else {
            // Above-right beyond the frame repeats the last above sample.
            i32::from(y_plane[(y0 - 1) * y_stride + x0 + 15])
        };
        w.data[1 + i as usize] = v as u8;
    }
    w.data[0] = if mby == 0 {
        127
    } else if mbx == 0 {
        129
    } else {
        y_plane[(y0 - 1) * y_stride + x0 - 1]
    };
    for y in 0..16 {
        w.data[(y + 1) * 21] = if mbx == 0 {
            129
        } else {
            y_plane[(y0 + y) * y_stride + x0 - 1]
        };
    }
    if ymode == B_PRED {
        // Subblocks on the right edge reuse the macroblock's above-right.
        for row in [4, 8, 12] {
            for i in 0..4 {
                w.data[row * 21 + 17 + i] = w.data[17 + i];
            }
        }
        for by in 0..4 {
            for bx in 0..4 {
                predict_sub(&mut w, bx, by, bmodes[by * 4 + bx]);
                let at = (by * 4 + 1) * 21 + bx * 4 + 1;
                idct_add(&coeffs[by * 4 + bx], &mut w.data, at, 21);
            }
        }
    } else {
        predict_block(&mut w, 16, ymode, mby > 0, mbx > 0);
        for by in 0..4 {
            for bx in 0..4 {
                let at = (by * 4 + 1) * 21 + bx * 4 + 1;
                idct_add(&coeffs[by * 4 + bx], &mut w.data, at, 21);
            }
        }
    }
    for y in 0..16 {
        let row = &w.data[(y + 1) * 21 + 1..(y + 1) * 21 + 17];
        y_plane[(y0 + y) * y_stride + x0..][..16].copy_from_slice(row);
    }
    let (cx, cy) = (mbx * 8, mby * 8);
    for (plane, base) in [(u_plane, 16), (v_plane, 20)] {
        let mut w = Work {
            data: [0; 21 * 17],
            stride: 9,
        };
        for i in 0..8 {
            w.data[1 + i] = if mby == 0 {
                127
            } else {
                plane[(cy - 1) * uv_stride + cx + i]
            };
        }
        w.data[0] = if mby == 0 {
            127
        } else if mbx == 0 {
            129
        } else {
            plane[(cy - 1) * uv_stride + cx - 1]
        };
        for y in 0..8 {
            w.data[(y + 1) * 9] = if mbx == 0 {
                129
            } else {
                plane[(cy + y) * uv_stride + cx - 1]
            };
        }
        predict_block(&mut w, 8, uvmode, mby > 0, mbx > 0);
        for by in 0..2 {
            for bx in 0..2 {
                let at = (by * 4 + 1) * 9 + bx * 4 + 1;
                idct_add(&coeffs[base + by * 2 + bx], &mut w.data, at, 9);
            }
        }
        for y in 0..8 {
            plane[(cy + y) * uv_stride + cx..][..8]
                .copy_from_slice(&w.data[(y + 1) * 9 + 1..(y + 1) * 9 + 9]);
        }
    }
}

fn sclip1(v: i32) -> i32 {
    v.clamp(-128, 127)
}
fn sclip2(v: i32) -> i32 {
    v.clamp(-16, 15)
}
fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The pixels across one edge: `p[at + k·step]` for k in −4..4.
struct Edge<'a> {
    p: &'a mut [u8],
    at: usize,
    step: usize,
}

impl Edge<'_> {
    fn get(&self, k: isize) -> i32 {
        i32::from(self.p[(self.at as isize + k * self.step as isize) as usize])
    }
    fn put(&mut self, k: isize, v: i32) {
        self.p[(self.at as isize + k * self.step as isize) as usize] = clip1(v);
    }
    fn filter2(&mut self) {
        let (p1, p0, q0, q1) = (self.get(-2), self.get(-1), self.get(0), self.get(1));
        let a = 3 * (q0 - p0) + sclip1(p1 - q1);
        let a1 = sclip2((a + 4) >> 3);
        let a2 = sclip2((a + 3) >> 3);
        self.put(-1, p0 + a2);
        self.put(0, q0 - a1);
    }
    fn filter4(&mut self) {
        let (p1, p0, q0, q1) = (self.get(-2), self.get(-1), self.get(0), self.get(1));
        let a = 3 * (q0 - p0);
        let a1 = sclip2((a + 4) >> 3);
        let a2 = sclip2((a + 3) >> 3);
        let a3 = (a1 + 1) >> 1;
        self.put(-2, p1 + a3);
        self.put(-1, p0 + a2);
        self.put(0, q0 - a1);
        self.put(1, q1 - a3);
    }
    fn filter6(&mut self) {
        let (p2, p1, p0) = (self.get(-3), self.get(-2), self.get(-1));
        let (q0, q1, q2) = (self.get(0), self.get(1), self.get(2));
        let a = sclip1(3 * (q0 - p0) + sclip1(p1 - q1));
        let a1 = (27 * a + 63) >> 7;
        let a2 = (18 * a + 63) >> 7;
        let a3 = (9 * a + 63) >> 7;
        self.put(-3, p2 + a3);
        self.put(-2, p1 + a2);
        self.put(-1, p0 + a1);
        self.put(0, q0 - a1);
        self.put(1, q1 - a2);
        self.put(2, q2 - a3);
    }
    fn hev(&self, thresh: i32) -> bool {
        (self.get(-2) - self.get(-1)).abs() > thresh || (self.get(1) - self.get(0)).abs() > thresh
    }
    fn needs(&self, t: i32) -> bool {
        4 * (self.get(-1) - self.get(0)).abs() + (self.get(-2) - self.get(1)).abs() <= t
    }
    fn needs2(&self, t: i32, it: i32) -> bool {
        if !self.needs(t) {
            return false;
        }
        let g = |k| self.get(k);
        (g(-4) - g(-3)).abs() <= it
            && (g(-3) - g(-2)).abs() <= it
            && (g(-2) - g(-1)).abs() <= it
            && (g(3) - g(2)).abs() <= it
            && (g(2) - g(1)).abs() <= it
            && (g(1) - g(0)).abs() <= it
    }
}

/// Filter `count` pixels along an edge starting at `at`; `step` crosses
/// the edge and `along` moves along it.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the filter loop parameters"
)]
fn filter_loop(
    p: &mut [u8],
    at: usize,
    step: usize,
    along: usize,
    count: usize,
    limit: i32,
    ilevel: i32,
    hev: i32,
    macroblock_edge: bool,
) {
    let t = 2 * limit + 1;
    for i in 0..count {
        let mut e = Edge {
            p: &mut *p,
            at: at + i * along,
            step,
        };
        if e.needs2(t, ilevel) {
            if e.hev(hev) {
                e.filter2();
            } else if macroblock_edge {
                e.filter6();
            } else {
                e.filter4();
            }
        }
    }
}

fn normal_filter(p: &mut [u8], stride: usize, size: usize, mbx: usize, mby: usize, fi: FilterInfo) {
    let origin = mby * size * stride + mbx * size;
    let (limit, il, hev) = (fi.limit, fi.ilevel, fi.hev);
    if mbx > 0 {
        filter_loop(p, origin, 1, stride, size, limit + 4, il, hev, true);
    }
    if fi.inner {
        for k in (4..size).step_by(4) {
            filter_loop(p, origin + k, 1, stride, size, limit, il, hev, false);
        }
    }
    if mby > 0 {
        filter_loop(p, origin, stride, 1, size, limit + 4, il, hev, true);
    }
    if fi.inner {
        for k in (4..size).step_by(4) {
            filter_loop(
                p,
                origin + k * stride,
                stride,
                1,
                size,
                limit,
                il,
                hev,
                false,
            );
        }
    }
}

fn simple_edge(p: &mut [u8], at: usize, step: usize, along: usize, thresh: i32) {
    let t = 2 * thresh + 1;
    for i in 0..16 {
        let mut e = Edge {
            p: &mut *p,
            at: at + i * along,
            step,
        };
        if e.needs(t) {
            e.filter2();
        }
    }
}

fn simple_filter(p: &mut [u8], stride: usize, mbx: usize, mby: usize, fi: FilterInfo) {
    let origin = mby * 16 * stride + mbx * 16;
    if mbx > 0 {
        simple_edge(p, origin, 1, stride, fi.limit + 4);
    }
    if fi.inner {
        for k in [4, 8, 12] {
            simple_edge(p, origin + k, 1, stride, fi.limit);
        }
    }
    if mby > 0 {
        simple_edge(p, origin, stride, 1, fi.limit + 4);
    }
    if fi.inner {
        for k in [4, 8, 12] {
            simple_edge(p, origin + k * stride, stride, 1, fi.limit);
        }
    }
}

/// libwebp's 14-bit fixed-point BT.601 limited-range conversion.
fn yuv_to_rgb(y: i32, u: i32, v: i32) -> [u8; 3] {
    let mult = |v: i32, c: i32| (v * c) >> 8;
    let clip = |v: i32| -> u8 {
        if (0..256 << 6).contains(&v) {
            (v >> 6) as u8
        } else if v < 0 {
            0
        } else {
            255
        }
    };
    [
        clip(mult(y, 19077) + mult(v, 26149) - 14234),
        clip(mult(y, 19077) - mult(u, 6419) - mult(v, 13320) + 8708),
        clip(mult(y, 19077) + mult(u, 33050) - 17685),
    ]
}

impl Yuv {
    /// Convert to RGBA (alpha 255) with the "fancy" bilinear chroma
    /// upsampling browsers use (libwebp's default).
    pub(crate) fn to_rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width, self.height);
        let uvw = w.div_ceil(2);
        let uvh = h.div_ceil(2);
        let mut out = vec![255u8; w * h * 4];
        let mut us = vec![0i32; w];
        let mut vs = vec![0i32; w];
        for y in 0..h {
            let near = y / 2;
            let far = if y % 2 == 1 {
                (near + 1).min(uvh - 1)
            } else {
                near.saturating_sub(1)
            };
            for (plane, dst) in [(&self.u, &mut us), (&self.v, &mut vs)] {
                let n = &plane[near * self.uv_stride..][..uvw];
                let f = &plane[far * self.uv_stride..][..uvw];
                upsample_line(n, f, w, dst);
            }
            for x in 0..w {
                let luma = i32::from(self.y[y * self.y_stride + x]);
                let rgb = yuv_to_rgb(luma, us[x], vs[x]);
                out[(y * w + x) * 4..][..3].copy_from_slice(&rgb);
            }
        }
        out
    }
}

/// One output row of chroma, weighting the `near` row 3:1 against `far`
/// with libwebp's diagonal rounding.
fn upsample_line(near: &[u8], far: &[u8], len: usize, out: &mut [i32]) {
    let n = |i: usize| i32::from(near[i]);
    let f = |i: usize| i32::from(far[i]);
    out[0] = (3 * n(0) + f(0) + 2) >> 2;
    let last_pair = (len - 1) >> 1;
    for x in 1..=last_pair {
        let avg = n(x - 1) + n(x) + f(x - 1) + f(x) + 8;
        let diag_12 = (avg + 2 * (n(x) + f(x - 1))) >> 3;
        let diag_03 = (avg + 2 * (n(x - 1) + f(x))) >> 3;
        out[2 * x - 1] = (diag_12 + n(x - 1)) >> 1;
        out[2 * x] = (diag_03 + n(x)) >> 1;
    }
    if len % 2 == 0 {
        let i = last_pair;
        out[len - 1] = (3 * n(i) + f(i) + 2) >> 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_transforms_follow_the_reference() {
        let mut dc = [0i32; 16];
        dc[0] = 80;
        // A DC-only WHT spreads (80 + 3) >> 3 to every block.
        assert_eq!(iwht(&dc), [10; 16]);
        let mut block = [0i32; 16];
        block[0] = 80;
        let mut dst = [100u8; 16];
        idct_add(&block, &mut dst, 0, 4);
        assert_eq!(dst, [110; 16]);
    }

    #[test]
    fn yuv_conversion_matches_bt601_limited_range() {
        assert_eq!(yuv_to_rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(yuv_to_rgb(235, 128, 128), [255, 255, 255]);
        let [r, g, b] = yuv_to_rgb(81, 90, 240);
        assert!(r > 250 && g < 5 && b < 5, "{r} {g} {b}");
    }

    #[test]
    fn bool_decoder_reads_literals() {
        // A stream of 0xff bytes decodes ones at probability 128.
        let mut d = BoolDecoder::new(&[0xff; 8]);
        assert_eq!(d.literal(4), 15);
        let mut d = BoolDecoder::new(&[0; 8]);
        assert_eq!(d.literal(7), 0);
    }
}
