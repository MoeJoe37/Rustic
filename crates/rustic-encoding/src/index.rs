//! Index lookups: pointer to code point for decoders, code point to pointer
//! for encoders (Encoding Standard §5 "Indexes").

use std::sync::OnceLock;

use crate::tables;

/// "Index code point" in a little-endian `u16` index; `None` when unmapped.
pub(crate) fn code_point_u16(index: &[u8], pointer: usize) -> Option<u32> {
    let offset = pointer.checked_mul(2)?;
    let bytes = index.get(offset..offset.checked_add(2)?)?;
    let value = u16::from_le_bytes([bytes[0], bytes[1]]);
    (value != 0).then_some(u32::from(value))
}

/// "Index code point" in a little-endian `u32` index; `None` when unmapped.
pub(crate) fn code_point_u32(index: &[u8], pointer: usize) -> Option<u32> {
    let offset = pointer.checked_mul(4)?;
    let bytes = index.get(offset..offset.checked_add(4)?)?;
    let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    (value != 0).then_some(value)
}

pub(crate) fn big5(pointer: usize) -> Option<u32> {
    code_point_u32(tables::BIG5, pointer)
}

pub(crate) fn euc_kr(pointer: usize) -> Option<u32> {
    code_point_u16(tables::EUC_KR, pointer)
}

pub(crate) fn gb18030(pointer: usize) -> Option<u32> {
    code_point_u16(tables::GB18030, pointer)
}

pub(crate) fn jis0208(pointer: usize) -> Option<u32> {
    code_point_u16(tables::JIS0208, pointer)
}

pub(crate) fn jis0212(pointer: usize) -> Option<u32> {
    code_point_u16(tables::JIS0212, pointer)
}

/// "Index gb18030 ranges code point".
pub(crate) fn gb18030_ranges_code_point(pointer: u32) -> Option<u32> {
    if (pointer > 39419 && pointer < 189000) || pointer > 1237575 {
        return None;
    }
    if pointer == 7457 {
        return Some(0xE7C7);
    }
    let ranges = &tables::GB18030_RANGES;
    // The last entry whose pointer is <= `pointer`; entry 0 has pointer 0.
    let position = ranges
        .partition_point(|&(start, _)| start <= pointer)
        .checked_sub(1)?;
    let (offset, code_point_offset) = ranges[position];
    Some(code_point_offset + (pointer - offset))
}

/// "Index gb18030 ranges pointer".
pub(crate) fn gb18030_ranges_pointer(code_point: u32) -> u32 {
    if code_point == 0xE7C7 {
        return 7457;
    }
    let ranges = &tables::GB18030_RANGES;
    // Entry 0 maps U+0080, and encoders only ask for non-ASCII code points.
    let position = ranges
        .partition_point(|&(_, start)| start <= code_point)
        .saturating_sub(1);
    let (pointer_offset, offset) = ranges[position];
    pointer_offset + (code_point - offset)
}

/// A reverse index: (code point, pointer) pairs sorted by code point, with
/// one pointer per code point chosen by the encoder's rule.
pub(crate) struct Reverse(Vec<(u32, u16)>);

impl Reverse {
    fn build(
        length: usize,
        lookup: fn(usize) -> Option<u32>,
        keep: impl Fn(usize) -> bool,
    ) -> Self {
        let mut pairs = Vec::new();
        for pointer in 0..length {
            if !keep(pointer) {
                continue;
            }
            if let (Some(code_point), Ok(pointer)) = (lookup(pointer), u16::try_from(pointer)) {
                pairs.push((code_point, pointer));
            }
        }
        // A stable sort keeps pointers in ascending order within a code point,
        // so deduplicating keeps the first pointer ("index pointer").
        pairs.sort_by_key(|&(code_point, _)| code_point);
        pairs.dedup_by_key(|&mut (code_point, _)| code_point);
        Reverse(pairs)
    }

    pub(crate) fn pointer(&self, code_point: u32) -> Option<usize> {
        self.0
            .binary_search_by_key(&code_point, |&(code_point, _)| code_point)
            .ok()
            .map(|position| usize::from(self.0[position].1))
    }
}

const BIG5_POINTERS: usize = tables::BIG5.len() / 4;
const EUC_KR_POINTERS: usize = tables::EUC_KR.len() / 2;
const GB18030_POINTERS: usize = tables::GB18030.len() / 2;
const JIS0208_POINTERS: usize = tables::JIS0208.len() / 2;

/// "Index pointer" in index EUC-KR.
pub(crate) fn euc_kr_pointer(code_point: u32) -> Option<usize> {
    static REVERSE: OnceLock<Reverse> = OnceLock::new();
    REVERSE
        .get_or_init(|| Reverse::build(EUC_KR_POINTERS, euc_kr, |_| true))
        .pointer(code_point)
}

/// "Index pointer" in index gb18030.
pub(crate) fn gb18030_pointer(code_point: u32) -> Option<usize> {
    static REVERSE: OnceLock<Reverse> = OnceLock::new();
    REVERSE
        .get_or_init(|| Reverse::build(GB18030_POINTERS, gb18030, |_| true))
        .pointer(code_point)
}

/// "Index pointer" in index jis0208 (EUC-JP and ISO-2022-JP encoders).
pub(crate) fn jis0208_pointer(code_point: u32) -> Option<usize> {
    static REVERSE: OnceLock<Reverse> = OnceLock::new();
    REVERSE
        .get_or_init(|| Reverse::build(JIS0208_POINTERS, jis0208, |_| true))
        .pointer(code_point)
}

/// "Index Shift_JIS pointer": index jis0208 without pointers 8272 to 8835.
pub(crate) fn shift_jis_pointer(code_point: u32) -> Option<usize> {
    static REVERSE: OnceLock<Reverse> = OnceLock::new();
    REVERSE
        .get_or_init(|| {
            Reverse::build(JIS0208_POINTERS, jis0208, |pointer| {
                !(8272..=8835).contains(&pointer)
            })
        })
        .pointer(code_point)
}

/// "Index Big5 pointer": index Big5 without the pointers below
/// (0xA1 - 0x81) × 157, and the last pointer for six code points whose
/// first pointer is an HKSCS duplicate.
pub(crate) fn big5_pointer(code_point: u32) -> Option<usize> {
    const FIRST: usize = (0xA1 - 0x81) * 157;
    static REVERSE: OnceLock<Reverse> = OnceLock::new();
    if matches!(
        code_point,
        0x2550 | 0x255E | 0x2561 | 0x256A | 0x5341 | 0x5345
    ) {
        return (FIRST..BIG5_POINTERS)
            .rev()
            .find(|&pointer| big5(pointer) == Some(code_point));
    }
    REVERSE
        .get_or_init(|| Reverse::build(BIG5_POINTERS, big5, |pointer| pointer >= FIRST))
        .pointer(code_point)
}
