//! MIME types (WHATWG MIME Sniffing §4) and Fetch's `Content-Type`
//! extraction (Fetch §3.1.7 "extract a MIME type", "legacy extract an
//! encoding") and header value splitting ("get, decode, and split").

use std::fmt;

use rustic_encoding::Encoding;

use crate::Headers;

/// A parsed MIME type record.
///
/// Type, subtype and parameter names are ASCII-lowercased; parameter values
/// keep their case. Parameters keep their first occurrence and order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MimeType {
    kind: String,
    subtype: String,
    parameters: Vec<(String, String)>,
}

impl MimeType {
    /// MIME Sniffing "parse a MIME type". `None` is failure.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim_matches(is_http_whitespace);
        let (kind, rest) = input.split_once('/')?;
        if kind.is_empty() || !kind.chars().all(is_token_char) {
            return None;
        }
        let (subtype, mut rest) = match rest.find(';') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        let subtype = subtype.trim_end_matches(is_http_whitespace);
        if subtype.is_empty() || !subtype.chars().all(is_token_char) {
            return None;
        }
        let mut mime = Self {
            kind: kind.to_ascii_lowercase(),
            subtype: subtype.to_ascii_lowercase(),
            parameters: Vec::new(),
        };
        // `rest` is empty or starts at a ';'.
        while let Some(after) = rest.strip_prefix(';') {
            let after = after.trim_start_matches(is_http_whitespace);
            let name_end = after.find([';', '=']).unwrap_or(after.len());
            let name = after[..name_end].to_ascii_lowercase();
            rest = &after[name_end..];
            if rest.starts_with(';') {
                continue;
            }
            let Some(value_start) = rest.strip_prefix('=') else {
                break;
            };
            let value;
            if value_start.starts_with('"') {
                let (quoted, consumed) = collect_quoted_string(value_start, true);
                value = quoted;
                let after_value = &value_start[consumed..];
                rest = &after_value[after_value.find(';').unwrap_or(after_value.len())..];
            } else {
                let end = value_start.find(';').unwrap_or(value_start.len());
                let raw = value_start[..end].trim_end_matches(is_http_whitespace);
                rest = &value_start[end..];
                if raw.is_empty() {
                    continue;
                }
                value = raw.to_owned();
            }
            if !name.is_empty()
                && name.chars().all(is_token_char)
                && value.chars().all(is_quoted_string_token_char)
                && mime.parameter(&name).is_none()
            {
                mime.parameters.push((name, value));
            }
        }
        Some(mime)
    }

    /// The type, such as `text`.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The subtype, such as `html`.
    pub fn subtype(&self) -> &str {
        &self.subtype
    }

    /// `type/subtype`.
    pub fn essence(&self) -> String {
        format!("{}/{}", self.kind, self.subtype)
    }

    /// A parameter value by (lowercase) name.
    pub fn parameter(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Parameters in order.
    pub fn parameters(&self) -> impl Iterator<Item = (&str, &str)> {
        self.parameters
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    fn set_parameter(&mut self, name: &str, value: String) {
        match self.parameters.iter_mut().find(|(key, _)| key == name) {
            Some(entry) => entry.1 = value,
            None => self.parameters.push((name.to_owned(), value)),
        }
    }

    /// The encoding named by the `charset` parameter, if any is valid.
    pub fn charset(&self) -> Option<Encoding> {
        self.parameter("charset").and_then(Encoding::for_label)
    }
}

impl fmt::Display for MimeType {
    /// MIME Sniffing "serialize a MIME type".
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.kind, self.subtype)?;
        for (name, value) in &self.parameters {
            write!(formatter, ";{name}=")?;
            if !value.is_empty() && value.chars().all(is_token_char) {
                formatter.write_str(value)?;
            } else {
                formatter.write_str("\"")?;
                for character in value.chars() {
                    if matches!(character, '"' | '\\') {
                        formatter.write_str("\\")?;
                    }
                    write!(formatter, "{character}")?;
                }
                formatter.write_str("\"")?;
            }
        }
        Ok(())
    }
}

