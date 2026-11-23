use std::fmt;

/// What prevented the binary scanner from reading a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryErrorKind {
    /// A byte or length-prefixed payload extends past its containing span.
    UnexpectedEnd,
    /// The four-byte signature is incorrect.
    BadMagic,
    /// The version is different from version 1.
    UnsupportedVersion,
    /// An integer has too many bytes or invalid unused bits.
    InvalidLeb,
    /// A section identifier is unrecognized, repeated or out of order.
    InvalidSection,
    /// A structured payload contains extra bytes after its last field.
    TrailingBytes,
    /// A name contains invalid UTF-8.
    InvalidName,
    /// Function declaration and body counts differ.
    FunctionCountMismatch,
    /// The declared data count and segment count differ.
    DataCountMismatch,
    /// A local declaration uses an invalid binary type code.
    InvalidType,
    /// A function body lacks its outer end byte.
    MissingEnd,
    /// An instruction byte or reserved field is invalid.
    InvalidInstruction,
    /// An instruction belongs to a group the decoder has yet to implement.
    UnsupportedInstruction,
    /// An `else` does not belong to an open if without an earlier `else`.
    InvalidControl,
    /// Admission limits or metadata allocation capacity were exceeded.
    LimitExceeded,
    /// The host cancelled scanning.
    Cancelled,
}

/// A scanner failure with a byte position in the original module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinaryError {
    /// Absolute byte offset. At end of input this can equal input length.
    pub offset: usize,
    /// The reason scanning stopped.
    pub kind: BinaryErrorKind,
}

impl fmt::Display for BinaryError {
    fn fmt(&self, writer: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            writer,
            "WebAssembly binary {:?} at byte {}",
            self.kind, self.offset
        )
    }
}

impl std::error::Error for BinaryError {}

pub(crate) struct Reader<'a> {
    rest: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8], position: usize) -> Self {
        Self {
            rest: bytes,
            position,
        }
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn remaining(&self) -> &'a [u8] {
        self.rest
    }

    pub(crate) fn error(&self, kind: BinaryErrorKind) -> BinaryError {
        BinaryError {
            kind,
            offset: self.position,
        }
    }

    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8], BinaryError> {
        let Some((part, rest)) = self.rest.split_at_checked(length) else {
            return Err(self.error(BinaryErrorKind::UnexpectedEnd));
        };
        self.rest = rest;
        self.position += length;
        Ok(part)
    }

    pub(crate) fn byte(&mut self) -> Result<u8, BinaryError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Result<u32, BinaryError> {
        self.leb(32, false).map(|value| value as u32)
    }

    pub(crate) fn u64(&mut self) -> Result<u64, BinaryError> {
        self.leb(64, false).map(|value| value as u64)
    }

    pub(crate) fn s32(&mut self) -> Result<i32, BinaryError> {
        self.leb(32, true).map(|value| value as i32)
    }

    pub(crate) fn s33(&mut self) -> Result<i64, BinaryError> {
        self.leb(33, true)
    }

    pub(crate) fn s64(&mut self) -> Result<i64, BinaryError> {
        self.leb(64, true)
    }

    fn leb(&mut self, width: u32, signed: bool) -> Result<i64, BinaryError> {
        let mut bits = 0_u64;
        for shift in (0..width).step_by(7) {
            let at = self.position;
            let byte = self.byte()?;
            let used = (width - shift).min(7);
            let mask = (1_u8 << used) - 1;
            let payload = byte & 0x7f;
            bits |= u64::from(payload & mask) << shift;
            if byte & 0x80 == 0 {
                let negative = signed && payload & (1 << (used - 1)) != 0;
                let padding = if negative { 0x7f & !mask } else { 0 };
                if payload & !mask == padding {
                    if negative && shift + used < 64 {
                        bits |= !0_u64 << (shift + used);
                    }
                    return Ok(bits as i64);
                }
            } else if used == 7 && shift + used < width {
                continue;
            }
            return Err(BinaryError {
                offset: at,
                kind: BinaryErrorKind::InvalidLeb,
            });
        }
        unreachable!("the final LEB byte either terminates or fails")
    }

    pub(crate) fn sized_span(&mut self) -> Result<Self, BinaryError> {
        let length = self.u32()? as usize;
        let position = self.position;
        Ok(Self::new(self.take(length)?, position))
    }

    pub(crate) fn name(&mut self) -> Result<&'a str, BinaryError> {
        let span = self.sized_span()?;
        std::str::from_utf8(span.rest).map_err(|error| BinaryError {
            offset: span.position + error.valid_up_to(),
            kind: BinaryErrorKind::InvalidName,
        })
    }

    pub(crate) fn finish(self) -> Result<(), BinaryError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(self.error(BinaryErrorKind::TrailingBytes))
        }
    }
}
