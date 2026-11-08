//! A minimal JSON reader for the vendored css-parsing-tests files.

#![allow(dead_code, reason = "each test binary uses a different subset")]

#[derive(Clone, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
}

impl PartialEq for Json {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            // Numeric values compare by value: `-0` equals `0`.
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            _ => false,
        }
    }
}

/// A minimal JSON reader for the vendored files (no objects needed).
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn skip(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Json {
        self.skip();
        match self.bytes[self.pos] {
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip();
                    if self.bytes[self.pos] == b']' {
                        self.pos += 1;
                        return Json::Array(items);
                    }
                    items.push(self.value());
                    self.skip();
                    if self.bytes[self.pos] == b',' {
                        self.pos += 1;
                    }
                }
            }
            b'"' => Json::String(self.string()),
            b'n' => self.word("null", Json::Null),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            _ => {
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && matches!(
                        self.bytes[self.pos],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.pos += 1;
                }
                let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
                Json::Number(text.parse().unwrap_or_else(|_| panic!("bad number {text}")))
            }
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Json {
        assert!(self.bytes[self.pos..].starts_with(word.as_bytes()));
        self.pos += word.len();
        value
    }

    fn hex4(&mut self) -> u32 {
        let text = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4]).unwrap();
        self.pos += 4;
        u32::from_str_radix(text, 16).unwrap()
    }

    fn string(&mut self) -> String {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let rest = std::str::from_utf8(&self.bytes[self.pos..]).unwrap();
            let c = rest.chars().next().unwrap();
            self.pos += c.len_utf8();
            match c {
                '"' => return out,
                '\\' => {
                    let e = self.bytes[self.pos];
                    self.pos += 1;
                    match e {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'f' => out.push('\u{c}'),
                        b'b' => out.push('\u{8}'),
                        b'u' => {
                            let mut unit = self.hex4();
                            if (0xd800..0xdc00).contains(&unit)
                                && self.bytes[self.pos..].starts_with(b"\\u")
                            {
                                self.pos += 2;
                                let low = self.hex4();
                                unit = 0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
                            }
                            out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                        }
                        other => out.push(other as char),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

pub fn parse_json(source: &str) -> Vec<Json> {
    let mut reader = Reader {
        bytes: source.as_bytes(),
        pos: 0,
    };
    match reader.value() {
        Json::Array(items) => items,
        _ => panic!("corpus file is not an array"),
    }
}
