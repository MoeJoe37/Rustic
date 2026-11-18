//! Decoders (Encoding Standard §8–§14) behind one streaming driver.
//!
//! Each decoder is the standard's handler algorithm: it receives one byte or
//! end-of-queue and returns continue, one or two code points, an error or
//! finished. "Restore to ioQueue" pushes bytes onto a small queue that the
//! driver reads before the next input byte, so chunk boundaries never change
//! the result.

use std::collections::VecDeque;

use crate::{Encoding, index, tables};

/// A decoder's error mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DecoderErrorMode {
    /// Each error becomes U+FFFD and decoding continues.
    Replacement,
    /// The first error stops decoding.
    Fatal,
}

/// A decoder error in [`DecoderErrorMode::Fatal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecodeError {
    /// The encoding whose decoder failed.
    pub encoding: Encoding,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "invalid {} input", self.encoding.name())
    }
}

impl std::error::Error for DecodeError {}

/// What a handler returns for one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Continue,
    Emit(u32),
    Emit2(u32, u32),
    Error,
    Finished,
}

/// An instance of an encoding's decoder.
///
/// Feed input with [`Decoder::decode`] in any number of chunks; pass
/// `last = true` with the final chunk (which may be empty) so incomplete
/// sequences at the end are reported. A decoder that finished or failed is
/// reset and may decode a new stream.
#[derive(Clone, Debug)]
pub struct Decoder {
    encoding: Encoding,
    mode: DecoderErrorMode,
    state: State,
    queue: VecDeque<u8>,
    had_errors: bool,
}

