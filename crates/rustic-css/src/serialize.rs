//! CSSOM serialization helpers (CSSOM §2.1, "Common Serializing Idioms").

use std::fmt::{self, Write};

fn escape_code_point(out: &mut impl Write, c: char) -> fmt::Result {
    write!(out, "\\{:x} ", u32::from(c))
}

/// Serialize an identifier so it reads back as the same `<ident-token>`.
///
/// ```
/// let mut out = String::new();
/// rustic_css::serialize_identifier(&mut out, "1st item").unwrap();
/// assert_eq!(out, "\\31 st\\ item");
/// ```
pub fn serialize_identifier(out: &mut impl Write, value: &str) -> fmt::Result {
    let mut chars = value.chars().peekable();
    let mut index = 0usize;
    let starts_with_dash = value.starts_with('-');
    while let Some(c) = chars.next() {
        match c {
            '\0' => out.write_char('\u{fffd}')?,
            '\u{1}'..='\u{1f}' | '\u{7f}' => escape_code_point(out, c)?,
            '0'..='9' if index == 0 || (index == 1 && starts_with_dash) => {
                escape_code_point(out, c)?;
            }
            '-' if index == 0 && chars.peek().is_none() => out.write_str("\\-")?,
            c if c >= '\u{80}' || c == '-' || c == '_' || c.is_ascii_alphanumeric() => {
                out.write_char(c)?;
            }
            c => {
                out.write_char('\\')?;
                out.write_char(c)?;
            }
        }
        index += 1;
    }
    Ok(())
}

/// Serialize a string as a double-quoted `<string-token>`.
///
/// ```
/// let mut out = String::new();
/// rustic_css::serialize_string(&mut out, "say \"hi\"").unwrap();
/// assert_eq!(out, r#""say \"hi\"""#);
/// ```
pub fn serialize_string(out: &mut impl Write, value: &str) -> fmt::Result {
    out.write_char('"')?;
    for c in value.chars() {
        match c {
            '\0' => out.write_char('\u{fffd}')?,
            '\u{1}'..='\u{1f}' | '\u{7f}' => escape_code_point(out, c)?,
            '"' | '\\' => {
                out.write_char('\\')?;
                out.write_char(c)?;
            }
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}
