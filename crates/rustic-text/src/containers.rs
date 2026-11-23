//! Preflight untrusted WOFF directories before the reviewed decoder allocates.
use crate::{FontError, FontLimits, Tag};
fn bad() -> FontError {
    FontError::Invalid {
        table: Tag::DIRECTORY,
        reason: "invalid WOFF container",
    }
}
fn n16(d: &[u8], p: usize) -> Result<usize, FontError> {
    let b = d.get(p..p + 2).ok_or_else(bad)?;
    Ok(usize::from(u16::from_be_bytes([b[0], b[1]])))
}
fn n32(d: &[u8], p: usize) -> Result<usize, FontError> {
    let b = d.get(p..p + 4).ok_or_else(bad)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
}
fn add(n: usize, v: usize, cap: usize) -> Result<usize, FontError> {
    n.checked_add(v)
        .filter(|&x| x <= cap)
        .ok_or(FontError::LimitExceeded("decoded font bytes"))
}
fn padded(n: usize) -> Result<usize, FontError> {
    n.checked_add(3)
        .map(|n| n & !3)
        .ok_or(FontError::LimitExceeded("padded font bytes"))
}
fn base128(d: &[u8], p: &mut usize) -> Result<usize, FontError> {
    let mut n = 0u32;
    for k in 0..5 {
        let b = *d.get(*p).ok_or_else(bad)?;
        *p += 1;
        if (k == 0 && b == 0x80) || (n & 0xFE00_0000 != 0) {
            return Err(bad());
        }
        n = (n << 7) | u32::from(b & 127);
        if b & 128 == 0 {
            return Ok(n as usize);
        }
    }
    Err(bad())
}
pub(crate) fn decode(data: Vec<u8>, limits: &FontLimits) -> Result<Vec<u8>, FontError> {
    if data.len() > limits.max_font_bytes {
        return Err(FontError::LimitExceeded("font size"));
    }
    let w2 = match data.get(..4) {
        Some(b"wOFF") => false,
        Some(b"wOF2") => true,
        _ => return Ok(data),
    };
    let head = if w2 { 48 } else { 44 };
    if data.len() < head || n32(&data, 8)? != data.len() || n16(&data, 14)? != 0 {
        return Err(bad());
    }
    if data.get(4..8) == Some(b"ttcf") {
        return Err(FontError::Unsupported("WOFF collections"));
    }
    let count = n16(&data, 12)?;
    if count == 0 || count > limits.max_tables || count >= 4096 {
        return Err(FontError::LimitExceeded("WOFF tables"));
    }
    let cap = limits.max_font_bytes;
    let advertised = n32(&data, 16)?;
    if advertised > cap {
        return Err(FontError::LimitExceeded("decoded font bytes"));
    }
    let mut original = add(12, count * 16, cap)?;
    if w2 {
        let mut p = 48;
        let mut transformed = 0;
        for _ in 0..count {
            let flags = *data.get(p).ok_or_else(bad)?;
            p += 1;
            let index = flags & 63;
            let custom = if index == 63 {
                let t = data.get(p..p + 4).ok_or_else(bad)?;
                p += 4;
                Some(t)
            } else {
                None
            };
            let glyf = index == 10 || custom == Some(b"glyf");
            let loca = index == 11 || custom == Some(b"loca");
            let size = base128(&data, &mut p)?;
            original = add(original, padded(size)?, cap)?;
            let changed = if glyf || loca {
                flags >> 6 != 3
            } else {
                flags >> 6 != 0
            };
            let stored = if changed {
                base128(&data, &mut p)?
            } else {
                size
            };
            if loca && changed && stored != 0 {
                return Err(bad());
            }
            transformed = add(transformed, stored, cap)?;
        }
        let compressed = n32(&data, 20)?;
        if p.checked_add(compressed).is_none_or(|e| e > data.len()) {
            return Err(bad());
        }
    } else {
        for k in 0..count {
            let p = 44 + k * 20;
            let offset = n32(&data, p + 4)?;
            let compressed = n32(&data, p + 8)?;
            let size = n32(&data, p + 12)?;
            if compressed > size
                || offset
                    .checked_add(compressed)
                    .is_none_or(|e| e > data.len())
            {
                return Err(bad());
            }
            original = add(original, padded(size)?, cap)?;
        }
    }
    let out = if w2 {
        wuff::decompress_woff2(&data)
    } else {
        wuff::decompress_woff1_with_custom_z(&data, &mut |bytes, size| {
            rustic_codecs::zlib_decompress(
                bytes,
                size.min(cap),
                &rustic_base::CancellationToken::default(),
            )
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        })
    }
    .map_err(|_| bad())?;
    if out.len() > cap {
        return Err(FontError::LimitExceeded("decoded font bytes"));
    }
    if !w2 && out.len() != advertised {
        return Err(bad());
    }
    Ok(out)
}

