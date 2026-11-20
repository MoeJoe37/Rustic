//! The field encoding inside a frame: little-endian integers and
//! length-prefixed byte strings, every count checked against the bytes
//! that remain before anything is allocated.

use std::fmt;

/// Why a frame did not decode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The frame ended inside a field.
    Truncated,
    /// Bytes were left after the message.
    TrailingBytes,
    /// A message or variant tag this protocol does not have.
    UnknownTag(u16),
    /// A string is not UTF-8.
    InvalidUtf8,
    /// A boolean or option byte other than 0 or 1.
    InvalidFlag,
    /// A field breaks its own limit (for example a count over its bound).
    OutOfRange,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("truncated field"),
            Self::TrailingBytes => formatter.write_str("trailing bytes"),
            Self::UnknownTag(tag) => write!(formatter, "unknown tag {tag}"),
            Self::InvalidUtf8 => formatter.write_str("invalid UTF-8"),
            Self::InvalidFlag => formatter.write_str("invalid flag byte"),
            Self::OutOfRange => formatter.write_str("field out of range"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Builds a frame payload.
#[derive(Debug, Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// An empty payload.
    pub fn new() -> Self {
        Self::default()
    }

    /// The payload.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// A byte.
    pub fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// A tag or small number.
    pub fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// A 32-bit number.
    pub fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// A 64-bit number.
    pub fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// A boolean.
    pub fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    /// A byte string. Lengths above `u32::MAX` cannot occur: frames are
    /// bounded far below that.
    pub fn bytes(&mut self, value: &[u8]) {
        self.u32(u32::try_from(value.len()).unwrap_or(u32::MAX));
        self.bytes.extend_from_slice(value);
    }

    /// A string.
    pub fn str(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    /// An optional string.
    pub fn opt_str(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.u8(1);
                self.str(value);
            }
            None => self.u8(0),
        }
    }

    /// A list of strings.
    pub fn strs(&mut self, values: &[String]) {
        self.u32(u32::try_from(values.len()).unwrap_or(u32::MAX));
        for value in values {
            self.str(value);
        }
    }
}

/// Reads a frame payload.
#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Read `bytes`.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Fail unless everything was read.
    pub fn finish(&self) -> Result<(), DecodeError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if count > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    /// A byte.
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    /// A tag or small number.
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// A 32-bit number.
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A 64-bit number.
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(array))
    }

    /// A boolean (0 or 1 only).
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::InvalidFlag),
        }
    }

    /// A byte string; its length is checked before copying.
    pub fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let length = self.u32()? as usize;
        Ok(self.take(length)?.to_vec())
    }

    /// A UTF-8 string.
    pub fn string(&mut self) -> Result<String, DecodeError> {
        String::from_utf8(self.bytes()?).map_err(|_| DecodeError::InvalidUtf8)
    }

    /// An optional string.
    pub fn opt_string(&mut self) -> Result<Option<String>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            1 => self.string().map(Some),
            _ => Err(DecodeError::InvalidFlag),
        }
    }

    /// A list of at most `max` strings; each needs at least four bytes, so
    /// the count is checked against the remaining bytes first.
    pub fn strings(&mut self, max: usize) -> Result<Vec<String>, DecodeError> {
        let count = self.u32()? as usize;
        if count > max || count.saturating_mul(4) > self.bytes.len() {
            return Err(DecodeError::OutOfRange);
        }
        (0..count).map(|_| self.string()).collect()
    }
}
