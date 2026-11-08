//! CSSOM serialization helpers (CSSOM §2.1, "Common Serializing Idioms").

use std::fmt::{self, Write};

use crate::{BlockKind, ComponentValue, Token, Value};

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

fn is_separator(value: &Value) -> bool {
    matches!(
        value.token(),
        Some(
            Token::Whitespace
                | Token::Colon
                | Token::Semicolon
                | Token::Comma
                | Token::OpenSquare
                | Token::CloseSquare
                | Token::OpenParen
                | Token::CloseParen
                | Token::OpenCurly
                | Token::CloseCurly
        )
    )
}

/// Serialize component values so they reparse to the same values (up to
/// whitespace runs, which become one space, and `url()` tokens, which become
/// `url("...")` functions). An empty comment separates adjacent values that
/// could otherwise merge into one token. Bad strings and bad URLs have no
/// serialization and must be rejected by the caller.
pub(crate) fn write_values(out: &mut impl Write, values: &[Value]) -> fmt::Result {
    let mut previous: Option<&Value> = None;
    for value in values {
        if let Some(p) = previous {
            // Whitespace split by a comment would merge on reparse.
            if p.is_whitespace() && value.is_whitespace() {
                continue;
            }
            if !is_separator(p) && !is_separator(value) && !p.is_whitespace() {
                out.write_str("/**/")?;
            }
        }
        write_value(out, value)?;
        previous = Some(value);
    }
    Ok(())
}

fn write_value(out: &mut impl Write, value: &Value) -> fmt::Result {
    match &value.value {
        ComponentValue::Token(token) => write_token(out, token),
        ComponentValue::Function { name, arguments } => {
            serialize_identifier(out, name)?;
            out.write_char('(')?;
            write_values(out, arguments)?;
            out.write_char(')')
        }
        ComponentValue::Block { kind, contents } => {
            let (open, close) = match kind {
                BlockKind::Curly => ('{', '}'),
                BlockKind::Square => ('[', ']'),
                BlockKind::Paren => ('(', ')'),
            };
            out.write_char(open)?;
            write_values(out, contents)?;
            out.write_char(close)
        }
    }
}

fn write_unit(out: &mut impl Write, unit: &str) -> fmt::Result {
    // A unit starting with `e` and a digit (or a sign and a digit) would
    // read back as an exponent.
    let mut chars = unit.chars();
    let first = chars.next();
    let exponent_like = match (chars.next(), chars.next()) {
        (Some(d), _) if d.is_ascii_digit() => true,
        (Some('+' | '-'), Some(d)) => d.is_ascii_digit(),
        _ => false,
    };
    match first {
        Some(c @ ('e' | 'E')) if exponent_like => {
            escape_code_point(out, c)?;
            serialize_identifier_rest(out, &unit[1..])
        }
        _ => serialize_identifier(out, unit),
    }
}

/// Serialize identifier characters that follow an already written start.
fn serialize_identifier_rest(out: &mut impl Write, rest: &str) -> fmt::Result {
    for c in rest.chars() {
        match c {
            '\0' => out.write_char('\u{fffd}')?,
            '\u{1}'..='\u{1f}' | '\u{7f}' => escape_code_point(out, c)?,
            c if c >= '\u{80}' || c == '-' || c == '_' || c.is_ascii_alphanumeric() => {
                out.write_char(c)?;
            }
            c => {
                out.write_char('\\')?;
                out.write_char(c)?;
            }
        }
    }
    Ok(())
}

fn write_token(out: &mut impl Write, token: &Token) -> fmt::Result {
    match token {
        Token::Ident(name) => serialize_identifier(out, name),
        Token::Function(name) => {
            serialize_identifier(out, name)?;
            out.write_char('(')
        }
        Token::AtKeyword(name) => {
            out.write_char('@')?;
            serialize_identifier(out, name)
        }
        Token::Hash { value, .. } => {
            out.write_char('#')?;
            serialize_identifier_rest(out, value)
        }
        Token::String(s) => serialize_string(out, s),
        Token::Url(url) => {
            out.write_str("url(")?;
            serialize_string(out, url)?;
            out.write_char(')')
        }
        Token::BadString | Token::BadUrl => Err(fmt::Error),
        Token::Delim('\\') => out.write_str("\\\n"),
        Token::Delim(c) => out.write_char(*c),
        Token::Number(n) => out.write_str(&n.repr),
        Token::Percentage(n) => {
            out.write_str(&n.repr)?;
            out.write_char('%')
        }
        Token::Dimension(n, unit) => {
            out.write_str(&n.repr)?;
            write_unit(out, unit)
        }
        Token::Whitespace => out.write_char(' '),
        Token::Cdo => out.write_str("<!--"),
        Token::Cdc => out.write_str("-->"),
        Token::Colon => out.write_char(':'),
        Token::Semicolon => out.write_char(';'),
        Token::Comma => out.write_char(','),
        Token::OpenSquare => out.write_char('['),
        Token::CloseSquare => out.write_char(']'),
        Token::OpenParen => out.write_char('('),
        Token::CloseParen => out.write_char(')'),
        Token::OpenCurly => out.write_char('{'),
        Token::CloseCurly => out.write_char('}'),
    }
}