/// Fetch "extract a MIME type" from a header list. `None` is failure.
///
/// Every `Content-Type` value is parsed in order; `*/*` and unparsable
/// values are skipped, a later type wins, and a `charset` from an earlier
/// value with the same essence is carried over.
pub fn extract_mime_type(headers: &Headers) -> Option<MimeType> {
    let combined = headers.combined("content-type")?;
    let mut result: Option<MimeType> = None;
    let mut charset: Option<String> = None;
    let mut essence: Option<String> = None;
    for value in split_header_value(&combined) {
        let Some(mut mime) = MimeType::parse(&value) else {
            continue;
        };
        let this_essence = mime.essence();
        if this_essence == "*/*" {
            continue;
        }
        if essence.as_deref() != Some(this_essence.as_str()) {
            charset = mime.parameter("charset").map(str::to_owned);
            essence = Some(this_essence);
        } else if mime.parameter("charset").is_none() {
            if let Some(charset) = &charset {
                mime.set_parameter("charset", charset.clone());
            }
        }
        result = Some(mime);
    }
    result
}

/// Fetch "legacy extract an encoding": the `charset` of `mime`, if it
/// names an encoding.
pub fn legacy_extract_encoding(mime: Option<&MimeType>) -> Option<Encoding> {
    mime.and_then(MimeType::charset)
}

/// Fetch "get, decode, and split" applied to one combined header value:
/// isomorphic-decode, then split on commas outside quoted strings and
/// trim HTTP tab or space.
pub fn split_header_value(value: &[u8]) -> Vec<String> {
    let input: String = value.iter().map(|&byte| char::from(byte)).collect();
    let mut values = Vec::new();
    let mut rest = input.as_str();
    let mut temporary = String::new();
    loop {
        let end = rest.find(['"', ',']).unwrap_or(rest.len());
        temporary.push_str(&rest[..end]);
        rest = &rest[end..];
        if rest.starts_with('"') {
            let (_, consumed) = collect_quoted_string(rest, false);
            temporary.push_str(&rest[..consumed]);
            rest = &rest[consumed..];
            if !rest.is_empty() {
                continue;
            }
        }
        values.push(temporary.trim_matches([' ', '\t']).to_owned());
        temporary.clear();
        match rest.strip_prefix(',') {
            Some(after) => rest = after,
            None => return values,
        }
    }
}

/// Fetch "collect an HTTP quoted string" at a leading `"`. Returns the
/// value (unescaped when `extract` is true, otherwise empty: the caller
/// takes the raw consumed text) and the number of bytes consumed.
fn collect_quoted_string(input: &str, extract: bool) -> (String, usize) {
    let mut value = String::new();
    let mut chars = input.char_indices().skip(1);
    while let Some((index, character)) = chars.next() {
        match character {
            '"' => return (value, index + 1),
            '\\' => match chars.next() {
                Some((_, escaped)) => {
                    if extract {
                        value.push(escaped);
                    }
                }
                None => {
                    if extract {
                        value.push('\\');
                    }
                    return (value, input.len());
                }
            },
            other => {
                if extract {
                    value.push(other);
                }
            }
        }
    }
    (value, input.len())
}

fn is_http_whitespace(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | ' ')
}

fn is_token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(character)
}

fn is_quoted_string_token_char(character: char) -> bool {
    matches!(character, '\t' | ' '..='~' | '\u{80}'..='\u{ff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_serializes_like_the_standard() {
        let mime =
            MimeType::parse(" Text/HTML ; Charset=\"utf-8\" ; x=\"a\\\"b\";charset=gbk;empty=;q")
                .unwrap();
        assert_eq!((mime.kind(), mime.subtype()), ("text", "html"));
        assert_eq!(mime.essence(), "text/html");
        assert_eq!(mime.parameter("charset"), Some("utf-8"));
        assert_eq!(mime.charset(), Some(Encoding::Utf8));
        assert_eq!(mime.to_string(), "text/html;charset=utf-8;x=\"a\\\"b\"");
        for bad in [
            "",
            "text",
            "text/",
            "/html",
            "te xt/html",
            "text/ht ml",
            "text/html\u{100}",
        ] {
            assert_eq!(MimeType::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn splits_header_values_outside_quotes() {
        assert_eq!(split_header_value(b"a, b ,\tc"), ["a", "b", "c"]);
        assert_eq!(split_header_value(b"x=\"1,2\", y"), ["x=\"1,2\"", "y"]);
        assert_eq!(split_header_value(b""), [""]);
        assert_eq!(
            split_header_value(b"\"unterminated, still"),
            ["\"unterminated, still"]
        );
        assert_eq!(split_header_value(b"\xe9"), ["\u{e9}"]);
    }
}
