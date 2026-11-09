//! `cmap` (OpenType §cmap): character to glyph mapping, formats 4 and 12.
//!
//! The best Unicode subtable is chosen and fully validated when the font
//! loads (sorted, non-overlapping ranges inside the table), so later lookups
//! only need local bounds checks.

use crate::error::{FontError, Tag};
use crate::reader::{Reader, size, u16_at, u32_at};

const TAG: Tag = Tag::CMAP;

fn invalid(reason: &'static str) -> FontError {
    FontError::Invalid { table: TAG, reason }
}

/// A validated Unicode subtable: its byte range in the `cmap` table.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CharMap {
    /// Format 4 at this offset with `segments` segments.
    Segments { offset: usize, segments: usize },
    /// Format 12 at this offset with `groups` groups.
    Groups { offset: usize, groups: usize },
}

/// Preference among encoding records: full-repertoire format 12 first, then
/// BMP format 4. Lower is better.
fn rank(platform: u16, encoding: u16, format: u16) -> Option<u8> {
    match (platform, encoding, format) {
        (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => Some(0),
        (3, 1, 4) | (0, 3, 4) | (0, 1, 4) | (0, 0, 4) => Some(1),
        _ => None,
    }
}

impl CharMap {
    pub(crate) fn parse(cmap: &[u8]) -> Result<Self, FontError> {
        let mut r = Reader::new(cmap, TAG);
        if r.u16()? != 0 {
            return Err(invalid("unknown version"));
        }
        let records = r.u16()?;
        let mut best: Option<(u8, usize)> = None;
        for _ in 0..records {
            let platform = r.u16()?;
            let encoding = r.u16()?;
            let offset = size(r.u32()?);
            let format = u16_at(cmap, offset, TAG)?;
            if let Some(rank) = rank(platform, encoding, format) {
                if best.is_none_or(|(b, _)| rank < b) {
                    best = Some((rank, offset));
                }
            }
        }
        let Some((_, offset)) = best else {
            return Err(FontError::Unsupported(
                "no Unicode cmap subtable in format 4 or 12",
            ));
        };
        match u16_at(cmap, offset, TAG)? {
            4 => parse_format4(cmap, offset),
            _ => parse_format12(cmap, offset),
        }
    }

    /// The glyph for `c`, or 0 (`.notdef`) when unmapped.
    pub(crate) fn glyph(&self, cmap: &[u8], c: u32) -> u32 {
        match *self {
            Self::Segments { offset, segments } => lookup4(cmap, offset, segments, c).unwrap_or(0),
            Self::Groups { offset, groups } => lookup12(cmap, offset, groups, c).unwrap_or(0),
        }
    }
}

/// The byte range `offset..offset+length` of a subtable, checked.
fn subtable(cmap: &[u8], offset: usize, length: usize) -> Result<&[u8], FontError> {
    let end = offset
        .checked_add(length)
        .ok_or(FontError::Truncated(TAG))?;
    cmap.get(offset..end).ok_or(FontError::Truncated(TAG))
}

fn parse_format4(cmap: &[u8], offset: usize) -> Result<CharMap, FontError> {
    let length = usize::from(u16_at(cmap, offset + 2, TAG)?);
    let table = subtable(cmap, offset, length)?;
    let seg_x2 = usize::from(u16_at(table, 6, TAG)?);
    if seg_x2 == 0 || seg_x2 % 2 != 0 {
        return Err(invalid("format 4 segment count"));
    }
    let segments = seg_x2 / 2;
    // endCode, reservedPad, startCode, idDelta, idRangeOffset.
    if 14 + seg_x2 * 4 + 2 > table.len() {
        return Err(FontError::Truncated(TAG));
    }
    let mut previous_end: Option<u16> = None;
    for i in 0..segments {
        let end = u16_at(table, 14 + 2 * i, TAG)?;
        let start = u16_at(table, 16 + seg_x2 + 2 * i, TAG)?;
        if start > end || previous_end.is_some_and(|p| start <= p) {
            return Err(invalid("format 4 segments overlap or are unsorted"));
        }
        previous_end = Some(end);
    }
    if previous_end != Some(0xFFFF) {
        return Err(invalid("format 4 lacks the final 0xFFFF segment"));
    }
    Ok(CharMap::Segments { offset, segments })
}

fn lookup4(cmap: &[u8], offset: usize, segments: usize, c: u32) -> Option<u32> {
    let c = u16::try_from(c).ok()?;
    let table = subtable(
        cmap,
        offset,
        usize::from(u16_at(cmap, offset + 2, TAG).ok()?),
    )
    .ok()?;
    let seg_x2 = segments * 2;
    // Binary search for the first segment whose end is >= c.
    let (mut lo, mut hi) = (0, segments);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if u16_at(table, 14 + 2 * mid, TAG).ok()? < c {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let i = lo;
    if i >= segments {
        return None;
    }
    let start = u16_at(table, 16 + seg_x2 + 2 * i, TAG).ok()?;
    if c < start {
        return None;
    }
    let delta = u16_at(table, 16 + seg_x2 * 2 + 2 * i, TAG).ok()?;
    let range_at = 16 + seg_x2 * 3 + 2 * i;
    let range_offset = u16_at(table, range_at, TAG).ok()?;
    let glyph = if range_offset == 0 {
        c.wrapping_add(delta)
    } else {
        let at = range_at + usize::from(range_offset) + 2 * usize::from(c - start);
        match u16_at(table, at, TAG).ok()? {
            0 => 0,
            g => g.wrapping_add(delta),
        }
    };
    Some(u32::from(glyph))
}

fn parse_format12(cmap: &[u8], offset: usize) -> Result<CharMap, FontError> {
    let length = size(u32_at(cmap, offset + 4, TAG)?);
    let table = subtable(cmap, offset, length)?;
    let groups = size(u32_at(table, 12, TAG)?);
    let needed = groups
        .checked_mul(12)
        .and_then(|n| n.checked_add(16))
        .ok_or(FontError::Truncated(TAG))?;
    if needed > table.len() {
        return Err(FontError::Truncated(TAG));
    }
    let mut previous_end: Option<u32> = None;
    for i in 0..groups {
        let at = 16 + 12 * i;
        let start = u32_at(table, at, TAG)?;
        let end = u32_at(table, at + 4, TAG)?;
        if start > end || end > 0x10FFFF || previous_end.is_some_and(|p| start <= p) {
            return Err(invalid(
                "format 12 groups overlap, are unsorted or exceed Unicode",
            ));
        }
        previous_end = Some(end);
    }
    Ok(CharMap::Groups { offset, groups })
}

fn lookup12(cmap: &[u8], offset: usize, groups: usize, c: u32) -> Option<u32> {
    let (mut lo, mut hi) = (0, groups);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let at = offset + 16 + 12 * mid;
        let start = u32_at(cmap, at, TAG).ok()?;
        let end = u32_at(cmap, at + 4, TAG).ok()?;
        if c < start {
            hi = mid;
        } else if c > end {
            lo = mid + 1;
        } else {
            let first = u32_at(cmap, at + 8, TAG).ok()?;
            return first.checked_add(c - start);
        }
    }
    None
}
