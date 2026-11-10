//! Bounded JPEG decoding (ITU-T T.81 / ISO/IEC 10918-1 with JFIF and Adobe
//! conventions) to straight-alpha RGBA8.
//!
//! Supports Huffman-coded baseline, extended (8-bit) sequential and
//! progressive frames with 1, 3 or 4 components, any sampling factors from
//! 1 to 4, restart intervals, 8- and 16-bit quantization tables, YCbCr, RGB,
//! CMYK and YCCK color, and EXIF orientation (see [`decode_jpeg`]).
//! Arithmetic coding, lossless and hierarchical frames, 12-bit samples and
//! `DNL` are rejected as [`ImageError::Unsupported`] (TD-80).
//!
//! Entropy-coded data that ends early decodes as if padded with zero bits,
//! as browsers do, so a truncated file still shows its decoded part once at
//! least one scan header was read. Work is bounded by a per-image budget of
//! block visits across scans.

use crate::image::{DecodeLimits, ImageError, cancelled};
use crate::png::DecodedImage;
use rustic_base::CancellationToken;

/// Zig-zag order: `ZIGZAG[k]` is the natural index of the k-th coefficient.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Maximum scans per image and block visits per block, bounding the work
/// a small file can demand.
const MAX_SCANS: usize = 1000;
const VISITS_PER_BLOCK: usize = 48;

#[derive(Clone)]
struct Huffman {
    /// `(length, value)` for each 9-bit prefix; length 0 means slow path.
    fast: Vec<(u8, u8)>,
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    values: Vec<u8>,
}

const FAST_BITS: u8 = 9;

impl Huffman {
    fn new(counts: &[u8; 16], values: &[u8]) -> Result<Self, ImageError> {
        let mut table = Self {
            fast: vec![(0, 0); 1 << FAST_BITS],
            maxcode: [-1; 18],
            valptr: [0; 17],
            mincode: [0; 17],
            values: values.to_vec(),
        };
        let mut code = 0i32;
        let mut k = 0usize;
        for len in 1..=16usize {
            let n = usize::from(counts[len - 1]);
            table.valptr[len] = k as i32;
            table.mincode[len] = code;
            // A complete code may not exceed `len` bits.
            if code + n as i32 > 1 << len {
                return Err(ImageError::Invalid);
            }
            for _ in 0..n {
                if len <= usize::from(FAST_BITS) {
                    let shift = usize::from(FAST_BITS) - len;
                    let base = (code as usize) << shift;
                    for fill in 0..(1usize << shift) {
                        table.fast[base + fill] = (len as u8, values[k]);
                    }
                }
                code += 1;
                k += 1;
            }
            if n > 0 {
                table.maxcode[len] = code - 1;
            }
            code <<= 1;
        }
        table.maxcode[17] = i32::MAX;
        Ok(table)
    }
}

/// Entropy-coded segment reader with byte unstuffing. Markers and the end
/// of data read as zero bits.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    count: u8,
}

impl Bits<'_> {
    fn fill(&mut self) {
        while self.count <= 24 {
            let byte = match self.data.get(self.pos) {
                Some(0xff) => match self.data.get(self.pos + 1) {
                    Some(0) => {
                        self.pos += 2;
                        0xff
                    }
                    _ => 0,
                },
                Some(&b) => {
                    self.pos += 1;
                    b
                }
                None => 0,
            };
            self.acc |= u32::from(byte) << (24 - self.count);
            self.count += 8;
        }
    }

    fn bits(&mut self, n: u8) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.count < n {
            self.fill();
        }
        let value = self.acc >> (32 - n);
        self.acc <<= n;
        self.count -= n;
        value
    }

    fn bit(&mut self) -> bool {
        self.bits(1) == 1
    }

    /// Read `s` bits and sign-extend them (T.81 F.2.2.1 EXTEND).
    fn receive_extend(&mut self, s: u8) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < 1 << (s - 1) {
            v - (1 << s) + 1
        } else {
            v
        }
    }

    fn decode(&mut self, table: &Huffman) -> Result<u8, ImageError> {
        if self.count < 16 {
            self.fill();
        }
        let (len, value) = table.fast[(self.acc >> (32 - FAST_BITS)) as usize];
        if len > 0 {
            self.acc <<= len;
            self.count -= len;
            return Ok(value);
        }
        let mut code = (self.acc >> (32 - FAST_BITS)) as i32;
        let mut len = FAST_BITS as usize;
        self.acc <<= FAST_BITS;
        self.count -= FAST_BITS;
        while code > table.maxcode[len] {
            if len == 16 {
                return Err(ImageError::Invalid);
            }
            code = (code << 1) | self.bits(1) as i32;
            len += 1;
        }
        let index = table.valptr[len] + code - table.mincode[len];
        table
            .values
            .get(index as usize)
            .copied()
            .ok_or(ImageError::Invalid)
    }

    /// Discard buffered bits and skip an `RSTn` marker if one is next.
    fn restart(&mut self) {
        self.acc = 0;
        self.count = 0;
        if self.data.get(self.pos) == Some(&0xff)
            && self
                .data
                .get(self.pos + 1)
                .is_some_and(|m| (0xd0..=0xd7).contains(m))
        {
            self.pos += 2;
        }
    }
}

