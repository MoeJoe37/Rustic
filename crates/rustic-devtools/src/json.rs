//! A small, bounded JSON reader and writer for protocol lines.
//!
//! The reader accepts RFC 8259 JSON with limits on nesting and length,
//! rejects duplicate object keys, lone surrogate escapes and trailing
//! bytes, and never panics on input. Numbers are `f64`; protocol ids and
//! integers are checked when read ([`Value::as_u64`]).

use std::fmt::Write as _;

/// Deepest nesting of arrays and objects a line may use.
pub const MAX_DEPTH: usize = 32;

/// A JSON value. Objects keep their members in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A finite number.
    Number(f64),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object, members in source order, keys unique.
    Object(Vec<(String, Value)>),
}

/// Why a line is not JSON this reader accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonError {
    /// Not well-formed JSON at this byte offset.
    Syntax(usize),
    /// Nested deeper than [`MAX_DEPTH`].
    TooDeep,
    /// An object names the same key twice.
    DuplicateKey,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonError::Syntax(at) => write!(f, "invalid JSON at byte {at}"),
            JsonError::TooDeep => write!(f, "JSON nested deeper than {MAX_DEPTH}"),
            JsonError::DuplicateKey => f.write_str("duplicate object key"),
        }
    }
}

impl std::error::Error for JsonError {}

impl Value {
    /// Parse one complete JSON text.
    pub fn parse(text: &str) -> Result<Value, JsonError> {
        let mut reader = Reader {
            bytes: text.as_bytes(),
            at: 0,
        };
        reader.space();
        let value = reader.value(0)?;
        reader.space();
        if reader.at != reader.bytes.len() {
            return Err(JsonError::Syntax(reader.at));
        }
        Ok(value)
    }

    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// A string's text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    /// A boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// A number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// A non-negative integer no larger than 2^53 - 1.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(value)
                if value.fract() == 0.0 && (0.0..=9_007_199_254_740_991.0).contains(value) =>
            {
                Some(*value as u64)
            }
            _ => None,
        }
    }

    /// An array's items.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// A string value.
    pub fn string(text: impl Into<String>) -> Value {
        Value::String(text.into())
    }

    /// A number value from an integer.
    pub fn int(value: impl Into<i64>) -> Value {
        Value::Number(value.into() as f64)
    }

    /// An object from members.
    pub fn object<K: Into<String>>(members: impl IntoIterator<Item = (K, Value)>) -> Value {
        Value::Object(members.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Serialize on one line: no raw control characters, line or
    /// paragraph separators appear in the output, so a value can never
    /// break the one-message-per-line framing.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Number(value) => {
                if !value.is_finite() {
                    out.push_str("null");
                } else if value.fract() == 0.0 && value.abs() < 1e15 {
                    let _ = write!(out, "{}", *value as i64);
                } else {
                    let _ = write!(out, "{value}");
                }
            }
            Value::String(text) => write_string(text, out),
            Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Value::Object(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn error<T>(&self) -> Result<T, JsonError> {
        Err(JsonError::Syntax(self.at))
    }

    fn space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn literal(&mut self, word: &[u8], value: Value) -> Result<Value, JsonError> {
        if self.bytes[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            self.error()
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsonError> {
        match self.bytes.get(self.at) {
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => {
                if depth >= MAX_DEPTH {
                    return Err(JsonError::TooDeep);
                }
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                loop {
                    self.space();
                    items.push(self.value(depth + 1)?);
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Value::Array(items));
                        }
                        _ => return self.error(),
                    }
                }
            }
            Some(b'{') => {
                if depth >= MAX_DEPTH {
                    return Err(JsonError::TooDeep);
                }
                self.at += 1;
                let mut members: Vec<(String, Value)> = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Value::Object(members));
                }
                loop {
                    self.space();
                    if self.bytes.get(self.at) != Some(&b'"') {
                        return self.error();
                    }
                    let key = self.string()?;
                    if members.iter().any(|(k, _)| *k == key) {
                        return Err(JsonError::DuplicateKey);
                    }
                    self.space();
                    if self.bytes.get(self.at) != Some(&b':') {
                        return self.error();
                    }
                    self.at += 1;
                    self.space();
                    let value = self.value(depth + 1)?;
                    members.push((key, value));
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Value::Object(members));
                        }
                        _ => return self.error(),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => self.error(),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.at - start
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        match self.bytes.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return self.error(),
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return self.error();
            }
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.bytes.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return self.error();
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| JsonError::Syntax(start))?;
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(Value::Number(value)),
            _ => Err(JsonError::Syntax(start)),
        }
    }

    fn hex4(&mut self) -> Result<u16, JsonError> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or(JsonError::Syntax(self.at))?;
        let mut value = 0u16;
        for &digit in digits {
            let nibble = (digit as char)
                .to_digit(16)
                .ok_or(JsonError::Syntax(self.at))?;
            value = value * 16 + nibble as u16;
        }
        self.at += 4;
        Ok(value)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.at += 1; // the opening quote
        let mut out = String::new();
        loop {
            let start = self.at;
            while let Some(&byte) = self.bytes.get(self.at) {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.at += 1;
            }
            // The input is a `str`, and a run ends only at ASCII bytes.
            out.push_str(
                std::str::from_utf8(&self.bytes[start..self.at])
                    .map_err(|_| JsonError::Syntax(start))?,
            );
            match self.bytes.get(self.at) {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = *self.bytes.get(self.at).ok_or(JsonError::Syntax(self.at))?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let high = self.hex4()?;
                            let c = if (0xD800..0xDC00).contains(&high) {
                                if !self.bytes[self.at..].starts_with(b"\\u") {
                                    return self.error();
                                }
                                self.at += 2;
                                let low = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&low) {
                                    return self.error();
                                }
                                let code = 0x10000
                                    + ((u32::from(high) - 0xD800) << 10)
                                    + (u32::from(low) - 0xDC00);
                                char::from_u32(code)
                            } else {
                                char::from_u32(u32::from(high))
                            };
                            out.push(c.ok_or(JsonError::Syntax(self.at))?);
                        }
                        _ => return self.error(),
                    }
                }
                _ => return self.error(),
            }
        }
    }
}