/// The number of faces in an OpenType collection (`ttcf`), or `None` when
/// `data` is not a collection.
pub fn collection_len(data: &[u8]) -> Option<usize> {
    (data.get(..4) == Some(b"ttcf")).then(|| n32(data, 8).unwrap_or(0))
}

/// Face `index` of an OpenType collection (`ttcf`, OpenType §5 "Font
/// Collections") as a standalone sfnt: its table directory rebuilt with
/// each table copied after it, 4-byte aligned. Tables the collection
/// shares between faces are copied into each extracted face. Every offset
/// and length is checked against the collection and `limits` before
/// anything is allocated.
pub fn collection_face(
    data: &[u8],
    index: usize,
    limits: &FontLimits,
) -> Result<Vec<u8>, FontError> {
    let invalid = || FontError::Invalid {
        table: Tag::DIRECTORY,
        reason: "invalid font collection",
    };
    if data.len() > limits.max_font_bytes {
        return Err(FontError::LimitExceeded("font size"));
    }
    let count = collection_len(data).ok_or(FontError::NotSfnt)?;
    if count == 0 || count > limits.max_tables.max(1) * 64 {
        return Err(invalid());
    }
    if index >= count {
        return Err(FontError::Invalid {
            table: Tag::DIRECTORY,
            reason: "no such face in the collection",
        });
    }
    let directory = n32(data, 12 + index * 4).map_err(|_| invalid())?;
    let version = data.get(directory..directory + 4).ok_or_else(invalid)?;
    let tables = n16(data, directory + 4).map_err(|_| invalid())?;
    if tables == 0 || tables > limits.max_tables {
        return Err(FontError::LimitExceeded("table count"));
    }
    let header = 12 + 16 * tables;
    let mut records = Vec::with_capacity(tables);
    let mut total = header;
    for k in 0..tables {
        let at = directory + 12 + 16 * k;
        let record = data.get(at..at + 16).ok_or_else(invalid)?;
        let offset = n32(data, at + 8).map_err(|_| invalid())?;
        let length = n32(data, at + 12).map_err(|_| invalid())?;
        let end = offset.checked_add(length).ok_or_else(invalid)?;
        if end > data.len() {
            return Err(FontError::TableOutOfBounds(Tag([
                record[0], record[1], record[2], record[3],
            ])));
        }
        total = add(total, padded(length)?, limits.max_font_bytes)?;
        records.push((record, offset, length));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(version);
    // numTables, searchRange, entrySelector, rangeShift as the face had them.
    out.extend_from_slice(
        data.get(directory + 4..directory + 12)
            .ok_or_else(invalid)?,
    );
    let mut next = header;
    for (record, _, length) in &records {
        out.extend_from_slice(&record[..8]);
        out.extend_from_slice(&u32::try_from(next).map_err(|_| invalid())?.to_be_bytes());
        out.extend_from_slice(&u32::try_from(*length).map_err(|_| invalid())?.to_be_bytes());
        next += padded(*length)?;
    }
    for (_, offset, length) in &records {
        out.extend_from_slice(&data[*offset..offset + length]);
        out.resize(padded(out.len())?, 0);
    }
    Ok(out)
}