#[derive(Clone)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Quantization table latched at the component's first scan.
    quant: Option<[u16; 64]>,
    /// Blocks per line and block rows, padded to whole MCUs.
    bw: usize,
    bh: usize,
    /// Blocks covering the component's own samples.
    used_w: usize,
    used_h: usize,
    coefs: Vec<i16>,
    dc_pred: i32,
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    components: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
}

struct Scan {
    comps: Vec<(usize, usize, usize)>,
    ss: usize,
    se: usize,
    ah: u8,
    al: u8,
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    quant: [Option<[u16; 64]>; 4],
    dc: [Option<Huffman>; 4],
    ac: [Option<Huffman>; 4],
    restart: usize,
    adobe: Option<u8>,
    jfif: bool,
    orientation: u8,
    frame: Option<Frame>,
    scans: usize,
    visits: usize,
    max_visits: usize,
    eobrun: u32,
}

fn be16(data: &[u8], at: usize) -> Result<usize, ImageError> {
    match data.get(at..at + 2) {
        Some(b) => Ok(usize::from(b[0]) << 8 | usize::from(b[1])),
        None => Err(ImageError::Truncated),
    }
}

impl<'a> Decoder<'a> {
    /// The payload of the marker segment at `self.pos` (after its length).
    fn segment(&mut self) -> Result<&'a [u8], ImageError> {
        let len = be16(self.data, self.pos)?;
        if len < 2 {
            return Err(ImageError::Invalid);
        }
        let body = self
            .data
            .get(self.pos + 2..self.pos + len)
            .ok_or(ImageError::Truncated)?;
        self.pos += len;
        Ok(body)
    }

    fn dqt(&mut self, mut body: &[u8]) -> Result<(), ImageError> {
        while let Some((&pq_tq, rest)) = body.split_first() {
            let (precision, id) = (pq_tq >> 4, usize::from(pq_tq & 15));
            if id > 3 || precision > 1 {
                return Err(ImageError::Invalid);
            }
            let size = if precision == 0 { 64 } else { 128 };
            let values = rest.get(..size).ok_or(ImageError::Invalid)?;
            let mut table = [0u16; 64];
            for (k, slot) in ZIGZAG.iter().enumerate() {
                table[*slot] = if precision == 0 {
                    u16::from(values[k])
                } else {
                    u16::from(values[2 * k]) << 8 | u16::from(values[2 * k + 1])
                };
            }
            self.quant[id] = Some(table);
            body = &rest[size..];
        }
        Ok(())
    }

    fn dht(&mut self, mut body: &[u8]) -> Result<(), ImageError> {
        while let Some((&tc_th, rest)) = body.split_first() {
            let (class, id) = (tc_th >> 4, usize::from(tc_th & 15));
            if class > 1 || id > 3 {
                return Err(ImageError::Invalid);
            }
            let counts: [u8; 16] = rest
                .get(..16)
                .and_then(|c| c.try_into().ok())
                .ok_or(ImageError::Invalid)?;
            let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
            if total > 256 {
                return Err(ImageError::Invalid);
            }
            let values = rest.get(16..16 + total).ok_or(ImageError::Invalid)?;
            let table = Huffman::new(&counts, values)?;
            if class == 0 {
                self.dc[id] = Some(table);
            } else {
                self.ac[id] = Some(table);
            }
            body = &rest[16 + total..];
        }
        Ok(())
    }

    fn sof(
        &mut self,
        body: &[u8],
        progressive: bool,
        limits: &DecodeLimits,
    ) -> Result<(), ImageError> {
        if self.frame.is_some() {
            return Err(ImageError::Invalid);
        }
        if body.len() < 6 {
            return Err(ImageError::Invalid);
        }
        if body[0] != 8 {
            return Err(ImageError::Unsupported);
        }
        let height = be16(body, 1)?;
        let width = be16(body, 3)?;
        if height == 0 {
            // Height defined by a later DNL marker.
            return Err(ImageError::Unsupported);
        }
        let output = limits.check(width as u32, height as u32)?;
        let n = usize::from(body[5]);
        if !matches!(n, 1 | 3 | 4) || body.len() < 6 + 3 * n {
            return Err(if n == 2 || n > 4 {
                ImageError::Unsupported
            } else {
                ImageError::Invalid
            });
        }
        let mut components = Vec::with_capacity(n);
        for c in 0..n {
            let at = 6 + 3 * c;
            let (h, v) = (
                usize::from(body[at + 1] >> 4),
                usize::from(body[at + 1] & 15),
            );
            let tq = usize::from(body[at + 2]);
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) || tq > 3 {
                return Err(ImageError::Invalid);
            }
            if components.iter().any(|x: &Component| x.id == body[at]) {
                return Err(ImageError::Invalid);
            }
            components.push(Component {
                id: body[at],
                h,
                v,
                tq,
                quant: None,
                bw: 0,
                bh: 0,
                used_w: 0,
                used_h: 0,
                coefs: Vec::new(),
                dc_pred: 0,
            });
        }
        let hmax = components.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = components.iter().map(|c| c.v).max().unwrap_or(1);
        let mcux = width.div_ceil(8 * hmax);
        let mcuy = height.div_ceil(8 * vmax);
        // Coefficients (2 bytes each) and sample planes (1 byte each) for
        // every component, plus the RGBA output, must fit the budget.
        let mut bytes = output;
        let mut blocks = 0usize;
        for c in &mut components {
            c.bw = mcux * c.h;
            c.bh = mcuy * c.v;
            c.used_w = (width * c.h).div_ceil(hmax).div_ceil(8);
            c.used_h = (height * c.v).div_ceil(vmax).div_ceil(8);
            let count = c.bw.checked_mul(c.bh).ok_or(ImageError::Limit)?;
            blocks = blocks.checked_add(count).ok_or(ImageError::Limit)?;
            bytes = count
                .checked_mul(64 * 3)
                .and_then(|b| b.checked_add(bytes))
                .ok_or(ImageError::Limit)?;
        }
        if bytes > limits.max_bytes {
            return Err(ImageError::Limit);
        }
        for c in &mut components {
            c.coefs = vec![0; c.bw * c.bh * 64];
        }
        self.max_visits = blocks.saturating_mul(VISITS_PER_BLOCK);
        self.frame = Some(Frame {
            width,
            height,
            progressive,
            components,
            hmax,
            vmax,
            mcux,
            mcuy,
        });
        Ok(())
    }

    fn sos(&mut self, body: &[u8]) -> Result<Scan, ImageError> {
        let frame = self.frame.as_mut().ok_or(ImageError::Invalid)?;
        let n = usize::from(*body.first().ok_or(ImageError::Invalid)?);
        if !(1..=4).contains(&n) || body.len() < 4 + 2 * n {
            return Err(ImageError::Invalid);
        }
        let mut comps = Vec::with_capacity(n);
        for i in 0..n {
            let id = body[1 + 2 * i];
            let tables = body[2 + 2 * i];
            let index = frame
                .components
                .iter()
                .position(|c| c.id == id)
                .ok_or(ImageError::Invalid)?;
            if comps.iter().any(|&(c, _, _)| c == index) {
                return Err(ImageError::Invalid);
            }
            let (td, ta) = (usize::from(tables >> 4), usize::from(tables & 15));
            if td > 3 || ta > 3 {
                return Err(ImageError::Invalid);
            }
            comps.push((index, td, ta));
        }
        let at = 1 + 2 * n;
        let (ss, se) = (usize::from(body[at]), usize::from(body[at + 1]));
        let (ah, al) = (body[at + 2] >> 4, body[at + 2] & 15);
        if frame.progressive {
            let dc = ss == 0;
            if se > 63 || ss > se || (dc && se != 0) || (!dc && n != 1) || al > 13 || ah > 13 {
                return Err(ImageError::Invalid);
            }
        } else if ss != 0 || se != 63 || ah != 0 || al != 0 {
            return Err(ImageError::Invalid);
        }
        let blocks_per_mcu: usize = comps
            .iter()
            .map(|&(c, _, _)| frame.components[c].h * frame.components[c].v)
            .sum();
        if n > 1 && blocks_per_mcu > 10 {
            return Err(ImageError::Invalid);
        }
        for &(c, _, _) in &comps {
            let component = &mut frame.components[c];
            if component.quant.is_none() {
                component.quant = Some(self.quant[component.tq].ok_or(ImageError::Invalid)?);
            }
        }
        Ok(Scan {
            comps,
            ss,
            se,
            ah,
            al,
        })
    }

    fn scan(&mut self, scan: &Scan, cancellation: &CancellationToken) -> Result<(), ImageError> {
        self.scans += 1;
        if self.scans > MAX_SCANS {
            return Err(ImageError::Limit);
        }
        let mut frame = self.frame.take().ok_or(ImageError::Invalid)?;
        let result = self.scan_frame(&mut frame, scan, cancellation);
        self.frame = Some(frame);
        result
    }

    fn scan_frame(
        &mut self,
        frame: &mut Frame,
        scan: &Scan,
        cancellation: &CancellationToken,
    ) -> Result<(), ImageError> {
        let refine_dc = frame.progressive && scan.ss == 0 && scan.ah > 0;
        let ac_only = frame.progressive && scan.ss > 0;
        // Look up the tables this scan needs.
        let mut tables = Vec::with_capacity(scan.comps.len());
        for &(_, td, ta) in &scan.comps {
            let dc = if scan.ss == 0 && !refine_dc {
                Some(self.dc[td].clone().ok_or(ImageError::Invalid)?)
            } else {
                None
            };
            let ac = if !frame.progressive || ac_only {
                Some(self.ac[ta].clone().ok_or(ImageError::Invalid)?)
            } else {
                None
            };
            tables.push((dc, ac));
        }
        let mut bits = Bits {
            data: self.data,
            pos: self.pos,
            acc: 0,
            count: 0,
        };
        for c in &mut frame.components {
            c.dc_pred = 0;
        }
        self.eobrun = 0;
        let single = scan.comps.len() == 1;
        let (units_x, units_y) = if single {
            let c = &frame.components[scan.comps[0].0];
            (c.used_w, c.used_h)
        } else {
            (frame.mcux, frame.mcuy)
        };
        let per_unit: usize = if single {
            1
        } else {
            scan.comps
                .iter()
                .map(|&(c, _, _)| frame.components[c].h * frame.components[c].v)
                .sum()
        };
        self.visits = self
            .visits
            .saturating_add(units_x.saturating_mul(units_y).saturating_mul(per_unit));
        if self.visits > self.max_visits {
            return Err(ImageError::Limit);
        }
        let mut until_restart = self.restart;
        for uy in 0..units_y {
            cancelled(cancellation)?;
            for ux in 0..units_x {
                if self.restart > 0 {
                    if until_restart == 0 {
                        bits.restart();
                        for c in &mut frame.components {
                            c.dc_pred = 0;
                        }
                        self.eobrun = 0;
                        until_restart = self.restart;
                    }
                    until_restart -= 1;
                }
                for (i, &(ci, _, _)) in scan.comps.iter().enumerate() {
                    let component = &mut frame.components[ci];
                    let (bh_n, bv_n) = if single {
                        (1, 1)
                    } else {
                        (component.h, component.v)
                    };
                    for by in 0..bv_n {
                        for bx in 0..bh_n {
                            let (x, y) = if single {
                                (ux, uy)
                            } else {
                                (ux * component.h + bx, uy * component.v + by)
                            };
                            let at = (y * component.bw + x) * 64;
                            let (dc, ac) = &tables[i];
                            let block = &mut component.coefs[at..at + 64];
                            if !frame.progressive {
                                decode_baseline(&mut bits, block, &mut component.dc_pred, dc, ac)?;
                            } else if scan.ss == 0 {
                                if refine_dc {
                                    if bits.bit() {
                                        block[0] |= 1 << scan.al;
                                    }
                                } else {
                                    let table = dc.as_ref().ok_or(ImageError::Invalid)?;
                                    let s = bits.decode(table)?;
                                    if s > 11 {
                                        return Err(ImageError::Invalid);
                                    }
                                    component.dc_pred =
                                        add_dc(component.dc_pred, bits.receive_extend(s));
                                    block[0] = clamp16(component.dc_pred << scan.al);
                                }
                            } else {
                                let table = ac.as_ref().ok_or(ImageError::Invalid)?;
                                if scan.ah == 0 {
                                    ac_first(&mut bits, block, table, scan, &mut self.eobrun)?;
                                } else {
                                    ac_refine(&mut bits, block, table, scan, &mut self.eobrun)?;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Resume marker parsing at the next marker after the entropy data.
        let mut pos = bits.pos;
        while pos + 1 < self.data.len() {
            if self.data[pos] == 0xff {
                let m = self.data[pos + 1];
                if m != 0 && m != 0xff && !(0xd0..=0xd7).contains(&m) {
                    break;
                }
            }
            pos += 1;
        }
        self.pos = if pos + 1 < self.data.len() {
            pos
        } else {
            self.data.len()
        };
        Ok(())
    }
}

/// Add a DC difference, keeping corrupt streams from overflowing.
fn add_dc(pred: i32, diff: i32) -> i32 {
    (pred + diff).clamp(-(1 << 16), 1 << 16)
}

fn clamp16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn decode_baseline(
    bits: &mut Bits<'_>,
    block: &mut [i16],
    pred: &mut i32,
    dc: &Option<Huffman>,
    ac: &Option<Huffman>,
) -> Result<(), ImageError> {
    let (dc, ac) = (
        dc.as_ref().ok_or(ImageError::Invalid)?,
        ac.as_ref().ok_or(ImageError::Invalid)?,
    );
    let s = bits.decode(dc)?;
    if s > 11 {
        return Err(ImageError::Invalid);
    }
    *pred = add_dc(*pred, bits.receive_extend(s));
    block[0] = clamp16(*pred);
    let mut k = 1;
    while k < 64 {
        let rs = bits.decode(ac)?;
        let (r, s) = (usize::from(rs >> 4), rs & 15);
        if s == 0 {
            if r == 15 {
                k += 16;
                continue;
            }
            break;
        }
        k += r;
        if k > 63 {
            return Err(ImageError::Invalid);
        }
        block[ZIGZAG[k]] = clamp16(bits.receive_extend(s));
        k += 1;
    }
    Ok(())
}

fn ac_first(
    bits: &mut Bits<'_>,
    block: &mut [i16],
    table: &Huffman,
    scan: &Scan,
    eobrun: &mut u32,
) -> Result<(), ImageError> {
    if *eobrun > 0 {
        *eobrun -= 1;
        return Ok(());
    }
    let mut k = scan.ss;
    while k <= scan.se {
        let rs = bits.decode(table)?;
        let (r, s) = (u32::from(rs >> 4), rs & 15);
        if s == 0 {
            if r < 15 {
                *eobrun = (1 << r) - 1;
                if r > 0 {
                    *eobrun += bits.bits(r as u8);
                }
                break;
            }
            k += 16;
            continue;
        }
        k += r as usize;
        if k > scan.se {
            return Err(ImageError::Invalid);
        }
        block[ZIGZAG[k]] = clamp16(bits.receive_extend(s) * (1 << scan.al));
        k += 1;
    }
    Ok(())
}

fn ac_refine(
    bits: &mut Bits<'_>,
    block: &mut [i16],
    table: &Huffman,
    scan: &Scan,
    eobrun: &mut u32,
) -> Result<(), ImageError> {
    let p1 = 1i16 << scan.al;
    let m1 = -p1;
    let mut k = scan.ss;
    let refine = |bits: &mut Bits<'_>, coef: &mut i16| {
        if bits.bit() && *coef & p1 == 0 {
            *coef = if *coef >= 0 {
                coef.saturating_add(p1)
            } else {
                coef.saturating_add(m1)
            };
        }
    };
    if *eobrun == 0 {
        while k <= scan.se {
            let rs = bits.decode(table)?;
            let (mut r, s) = (u32::from(rs >> 4), rs & 15);
            let mut value = 0i16;
            if s != 0 {
                if s != 1 {
                    return Err(ImageError::Invalid);
                }
                value = if bits.bit() { p1 } else { m1 };
            } else if r != 15 {
                *eobrun = 1 << r;
                if r > 0 {
                    *eobrun += bits.bits(r as u8);
                }
                break;
            }
            while k <= scan.se {
                let coef = &mut block[ZIGZAG[k]];
                if *coef != 0 {
                    refine(bits, coef);
                } else {
                    if r == 0 {
                        break;
                    }
                    r -= 1;
                }
                k += 1;
            }
            if value != 0 {
                if k > scan.se {
                    return Err(ImageError::Invalid);
                }
                block[ZIGZAG[k]] = value;
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        while k <= scan.se {
            let coef = &mut block[ZIGZAG[k]];
            if *coef != 0 {
                refine(bits, coef);
            }
            k += 1;
        }
        *eobrun -= 1;
    }
    Ok(())
}

/// `cos(k·π/16)` scaled by 2^15 for k = 0..=8.
const COS: [i64; 9] = [32768, 32138, 30274, 27246, 23170, 18205, 12540, 6393, 0];

const fn cos16(k: usize) -> i64 {
    let k = k % 32;
    if k <= 8 {
        COS[k]
    } else if k <= 16 {
        -COS[16 - k]
    } else if k <= 24 {
        -COS[k - 16]
    } else {
        COS[32 - k]
    }
}

/// `IDCT[x][u] = C(u)·cos((2x+1)uπ/16)` scaled by 2^15, with C(0) = 1/√2.
const IDCT: [[i64; 8]; 8] = {
    let mut t = [[0i64; 8]; 8];
    let mut x = 0;
    while x < 8 {
        let mut u = 0;
        while u < 8 {
            t[x][u] = if u == 0 {
                COS[4]
            } else {
                cos16((2 * x + 1) * u)
            };
            u += 1;
        }
        x += 1;
    }
    t
};

/// Dequantize and inverse-transform one block into 8×8 samples
/// (T.81 A.3.3), in exact integer arithmetic so every platform matches.
fn idct(block: &[i16], quant: &[u16; 64], out: &mut [u8], stride: usize) {
    let mut coef = [0i64; 64];
    for i in 0..64 {
        coef[i] = i64::from(block[i]) * i64::from(quant[i]);
    }
    // Columns: tmp[y][u] = Σ_v IDCT[y][v]·F(v, u), keeping 2^15 scale.
    let mut tmp = [0i64; 64];
    for u in 0..8 {
        for y in 0..8 {
            let mut sum = 0;
            for v in 0..8 {
                sum += IDCT[y][v] * coef[v * 8 + u];
            }
            tmp[y * 8 + u] = sum;
        }
    }
    // Rows, then remove 2^30 scale and the ¼ factor: shift by 32.
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0;
            for u in 0..8 {
                sum += IDCT[x][u] * tmp[y * 8 + u];
            }
            let value = ((sum + (1 << 31)) >> 32) + 128;
            out[y * stride + x] = value.clamp(0, 255) as u8;
        }
    }
}

/// One component's samples at its own resolution.
struct Plane {
    w: usize,
    h: usize,
    data: Vec<u8>,
}

/// Upsample by integer factors. 2× horizontal and/or vertical use the
/// triangle ("fancy") filter of the IJG library and browsers; other
/// factors replicate samples.
fn upsample(plane: &Plane, fx: usize, fy: usize, width: usize, height: usize) -> Vec<u8> {
    let (pw, ph) = (plane.w, plane.h);
    let at = |x: usize, y: usize| i32::from(plane.data[y.min(ph - 1) * pw + x.min(pw - 1)]);
    let mut out = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let v = if fx == 2 && fy == 2 {
                let (sx, sy) = (x / 2, y / 2);
                let ny = if y % 2 == 0 {
                    sy.saturating_sub(1)
                } else {
                    sy + 1
                };
                let nx = if x % 2 == 0 {
                    sx.saturating_sub(1)
                } else {
                    sx + 1
                };
                let col = |cx: usize| 3 * at(cx, sy) + at(cx, ny);
                let bias = if x % 2 == 0 { 8 } else { 7 };
                (3 * col(sx) + col(nx) + bias) >> 4
            } else if fx == 2 && fy == 1 {
                let sx = x / 2;
                let nx = if x % 2 == 0 {
                    sx.saturating_sub(1)
                } else {
                    sx + 1
                };
                let bias = if x % 2 == 0 { 1 } else { 2 };
                (3 * at(sx, y) + at(nx, y) + bias) >> 2
            } else if fx == 1 && fy == 2 {
                let sy = y / 2;
                let ny = if y % 2 == 0 {
                    sy.saturating_sub(1)
                } else {
                    sy + 1
                };
                let bias = if y % 2 == 0 { 1 } else { 2 };
                (3 * at(x, sy) + at(x, ny) + bias) >> 2
            } else {
                at(x / fx, y / fy)
            };
            out[y * width + x] = v as u8;
        }
    }
    out
}

fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// YCbCr to RGB (JFIF / ITU-R BT.601 full range) in 16-bit fixed point.
fn ycc_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let (y, cb, cr) = (i32::from(y), i32::from(cb) - 128, i32::from(cr) - 128);
    let half = 1 << 15;
    [
        clamp8(y + ((91_881 * cr + half) >> 16)),
        clamp8(y + ((-22_554 * cb - 46_802 * cr + half) >> 16)),
        clamp8(y + ((116_130 * cb + half) >> 16)),
    ]
}

/// The EXIF orientation (1–8) from an APP1 segment, if present.
pub(crate) fn exif_orientation(body: &[u8]) -> Option<u8> {
    let tiff = body.strip_prefix(b"Exif\0\0")?;
    let le = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b = tiff.get(at..at + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let entries = usize::from(u16_at(ifd)?);
    for i in 0..entries.min(256) {
        let entry = ifd + 2 + 12 * i;
        if u16_at(entry)? == 0x0112 && u16_at(entry + 2)? == 3 {
            let value = u16_at(entry + 8)?;
            return (1..=8).contains(&value).then_some(value as u8);
        }
    }
    None
}

/// Rotate or flip `image` from EXIF orientation `o` (2–8) to upright.
pub(crate) fn orient(image: DecodedImage, o: u8) -> DecodedImage {
    if !(2..=8).contains(&o) {
        return image;
    }
    let (w, h) = (image.width as usize, image.height as usize);
    let transpose = o >= 5;
    let (ow, oh) = if transpose { (h, w) } else { (w, h) };
    let mut pixels = vec![0u8; image.pixels.len()];
    for y in 0..oh {
        for x in 0..ow {
            // Source coordinates for destination (x, y).
            let (sx, sy) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x),
            };
            let (d, s) = ((y * ow + x) * 4, (sy * w + sx) * 4);
            pixels[d..d + 4].copy_from_slice(&image.pixels[s..s + 4]);
        }
    }
    DecodedImage {
        width: ow as u32,
        height: oh as u32,
        pixels,
    }
}

/// Decode a JPEG image to RGBA8, applying its EXIF orientation as CSS
/// `image-orientation: from-image` (the initial value) requires.
pub fn decode_jpeg(
    bytes: &[u8],
    limits: DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<DecodedImage, ImageError> {
    cancelled(cancellation)?;
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return Err(ImageError::Signature);
    }
    let mut d = Decoder {
        data: bytes,
        pos: 2,
        quant: [None; 4],
        dc: [None, None, None, None],
        ac: [None, None, None, None],
        restart: 0,
        adobe: None,
        jfif: false,
        orientation: 1,
        frame: None,
        scans: 0,
        visits: 0,
        max_visits: 0,
        eobrun: 0,
    };
    loop {
        // Find the next marker, skipping fill bytes.
        if d.pos >= bytes.len() {
            break;
        }
        // Garbage between segments is skipped, as the IJG library does.
        while d.pos < bytes.len() && bytes[d.pos] != 0xff {
            d.pos += 1;
        }
        while bytes.get(d.pos) == Some(&0xff) {
            d.pos += 1;
        }
        let Some(&marker) = bytes.get(d.pos) else {
            break;
        };
        d.pos += 1;
        match marker {
            0xd9 => break,
            0xd0..=0xd7 | 0x01 => {}
            0xc0..=0xc2 => {
                let body = d.segment()?;
                d.sof(body, marker == 0xc2, &limits)?;
            }
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf | 0xdc => {
                return Err(ImageError::Unsupported);
            }
            0xc4 => {
                let body = d.segment()?;
                d.dht(body)?;
            }
            0xdb => {
                let body = d.segment()?;
                d.dqt(body)?;
            }
            0xdd => {
                let body = d.segment()?;
                d.restart = be16(body, 0).map_err(|_| ImageError::Invalid)?;
            }
            0xda => {
                let body = d.segment()?;
                let scan = d.sos(body)?;
                d.scan(&scan, cancellation)?;
            }
            0xe0 => {
                let body = d.segment()?;
                d.jfif |= body.starts_with(b"JFIF\0");
            }
            0xe1 => {
                let body = d.segment()?;
                if let Some(o) = exif_orientation(body) {
                    d.orientation = o;
                }
            }
            0xee => {
                let body = d.segment()?;
                if body.len() >= 12 && body.starts_with(b"Adobe") {
                    d.adobe = Some(body[11]);
                }
            }
            0xc8 | 0xd8 => return Err(ImageError::Invalid),
            _ => {
                d.segment()?;
            }
        }
    }
    if d.scans == 0 {
        return Err(ImageError::Truncated);
    }
    let frame = d.frame.take().ok_or(ImageError::Invalid)?;
    let image = output(&frame, d.adobe, d.jfif, cancellation)?;
    Ok(orient(image, d.orientation))
}

fn output(
    frame: &Frame,
    adobe: Option<u8>,
    jfif: bool,
    cancellation: &CancellationToken,
) -> Result<DecodedImage, ImageError> {
    let (width, height) = (frame.width, frame.height);
    let mut planes = Vec::with_capacity(frame.components.len());
    for c in &frame.components {
        cancelled(cancellation)?;
        let stride = c.bw * 8;
        let mut plane = Plane {
            w: (width * c.h).div_ceil(frame.hmax),
            h: (height * c.v).div_ceil(frame.vmax),
            data: vec![0u8; stride * c.bh * 8],
        };
        // Components never scanned stay mid-grey (all-zero coefficients).
        let quant = c.quant.unwrap_or([0; 64]);
        for by in 0..c.used_h {
            for bx in 0..c.used_w {
                let at = (by * c.bw + bx) * 64;
                let out = &mut plane.data[by * 8 * stride + bx * 8..];
                idct(&c.coefs[at..at + 64], &quant, out, stride);
            }
        }
        // Compact rows to the component's own width.
        let mut compact = Vec::with_capacity(plane.w * plane.h);
        for y in 0..plane.h {
            compact.extend_from_slice(&plane.data[y * stride..y * stride + plane.w]);
        }
        plane.data = compact;
        let (fx, fy) = (frame.hmax / c.h, frame.vmax / c.v);
        let full = if c.h == frame.hmax && c.v == frame.vmax {
            plane.data
        } else if frame.hmax % c.h == 0 && frame.vmax % c.v == 0 {
            upsample(&plane, fx, fy, width, height)
        } else {
            // Non-integral factors: nearest sample.
            let mut out = vec![0u8; width * height];
            for y in 0..height {
                for x in 0..width {
                    let sx = (x * c.h / frame.hmax).min(plane.w - 1);
                    let sy = (y * c.v / frame.vmax).min(plane.h - 1);
                    out[y * width + x] = plane.data[sy * plane.w + sx];
                }
            }
            out
        };
        planes.push(full);
    }
    let mut pixels = vec![255u8; width * height * 4];
    let ids: Vec<u8> = frame.components.iter().map(|c| c.id).collect();
    for i in 0..width * height {
        let rgb = match planes.len() {
            1 => [planes[0][i]; 3],
            3 => {
                let (a, b, c) = (planes[0][i], planes[1][i], planes[2][i]);
                let rgb_ids = ids == b"RGB" && !jfif;
                if adobe == Some(0) || (adobe.is_none() && rgb_ids) {
                    [a, b, c]
                } else {
                    ycc_to_rgb(a, b, c)
                }
            }
            _ => {
                let (mut c, mut m, mut y, k) =
                    (planes[0][i], planes[1][i], planes[2][i], planes[3][i]);
                if adobe == Some(2) {
                    [c, m, y] = ycc_to_rgb(c, m, y);
                    // YCCK stores inverted CMY as YCbCr.
                    [c, m, y] = [255 - c, 255 - m, 255 - y];
                }
                // Adobe files store CMYK inverted (0 = full ink).
                let ink = |v: u8, k: u8| -> u8 {
                    if adobe.is_some() {
                        ((u32::from(v) * u32::from(k) + 127) / 255) as u8
                    } else {
                        (((255 - u32::from(v)) * (255 - u32::from(k)) + 127) / 255) as u8
                    }
                };
                [ink(c, k), ink(m, k), ink(y, k)]
            }
        };
        pixels[i * 4..i * 4 + 3].copy_from_slice(&rgb);
    }
    Ok(DecodedImage {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idct_of_a_dc_only_block_is_flat() {
        let mut block = [0i16; 64];
        block[0] = 80; // 80 × q1 / 8 = +10
        let quant = [1u16; 64];
        let mut out = [0u8; 64];
        idct(&block, &quant, &mut out, 8);
        assert!(out.iter().all(|&v| v == 138), "{out:?}");
    }

    #[test]
    fn idct_matches_the_definition_for_single_coefficients() {
        // f(x, y) = ¼·C(u)C(v)·F·cos((2x+1)uπ/16)·cos((2y+1)vπ/16)
        for (u, v) in [(1, 0), (0, 3), (2, 5), (7, 7)] {
            let mut block = [0i16; 64];
            block[v * 8 + u] = 400;
            let mut out = [0u8; 64];
            idct(&block, &[1; 64], &mut out, 8);
            for y in 0..8 {
                for x in 0..8 {
                    let c = |k: usize| if k == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                    let f = 0.25
                        * c(u)
                        * c(v)
                        * 400.0
                        * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos()
                        * ((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI / 16.0).cos();
                    let expected = (f + 128.0).round().clamp(0.0, 255.0);
                    assert!(
                        (f64::from(out[y * 8 + x]) - expected).abs() <= 1.0,
                        "({u},{v}) at ({x},{y}): {} vs {expected}",
                        out[y * 8 + x]
                    );
                }
            }
        }
    }

    #[test]
    fn huffman_tables_reject_oversubscribed_codes() {
        let mut counts = [0u8; 16];
        counts[0] = 3; // three 1-bit codes
        assert!(Huffman::new(&counts, &[1, 2, 3]).is_err());
        counts[0] = 2;
        assert!(Huffman::new(&counts, &[1, 2]).is_ok());
    }

    #[test]
    fn ycc_conversion_matches_jfif() {
        assert_eq!(ycc_to_rgb(128, 128, 128), [128, 128, 128]);
        assert_eq!(ycc_to_rgb(255, 128, 128), [255, 255, 255]);
        assert_eq!(ycc_to_rgb(76, 85, 255), [254, 0, 0]);
    }

    #[test]
    fn orientation_maps_every_exif_value() {
        // A 2×1 image [A B]; orientation 6 (rotate 90° clockwise) gives a
        // 1×2 column [A; B].
        let image = DecodedImage {
            width: 2,
            height: 1,
            pixels: vec![1, 1, 1, 1, 2, 2, 2, 2],
        };
        let rotated = orient(image.clone(), 6);
        assert_eq!((rotated.width, rotated.height), (1, 2));
        assert_eq!(rotated.pixels, vec![1, 1, 1, 1, 2, 2, 2, 2]);
        let rotated = orient(image.clone(), 8);
        assert_eq!(rotated.pixels, vec![2, 2, 2, 2, 1, 1, 1, 1]);
        assert_eq!(
            orient(image.clone(), 2).pixels,
            vec![2, 2, 2, 2, 1, 1, 1, 1]
        );
        assert_eq!(orient(image.clone(), 1), image);
        for o in 1..=8 {
            let out = orient(image.clone(), o);
            assert_eq!(out.pixels.len(), 8);
        }
    }

    #[test]
    fn exif_orientation_is_read_in_both_byte_orders() {
        let mut le = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0".to_vec();
        le.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        assert_eq!(exif_orientation(&le), Some(6));
        let mut be = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
        be.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 3, 0, 0]);
        assert_eq!(exif_orientation(&be), Some(3));
        assert_eq!(exif_orientation(b"Exif\0\0XX"), None);
    }
}
