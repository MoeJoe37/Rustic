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

    /// MIME Sniffing §4.6 "JavaScript MIME type" (by essence).
    pub fn is_javascript(&self) -> bool {
        is_javascript_essence(&self.essence())
    }

    /// Fetch §3.5 / HTML "destination is script-like": the types never
    /// executed as script (`audio/*`, `image/*`, `video/*`, `text/csv`).
    pub fn is_blocked_for_script(&self) -> bool {
        matches!(self.kind.as_str(), "audio" | "image" | "video") || self.essence() == "text/csv"
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

/// MIME Sniffing §4.6: the sixteen JavaScript MIME type essences.
pub fn is_javascript_essence(essence: &str) -> bool {
    matches!(
        essence,
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
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

/// MIME Sniffing §7.1 "rules for identifying an unknown MIME type" with
/// the sniff-scriptable flag set (navigations): HTML, XML and PDF
/// signatures, the image signatures Rustic decodes, then text versus
/// binary. `header` is the first bytes of the body (at most 1,445 are
/// examined, the resource header length of §5.2).
pub fn sniff_unknown(header: &[u8]) -> &'static str {
    let header = &header[..header.len().min(1445)];
    let start = header
        .iter()
        .position(|byte| !matches!(byte, b'\t' | b'\n' | 0x0c | b'\r' | b' '))
        .unwrap_or(header.len());
    let trimmed = &header[start..];
    const HTML: [&[u8]; 17] = [
        b"<!DOCTYPE HTML",
        b"<HTML",
        b"<HEAD",
        b"<SCRIPT",
        b"<IFRAME",
        b"<H1",
        b"<DIV",
        b"<FONT",
        b"<TABLE",
        b"<A",
        b"<STYLE",
        b"<TITLE",
        b"<B",
        b"<BODY",
        b"<BR",
        b"<P",
        b"<!--",
    ];
    for pattern in HTML {
        if trimmed.len() > pattern.len()
            && trimmed[..pattern.len()].eq_ignore_ascii_case(pattern)
            && matches!(trimmed[pattern.len()], b' ' | b'>')
        {
            return "text/html";
        }
    }
    if trimmed.starts_with(b"<?xml") {
        return "text/xml";
    }
    if header.starts_with(b"%PDF-") {
        return "application/pdf";
    }
    for (signature, mime) in [
        (&b"\x89PNG\r\n\x1a\n"[..], "image/png"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"\xff\xd8\xff", "image/jpeg"),
    ] {
        if header.starts_with(signature) {
            return mime;
        }
    }
    if header.len() >= 14 && header.starts_with(b"RIFF") && &header[8..14] == b"WEBPVP" {
        return "image/webp";
    }
    // §7.1 step 8: a BOM means text; otherwise any binary data byte means
    // binary.
    if header.starts_with(b"\xfe\xff")
        || header.starts_with(b"\xff\xfe")
        || header.starts_with(b"\xef\xbb\xbf")
    {
        return "text/plain";
    }
    let binary = |byte: &u8| matches!(byte, 0x00..=0x08 | 0x0b | 0x0e..=0x1a | 0x1c..=0x1f);
    if header.iter().any(binary) {
        "application/octet-stream"
    } else {
        "text/plain"
    }
}

#[cfg(test)]
mod sniff_tests {
    use super::{MimeType, sniff_unknown};

    #[test]
    fn unknown_types_are_sniffed() {
        for (body, expected) in [
            (&b"  <!doctype html><p>x"[..], "text/html"),
            (b"<p>para", "text/html"),
            (b"<!-- c -->", "text/html"),
            (b"<pre>", "text/plain"),
            (b"<a>", "text/html"),
            (b"<abbr>", "text/plain"),
            (b"<?xml version", "text/xml"),
            (b"%PDF-1.7", "application/pdf"),
            (b"\x89PNG\r\n\x1a\nrest", "image/png"),
            (b"GIF89a", "image/gif"),
            (b"\xff\xd8\xff\xe0", "image/jpeg"),
            (b"RIFF\0\0\0\0WEBPVP8 ", "image/webp"),
            (b"plain words", "text/plain"),
            (b"\x00\x01binary", "application/octet-stream"),
            (b"\xef\xbb\xbf\x00", "text/plain"),
            (b"", "text/plain"),
        ] {
            assert_eq!(sniff_unknown(body), expected, "{body:?}");
        }
    }

    #[test]
    fn javascript_and_script_blocked_types() {
        let mime = |input: &str| MimeType::parse(input).unwrap();
        assert!(mime("text/javascript;charset=utf-8").is_javascript());
        assert!(mime("Application/X-JavaScript").is_javascript());
        assert!(!mime("text/plain").is_javascript());
        assert!(!mime("application/json").is_javascript());
        assert!(mime("image/png").is_blocked_for_script());
        assert!(mime("text/csv").is_blocked_for_script());
        assert!(!mime("text/plain").is_blocked_for_script());
    }
}
