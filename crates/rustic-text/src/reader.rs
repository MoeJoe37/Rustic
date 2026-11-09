//! Checked big-endian reads over font bytes.
//!
//! Every read checks its bounds and reports the table being parsed, so a
//! truncated or lying font becomes [`FontError::Truncated`], never a panic.

use crate::error::{FontError, Tag};

/// A cursor over one table's bytes.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    tag: Tag,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8], tag: Tag) -> Self {
        Self { data, pos: 0, tag }
    }

    /// A reader over `data[offset..]`.
    pub(crate) fn at(data: &'a [u8], offset: usize, tag: Tag) -> Result<Self, FontError> {
        if offset > data.len() {
            return Err(FontError::Truncated(tag));
        }
        Ok(Self {
            data,
            pos: offset,
            tag,
        })
    }

    pub(crate) fn truncated(&self) -> FontError {
        FontError::Truncated(self.tag)
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Result<&'a [u8], FontError> {
        let end = self.pos.checked_add(len).ok_or(self.truncated())?;
        let slice = self.data.get(self.pos..end).ok_or(self.truncated())?;
        self.pos = end;
        Ok(slice)
    }

    pub(crate) fn skip(&mut self, len: usize) -> Result<(), FontError> {
        self.bytes(len).map(|_| ())
    }

    pub(crate) fn u8(&mut self) -> Result<u8, FontError> {
        Ok(self.bytes(1)?[0])
    }

    pub(crate) fn i8(&mut self) -> Result<i8, FontError> {
        Ok(i8::from_be_bytes([self.u8()?]))
    }

    pub(crate) fn u16(&mut self) -> Result<u16, FontError> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn i16(&mut self) -> Result<i16, FontError> {
        let b = self.bytes(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, FontError> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn tag(&mut self) -> Result<Tag, FontError> {
        let b = self.bytes(4)?;
        Ok(Tag([b[0], b[1], b[2], b[3]]))
    }
}

/// `u16` at `offset` in `data`.
pub(crate) fn u16_at(data: &[u8], offset: usize, tag: Tag) -> Result<u16, FontError> {
    Reader::at(data, offset, tag)?.u16()
}

/// `u32` at `offset` in `data`.
pub(crate) fn u32_at(data: &[u8], offset: usize, tag: Tag) -> Result<u32, FontError> {
    Reader::at(data, offset, tag)?.u32()
}

/// `usize` from a `u32` font field. Every supported target has at least
/// 32-bit pointers (docs/PLATFORMS.md), so this never truncates.
pub(crate) fn size(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}