#[derive(Clone, Debug)]
enum State {
    Utf8 {
        code_point: u32,
        seen: u8,
        needed: u8,
        lower: u8,
        upper: u8,
    },
    SingleByte(&'static [u16; 128]),
    Gb18030 {
        first: u8,
        second: u8,
        third: u8,
    },
    Big5 {
        leading: u8,
    },
    EucJp {
        jis0212: bool,
        leading: u8,
    },
    Iso2022Jp(Iso2022JpDecoder),
    ShiftJis {
        leading: u8,
    },
    EucKr {
        leading: u8,
    },
    Replacement {
        error_returned: bool,
    },
    Utf16 {
        big_endian: bool,
        leading_byte: Option<u8>,
        leading_surrogate: Option<u16>,
    },
    XUserDefined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JpState {
    Ascii,
    Roman,
    Katakana,
    LeadingByte,
    TrailingByte,
    EscapeStart,
    Escape,
}

#[derive(Clone, Debug)]
struct Iso2022JpDecoder {
    state: JpState,
    output_state: JpState,
    leading: u8,
    output: bool,
}

impl State {
    fn new(encoding: Encoding) -> State {
        if let Some(index) = tables::single_byte_index(encoding) {
            return State::SingleByte(index);
        }
        match encoding {
            Encoding::Gbk | Encoding::Gb18030 => State::Gb18030 {
                first: 0,
                second: 0,
                third: 0,
            },
            Encoding::Big5 => State::Big5 { leading: 0 },
            Encoding::EucJp => State::EucJp {
                jis0212: false,
                leading: 0,
            },
            Encoding::Iso2022Jp => State::Iso2022Jp(Iso2022JpDecoder {
                state: JpState::Ascii,
                output_state: JpState::Ascii,
                leading: 0,
                output: false,
            }),
            Encoding::ShiftJis => State::ShiftJis { leading: 0 },
            Encoding::EucKr => State::EucKr { leading: 0 },
            Encoding::Replacement => State::Replacement {
                error_returned: false,
            },
            Encoding::Utf16Be | Encoding::Utf16Le => State::Utf16 {
                big_endian: encoding == Encoding::Utf16Be,
                leading_byte: None,
                leading_surrogate: None,
            },
            Encoding::XUserDefined => State::XUserDefined,
            // UTF-8 and (unreachably) the single-byte encodings handled above.
            _ => State::Utf8 {
                code_point: 0,
                seen: 0,
                needed: 0,
                lower: 0x80,
                upper: 0xBF,
            },
        }
    }

    /// Whether the next byte, if ASCII, decodes to itself with no state
    /// change. Lets the driver copy ASCII runs without the per-byte handler.
    fn ascii_passthrough(&self) -> bool {
        match self {
            State::Utf8 { needed, .. } => *needed == 0,
            State::SingleByte(_) | State::XUserDefined => true,
            State::Gb18030 { first, .. } => *first == 0,
            State::Big5 { leading }
            | State::ShiftJis { leading }
            | State::EucKr { leading }
            | State::EucJp { leading, .. } => *leading == 0,
            State::Iso2022Jp(_) | State::Replacement { .. } | State::Utf16 { .. } => false,
        }
    }

    fn handle(&mut self, byte: Option<u8>, queue: &mut VecDeque<u8>) -> Step {
        match self {
            State::Utf8 {
                code_point,
                seen,
                needed,
                lower,
                upper,
            } => {
                let Some(byte) = byte else {
                    if *needed != 0 {
                        *needed = 0;
                        return Step::Error;
                    }
                    return Step::Finished;
                };
                if *needed == 0 {
                    match byte {
                        0x00..=0x7F => return Step::Emit(u32::from(byte)),
                        0xC2..=0xDF => {
                            *needed = 1;
                            *code_point = u32::from(byte & 0x1F);
                        }
                        0xE0..=0xEF => {
                            if byte == 0xE0 {
                                *lower = 0xA0;
                            }
                            if byte == 0xED {
                                *upper = 0x9F;
                            }
                            *needed = 2;
                            *code_point = u32::from(byte & 0xF);
                        }
                        0xF0..=0xF4 => {
                            if byte == 0xF0 {
                                *lower = 0x90;
                            }
                            if byte == 0xF4 {
                                *upper = 0x8F;
                            }
                            *needed = 3;
                            *code_point = u32::from(byte & 0x7);
                        }
                        _ => return Step::Error,
                    }
                    return Step::Continue;
                }
                if !(*lower..=*upper).contains(&byte) {
                    *code_point = 0;
                    *needed = 0;
                    *seen = 0;
                    *lower = 0x80;
                    *upper = 0xBF;
                    queue.push_front(byte);
                    return Step::Error;
                }
                *lower = 0x80;
                *upper = 0xBF;
                *code_point = (*code_point << 6) | u32::from(byte & 0x3F);
                *seen += 1;
                if seen != needed {
                    return Step::Continue;
                }
                let result = *code_point;
                *code_point = 0;
                *needed = 0;
                *seen = 0;
                Step::Emit(result)
            }
            State::SingleByte(table) => match byte {
                None => Step::Finished,
                Some(byte @ 0x00..=0x7F) => Step::Emit(u32::from(byte)),
                Some(byte) => match table[usize::from(byte - 0x80)] {
                    0 => Step::Error,
                    code_point => Step::Emit(u32::from(code_point)),
                },
            },
            State::Gb18030 {
                first,
                second,
                third,
            } => gb18030(first, second, third, byte, queue),
            State::Big5 { leading } => big5(leading, byte, queue),
            State::EucJp { jis0212, leading } => euc_jp(jis0212, leading, byte, queue),
            State::Iso2022Jp(decoder) => decoder.handle(byte, queue),
            State::ShiftJis { leading } => shift_jis(leading, byte, queue),
            State::EucKr { leading } => euc_kr(leading, byte, queue),
            State::Replacement { error_returned } => {
                if byte.is_none() {
                    return Step::Finished;
                }
                if *error_returned {
                    // The standard returns finished here, which ends the queue;
                    // ignoring the rest of the stream has the same output and
                    // keeps later chunks from producing another error.
                    return Step::Continue;
                }
                *error_returned = true;
                Step::Error
            }
            State::Utf16 {
                big_endian,
                leading_byte,
                leading_surrogate,
            } => utf16(*big_endian, leading_byte, leading_surrogate, byte, queue),
            State::XUserDefined => match byte {
                None => Step::Finished,
                Some(byte @ 0x00..=0x7F) => Step::Emit(u32::from(byte)),
                Some(byte) => Step::Emit(0xF780 + u32::from(byte) - 0x80),
            },
        }
    }
}

fn gb18030(
    first: &mut u8,
    second: &mut u8,
    third: &mut u8,
    byte: Option<u8>,
    queue: &mut VecDeque<u8>,
) -> Step {
    let Some(byte) = byte else {
        if *first == 0 && *second == 0 && *third == 0 {
            return Step::Finished;
        }
        (*first, *second, *third) = (0, 0, 0);
        return Step::Error;
    };
    if *third != 0 {
        if !(0x30..=0x39).contains(&byte) {
            restore(queue, &[*second, *third, byte]);
            (*first, *second, *third) = (0, 0, 0);
            return Step::Error;
        }
        let pointer = (u32::from(*first) - 0x81) * (10 * 126 * 10)
            + (u32::from(*second) - 0x30) * (10 * 126)
            + (u32::from(*third) - 0x81) * 10
            + u32::from(byte)
            - 0x30;
        (*first, *second, *third) = (0, 0, 0);
        return match index::gb18030_ranges_code_point(pointer) {
            Some(code_point) => Step::Emit(code_point),
            None => Step::Error,
        };
    }
    if *second != 0 {
        if (0x81..=0xFE).contains(&byte) {
            *third = byte;
            return Step::Continue;
        }
        restore(queue, &[*second, byte]);
        (*first, *second) = (0, 0);
        return Step::Error;
    }
    if *first != 0 {
        if (0x30..=0x39).contains(&byte) {
            *second = byte;
            return Step::Continue;
        }
        let leading = usize::from(*first);
        *first = 0;
        let offset = if byte < 0x7F { 0x40 } else { 0x41 };
        let pointer = matches!(byte, 0x40..=0x7E | 0x80..=0xFE)
            .then(|| (leading - 0x81) * 190 + (usize::from(byte) - offset));
        if let Some(code_point) = pointer.and_then(index::gb18030) {
            return Step::Emit(code_point);
        }
        return restore_ascii_then_error(queue, byte);
    }
    match byte {
        0x00..=0x7F => Step::Emit(u32::from(byte)),
        0x80 => Step::Emit(0x20AC),
        0x81..=0xFE => {
            *first = byte;
            Step::Continue
        }
        0xFF => Step::Error,
    }
}

fn big5(leading_byte: &mut u8, byte: Option<u8>, queue: &mut VecDeque<u8>) -> Step {
    let Some(byte) = byte else {
        if *leading_byte != 0 {
            *leading_byte = 0;
            return Step::Error;
        }
        return Step::Finished;
    };
    if *leading_byte != 0 {
        let leading = usize::from(*leading_byte);
        *leading_byte = 0;
        let offset = if byte < 0x7F { 0x40 } else { 0x62 };
        let pointer = matches!(byte, 0x40..=0x7E | 0xA1..=0xFE)
            .then(|| (leading - 0x81) * 157 + (usize::from(byte) - offset));
        match pointer {
            Some(1133) => return Step::Emit2(0x00CA, 0x0304),
            Some(1135) => return Step::Emit2(0x00CA, 0x030C),
            Some(1164) => return Step::Emit2(0x00EA, 0x0304),
            Some(1166) => return Step::Emit2(0x00EA, 0x030C),
            _ => {}
        }
        if let Some(code_point) = pointer.and_then(index::big5) {
            return Step::Emit(code_point);
        }
        return restore_ascii_then_error(queue, byte);
    }
    match byte {
        0x00..=0x7F => Step::Emit(u32::from(byte)),
        0x81..=0xFE => {
            *leading_byte = byte;
            Step::Continue
        }
        _ => Step::Error,
    }
}

fn euc_jp(
    jis0212: &mut bool,
    leading_byte: &mut u8,
    byte: Option<u8>,
    queue: &mut VecDeque<u8>,
) -> Step {
    let Some(byte) = byte else {
        if *leading_byte != 0 {
            *leading_byte = 0;
            return Step::Error;
        }
        return Step::Finished;
    };
    if *leading_byte == 0x8E && (0xA1..=0xDF).contains(&byte) {
        *leading_byte = 0;
        return Step::Emit(0xFF61 - 0xA1 + u32::from(byte));
    }
    if *leading_byte == 0x8F && (0xA1..=0xFE).contains(&byte) {
        *jis0212 = true;
        *leading_byte = byte;
        return Step::Continue;
    }
    if *leading_byte != 0 {
        let leading = *leading_byte;
        *leading_byte = 0;
        let mut code_point = None;
        if (0xA1..=0xFE).contains(&leading) && (0xA1..=0xFE).contains(&byte) {
            let pointer = (usize::from(leading) - 0xA1) * 94 + usize::from(byte) - 0xA1;
            code_point = if *jis0212 {
                index::jis0212(pointer)
            } else {
                index::jis0208(pointer)
            };
        }
        *jis0212 = false;
        if let Some(code_point) = code_point {
            return Step::Emit(code_point);
        }
        return restore_ascii_then_error(queue, byte);
    }
    match byte {
        0x00..=0x7F => Step::Emit(u32::from(byte)),
        0x8E | 0x8F | 0xA1..=0xFE => {
            *leading_byte = byte;
            Step::Continue
        }
        _ => Step::Error,
    }
}

impl Iso2022JpDecoder {
    fn handle(&mut self, byte: Option<u8>, queue: &mut VecDeque<u8>) -> Step {
        match self.state {
            JpState::Ascii | JpState::Roman | JpState::Katakana | JpState::LeadingByte => {
                let Some(byte) = byte else {
                    return Step::Finished;
                };
                if byte == 0x1B {
                    self.state = JpState::EscapeStart;
                    return Step::Continue;
                }
                self.output = false;
                match (self.state, byte) {
                    (JpState::Ascii, 0x00..=0x7F) if !matches!(byte, 0x0E | 0x0F) => {
                        Step::Emit(u32::from(byte))
                    }
                    (JpState::Roman, 0x5C) => Step::Emit(0x00A5),
                    (JpState::Roman, 0x7E) => Step::Emit(0x203E),
                    (JpState::Roman, 0x00..=0x7F) if !matches!(byte, 0x0E | 0x0F) => {
                        Step::Emit(u32::from(byte))
                    }
                    (JpState::Katakana, 0x21..=0x5F) => Step::Emit(0xFF61 - 0x21 + u32::from(byte)),
                    (JpState::LeadingByte, 0x21..=0x7E) => {
                        self.leading = byte;
                        self.state = JpState::TrailingByte;
                        Step::Continue
                    }
                    _ => Step::Error,
                }
            }
            JpState::TrailingByte => match byte {
                Some(0x1B) => {
                    self.state = JpState::EscapeStart;
                    Step::Error
                }
                Some(byte @ 0x21..=0x7E) => {
                    self.state = JpState::LeadingByte;
                    let pointer =
                        (usize::from(self.leading) - 0x21) * 94 + usize::from(byte) - 0x21;
                    match index::jis0208(pointer) {
                        Some(code_point) => Step::Emit(code_point),
                        None => Step::Error,
                    }
                }
                _ => {
                    self.state = JpState::LeadingByte;
                    Step::Error
                }
            },
            JpState::EscapeStart => {
                if let Some(byte @ (0x24 | 0x28)) = byte {
                    self.leading = byte;
                    self.state = JpState::Escape;
                    return Step::Continue;
                }
                if let Some(byte) = byte {
                    queue.push_front(byte);
                }
                self.output = false;
                self.state = self.output_state;
                Step::Error
            }
            JpState::Escape => {
                let leading = self.leading;
                self.leading = 0;
                let state = match (leading, byte) {
                    (0x28, Some(0x42)) => Some(JpState::Ascii),
                    (0x28, Some(0x4A)) => Some(JpState::Roman),
                    (0x28, Some(0x49)) => Some(JpState::Katakana),
                    (0x24, Some(0x40 | 0x42)) => Some(JpState::LeadingByte),
                    _ => None,
                };
                if let Some(state) = state {
                    self.state = state;
                    self.output_state = state;
                    let output = self.output;
                    self.output = true;
                    return if output { Step::Error } else { Step::Continue };
                }
                match byte {
                    None => queue.push_front(leading),
                    Some(byte) => restore(queue, &[leading, byte]),
                }
                self.output = false;
                self.state = self.output_state;
                Step::Error
            }
        }
    }
}

fn shift_jis(leading_byte: &mut u8, byte: Option<u8>, queue: &mut VecDeque<u8>) -> Step {
    let Some(byte) = byte else {
        if *leading_byte != 0 {
            *leading_byte = 0;
            return Step::Error;
        }
        return Step::Finished;
    };
    if *leading_byte != 0 {
        let leading = usize::from(*leading_byte);
        *leading_byte = 0;
        let offset = if byte < 0x7F { 0x40 } else { 0x41 };
        let leading_offset = if leading < 0xA0 { 0x81 } else { 0xC1 };
        let pointer = matches!(byte, 0x40..=0x7E | 0x80..=0xFC)
            .then(|| (leading - leading_offset) * 188 + usize::from(byte) - offset);
        if let Some(pointer @ 8836..=10715) = pointer {
            // Windows end-user-defined characters map to the Private Use Area.
            return Step::Emit(0xE000 - 8836 + pointer as u32);
        }
        if let Some(code_point) = pointer.and_then(index::jis0208) {
            return Step::Emit(code_point);
        }
        return restore_ascii_then_error(queue, byte);
    }
    match byte {
        0x00..=0x80 => Step::Emit(u32::from(byte)),
        0xA1..=0xDF => Step::Emit(0xFF61 - 0xA1 + u32::from(byte)),
        0x81..=0x9F | 0xE0..=0xFC => {
            *leading_byte = byte;
            Step::Continue
        }
        _ => Step::Error,
    }
}

fn euc_kr(leading_byte: &mut u8, byte: Option<u8>, queue: &mut VecDeque<u8>) -> Step {
    let Some(byte) = byte else {
        if *leading_byte != 0 {
            *leading_byte = 0;
            return Step::Error;
        }
        return Step::Finished;
    };
    if *leading_byte != 0 {
        let leading = usize::from(*leading_byte);
        *leading_byte = 0;
        let pointer = (0x41..=0xFE)
            .contains(&byte)
            .then(|| (leading - 0x81) * 190 + (usize::from(byte) - 0x41));
        if let Some(code_point) = pointer.and_then(index::euc_kr) {
            return Step::Emit(code_point);
        }
        return restore_ascii_then_error(queue, byte);
    }
    match byte {
        0x00..=0x7F => Step::Emit(u32::from(byte)),
        0x81..=0xFE => {
            *leading_byte = byte;
            Step::Continue
        }
        _ => Step::Error,
    }
}

fn utf16(
    big_endian: bool,
    leading_byte: &mut Option<u8>,
    leading_surrogate: &mut Option<u16>,
    byte: Option<u8>,
    queue: &mut VecDeque<u8>,
) -> Step {
    let Some(byte) = byte else {
        if leading_byte.is_some() || leading_surrogate.is_some() {
            *leading_byte = None;
            *leading_surrogate = None;
            return Step::Error;
        }
        return Step::Finished;
    };
    let Some(first) = leading_byte.take() else {
        *leading_byte = Some(byte);
        return Step::Continue;
    };
    let code_unit = if big_endian {
        u16::from_be_bytes([first, byte])
    } else {
        u16::from_le_bytes([first, byte])
    };
    if let Some(leading) = leading_surrogate.take() {
        if (0xDC00..=0xDFFF).contains(&code_unit) {
            let scalar =
                0x10000 + ((u32::from(leading) - 0xD800) << 10) + (u32::from(code_unit) - 0xDC00);
            return Step::Emit(scalar);
        }
        let bytes = if big_endian {
            code_unit.to_be_bytes()
        } else {
            code_unit.to_le_bytes()
        };
        restore(queue, &bytes);
        return Step::Error;
    }
    match code_unit {
        0xD800..=0xDBFF => {
            *leading_surrogate = Some(code_unit);
            Step::Continue
        }
        0xDC00..=0xDFFF => Step::Error,
        _ => Step::Emit(u32::from(code_unit)),
    }
}

/// "Restore `bytes` to ioQueue": they are read next, in order.
fn restore(queue: &mut VecDeque<u8>, bytes: &[u8]) {
    for &byte in bytes.iter().rev() {
        queue.push_front(byte);
    }
}

/// The multi-byte decoders' shared tail: "If byte is an ASCII byte, restore
/// byte to ioQueue. Return error."
fn restore_ascii_then_error(queue: &mut VecDeque<u8>, byte: u8) -> Step {
    if byte.is_ascii() {
        queue.push_front(byte);
    }
    Step::Error
}

impl Decoder {
    /// A new instance of `encoding`'s decoder.
    pub fn new(encoding: Encoding, mode: DecoderErrorMode) -> Decoder {
        Decoder {
            encoding,
            mode,
            state: State::new(encoding),
            queue: VecDeque::new(),
            had_errors: false,
        }
    }

    /// The encoding this decoder decodes.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Whether an error was replaced with U+FFFD since the decoder was
    /// created.
    pub fn had_errors(&self) -> bool {
        self.had_errors
    }

    /// Decode `input`, appending to `output`. Pass `last = true` with the
    /// final chunk. In fatal mode the first error returns `Err`, the text
    /// decoded before it stays in `output`, and the decoder resets.
    pub fn decode(
        &mut self,
        input: &[u8],
        last: bool,
        output: &mut String,
    ) -> Result<(), DecodeError> {
        let mut position = 0;
        loop {
            if self.queue.is_empty() && position < input.len() && self.state.ascii_passthrough() {
                position += self.copy_run(&input[position..], output);
            }
            let byte = if let Some(byte) = self.queue.pop_front() {
                Some(byte)
            } else if let Some(&byte) = input.get(position) {
                position += 1;
                Some(byte)
            } else if last {
                None
            } else {
                return Ok(());
            };
            match self.state.handle(byte, &mut self.queue) {
                Step::Continue => {}
                Step::Emit(code_point) => output.push(scalar(code_point)),
                Step::Emit2(first, second) => {
                    output.push(scalar(first));
                    output.push(scalar(second));
                }
                Step::Error => match self.mode {
                    DecoderErrorMode::Replacement => {
                        self.had_errors = true;
                        output.push('\u{FFFD}');
                    }
                    DecoderErrorMode::Fatal => {
                        self.reset();
                        return Err(DecodeError {
                            encoding: self.encoding,
                        });
                    }
                },
                Step::Finished => {
                    self.reset();
                    return Ok(());
                }
            }
        }
    }

    /// Copy the run of bytes that decode to themselves (ASCII, or for UTF-8
    /// any valid prefix) and return its length.
    fn copy_run(&self, input: &[u8], output: &mut String) -> usize {
        let length = if matches!(self.state, State::Utf8 { .. }) {
            match core::str::from_utf8(input) {
                Ok(_) => input.len(),
                Err(error) => error.valid_up_to(),
            }
        } else {
            input
                .iter()
                .position(|byte| !byte.is_ascii())
                .unwrap_or(input.len())
        };
        // The prefix is valid UTF-8 (ASCII in the second case).
        if let Ok(text) = core::str::from_utf8(&input[..length]) {
            output.push_str(text);
        }
        length
    }

    fn reset(&mut self) {
        self.state = State::new(self.encoding);
        self.queue.clear();
    }
}

/// Every decoder emits scalar values; anything else is a table bug, which
/// shows as U+FFFD rather than a panic.
fn scalar(code_point: u32) -> char {
    char::from_u32(code_point).unwrap_or('\u{FFFD}')
}
