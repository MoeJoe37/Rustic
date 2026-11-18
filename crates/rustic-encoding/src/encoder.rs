//! Encoders (Encoding Standard §8–§14), one scalar value at a time.

use crate::{Encoding, index, tables};

/// An instance of an encoding's encoder.
///
/// [`Encoder::encode_char`] is the encoder's handler in "fatal" mode: an
/// unmappable scalar value is returned to the caller, which decides how to
/// represent it (HTML forms write a character reference, URLs percent-encode
/// one) and continues with the same instance. [`Encoder::finish`] runs the
/// end-of-queue step, which only ISO-2022-JP uses (to return to ASCII).
#[derive(Clone, Debug)]
pub struct Encoder {
    encoding: Encoding,
    jp_state: JpState,
}

/// ISO-2022-JP encoder state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JpState {
    Ascii,
    Roman,
    Jis0208,
}

const TO_ASCII: [u8; 3] = [0x1B, 0x28, 0x42];
const TO_ROMAN: [u8; 3] = [0x1B, 0x28, 0x4A];
const TO_JIS0208: [u8; 3] = [0x1B, 0x24, 0x42];

/// The gb18030 encoder's side table for Private Use code points that
/// GB18030-2022 moved (kept for compatibility with GB18030-2005).
const GB18030_PUA: [(u32, [u8; 2]); 18] = [
    (0xE78D, [0xA6, 0xD9]),
    (0xE78E, [0xA6, 0xDA]),
    (0xE78F, [0xA6, 0xDB]),
    (0xE790, [0xA6, 0xDC]),
    (0xE791, [0xA6, 0xDD]),
    (0xE792, [0xA6, 0xDE]),
    (0xE793, [0xA6, 0xDF]),
    (0xE794, [0xA6, 0xEC]),
    (0xE795, [0xA6, 0xED]),
    (0xE796, [0xA6, 0xF3]),
    (0xE81E, [0xFE, 0x59]),
    (0xE826, [0xFE, 0x61]),
    (0xE82B, [0xFE, 0x66]),
    (0xE82C, [0xFE, 0x67]),
    (0xE832, [0xFE, 0x6D]),
    (0xE843, [0xFE, 0x7E]),
    (0xE854, [0xFE, 0x90]),
    (0xE864, [0xFE, 0xA0]),
];

impl Encoder {
    /// A new instance of the encoder of `encoding`'s output encoding
    /// (replacement and UTF-16BE/LE have no encoder and use UTF-8's).
    pub fn new(encoding: Encoding) -> Encoder {
        Encoder {
            encoding: encoding.output_encoding(),
            jp_state: JpState::Ascii,
        }
    }

    /// The encoding this encoder produces.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Encode one scalar value, appending its bytes to `output`. Returns the
    /// value to report when it cannot be encoded; ISO-2022-JP reports
    /// U+FFFD for U+000E, U+000F and U+001B, as the standard requires.
    pub fn encode_char(&mut self, character: char, output: &mut Vec<u8>) -> Result<(), char> {
        let code_point = u32::from(character);
        if let Some(table) = tables::single_byte_index(self.encoding) {
            if character.is_ascii() {
                output.push(code_point as u8);
                return Ok(());
            }
            let pointer = table
                .iter()
                .position(|&entry| entry != 0 && u32::from(entry) == code_point);
            return match pointer {
                Some(pointer) => {
                    output.push(pointer as u8 + 0x80);
                    Ok(())
                }
                None => Err(character),
            };
        }
        match self.encoding {
            Encoding::Gbk | Encoding::Gb18030 => {
                gb18030(code_point, self.encoding == Encoding::Gbk, output).ok_or(character)
            }
            Encoding::Big5 => big5(code_point, output).ok_or(character),
            Encoding::EucJp => euc_jp(code_point, output).ok_or(character),
            Encoding::Iso2022Jp => self.iso_2022_jp(character, output),
            Encoding::ShiftJis => shift_jis(code_point, output).ok_or(character),
            Encoding::EucKr => euc_kr(code_point, output).ok_or(character),
            Encoding::XUserDefined => match code_point {
                0x00..=0x7F => {
                    output.push(code_point as u8);
                    Ok(())
                }
                0xF780..=0xF7FF => {
                    output.push((code_point - 0xF780 + 0x80) as u8);
                    Ok(())
                }
                _ => Err(character),
            },
            // UTF-8, the output encoding of everything else.
            _ => {
                let mut buffer = [0; 4];
                output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                Ok(())
            }
        }
    }

    /// Run the end-of-queue step: ISO-2022-JP returns to the ASCII state.
    /// The encoder can then encode a new stream.
    pub fn finish(&mut self, output: &mut Vec<u8>) {
        if self.jp_state != JpState::Ascii {
            self.jp_state = JpState::Ascii;
            output.extend_from_slice(&TO_ASCII);
        }
    }

    /// Whether [`Encoder::finish`] would write bytes.
    pub fn has_pending_state(&self) -> bool {
        self.jp_state != JpState::Ascii
    }

    fn iso_2022_jp(&mut self, character: char, output: &mut Vec<u8>) -> Result<(), char> {
        let mut code_point = u32::from(character);
        // Each "restore codePoint to ioQueue" below is a state switch followed by
        // another pass; at most two switches happen per scalar value.
        loop {
            let state = self.jp_state;
            if matches!(state, JpState::Ascii | JpState::Roman)
                && matches!(code_point, 0x0E | 0x0F | 0x1B)
            {
                return Err('\u{FFFD}');
            }
            let ascii = code_point < 0x80;
            if state == JpState::Ascii && ascii {
                output.push(code_point as u8);
                return Ok(());
            }
            if state == JpState::Roman
                && ((ascii && code_point != 0x5C && code_point != 0x7E)
                    || matches!(code_point, 0xA5 | 0x203E))
            {
                output.push(match code_point {
                    0xA5 => 0x5C,
                    0x203E => 0x7E,
                    _ => code_point as u8,
                });
                return Ok(());
            }
            if ascii {
                self.jp_state = JpState::Ascii;
                output.extend_from_slice(&TO_ASCII);
                continue;
            }
            if matches!(code_point, 0xA5 | 0x203E) {
                self.jp_state = JpState::Roman;
                output.extend_from_slice(&TO_ROMAN);
                continue;
            }
            if code_point == 0x2212 {
                code_point = 0xFF0D;
            }
            if (0xFF61..=0xFF9F).contains(&code_point) {
                code_point =
                    u32::from(tables::ISO_2022_JP_KATAKANA[(code_point - 0xFF61) as usize]);
            }
            let Some(pointer) = index::jis0208_pointer(code_point) else {
                if state == JpState::Jis0208 {
                    self.jp_state = JpState::Ascii;
                    output.extend_from_slice(&TO_ASCII);
                }
                return Err(character);
            };
            if state != JpState::Jis0208 {
                self.jp_state = JpState::Jis0208;
                output.extend_from_slice(&TO_JIS0208);
            }
            output.push((pointer / 94 + 0x21) as u8);
            output.push((pointer % 94 + 0x21) as u8);
            return Ok(());
        }
    }
}

fn gb18030(code_point: u32, is_gbk: bool, output: &mut Vec<u8>) -> Option<()> {
    if code_point < 0x80 {
        output.push(code_point as u8);
        return Some(());
    }
    if code_point == 0xE5E5 {
        return None;
    }
    if is_gbk && code_point == 0x20AC {
        output.push(0x80);
        return Some(());
    }
    if let Some((_, bytes)) = GB18030_PUA.iter().find(|(entry, _)| *entry == code_point) {
        output.extend_from_slice(bytes);
        return Some(());
    }
    if let Some(pointer) = index::gb18030_pointer(code_point) {
        let trailing = pointer % 190;
        let offset = if trailing < 0x3F { 0x40 } else { 0x41 };
        output.push((pointer / 190 + 0x81) as u8);
        output.push((trailing + offset) as u8);
        return Some(());
    }
    if is_gbk {
        return None;
    }
    let mut pointer = index::gb18030_ranges_pointer(code_point);
    let byte1 = pointer / (10 * 126 * 10);
    pointer %= 10 * 126 * 10;
    let byte2 = pointer / (10 * 126);
    pointer %= 10 * 126;
    let byte3 = pointer / 10;
    let byte4 = pointer % 10;
    output.extend_from_slice(&[
        (byte1 + 0x81) as u8,
        (byte2 + 0x30) as u8,
        (byte3 + 0x81) as u8,
        (byte4 + 0x30) as u8,
    ]);
    Some(())
}

fn big5(code_point: u32, output: &mut Vec<u8>) -> Option<()> {
    if code_point < 0x80 {
        output.push(code_point as u8);
        return Some(());
    }
    let pointer = index::big5_pointer(code_point)?;
    let trailing = pointer % 157;
    let offset = if trailing < 0x3F { 0x40 } else { 0x62 };
    output.push((pointer / 157 + 0x81) as u8);
    output.push((trailing + offset) as u8);
    Some(())
}

fn euc_jp(code_point: u32, output: &mut Vec<u8>) -> Option<()> {
    match code_point {
        0x00..=0x7F => output.push(code_point as u8),
        0xA5 => output.push(0x5C),
        0x203E => output.push(0x7E),
        0xFF61..=0xFF9F => output.extend_from_slice(&[0x8E, (code_point - 0xFF61 + 0xA1) as u8]),
        _ => {
            let code_point = if code_point == 0x2212 {
                0xFF0D
            } else {
                code_point
            };
            let pointer = index::jis0208_pointer(code_point)?;
            output.push((pointer / 94 + 0xA1) as u8);
            output.push((pointer % 94 + 0xA1) as u8);
        }
    }
    Some(())
}

fn shift_jis(code_point: u32, output: &mut Vec<u8>) -> Option<()> {
    match code_point {
        0x00..=0x80 => output.push(code_point as u8),
        0xA5 => output.push(0x5C),
        0x203E => output.push(0x7E),
        0xFF61..=0xFF9F => output.push((code_point - 0xFF61 + 0xA1) as u8),
        _ => {
            let code_point = if code_point == 0x2212 {
                0xFF0D
            } else {
                code_point
            };
            let pointer = index::shift_jis_pointer(code_point)?;
            let leading = pointer / 188;
            let leading_offset = if leading < 0x1F { 0x81 } else { 0xC1 };
            let trailing = pointer % 188;
            let offset = if trailing < 0x3F { 0x40 } else { 0x41 };
            output.push((leading + leading_offset) as u8);
            output.push((trailing + offset) as u8);
        }
    }
    Some(())
}

fn euc_kr(code_point: u32, output: &mut Vec<u8>) -> Option<()> {
    if code_point < 0x80 {
        output.push(code_point as u8);
        return Some(());
    }
    let pointer = index::euc_kr_pointer(code_point)?;
    output.push((pointer / 190 + 0x81) as u8);
    output.push((pointer % 190 + 0x41) as u8);
    Some(())
}
