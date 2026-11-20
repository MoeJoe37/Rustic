//! Fetch's `data:` URL processor (Fetch §4.2 "data: URL processor") and
//! Infra's forgiving-base64 decode.

use rustic_url::Url;

use crate::mime::MimeType;

/// A processed `data:` URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataUrl {
    /// The MIME type (`text/plain;charset=US-ASCII` when absent or invalid).
    pub mime: MimeType,
    /// The decoded body.
    pub body: Vec<u8>,
}

fn is_ascii_whitespace(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

/// Run the processor; `None` is failure (a network error).
pub fn process_data_url(url: &Url) -> Option<DataUrl> {
    if url.scheme() != "data" {
        return None;
    }
    let input = url.serialize_without_fragment();
    let input = &input["data:".len()..];
    let comma = input.find(',')?;
    let mut mime = input[..comma].trim_matches(is_ascii_whitespace).to_owned();
    let mut body = percent_decode(&input.as_bytes()[comma + 1..]);
    if let Some(stripped) = strip_base64_suffix(&mime) {
        let text: String = body.iter().map(|&byte| char::from(byte)).collect();
        body = forgiving_base64_decode(&text)?;
        mime = stripped;
    }
    if mime.starts_with(';') {
        mime.insert_str(0, "text/plain");
    }
    let mime = MimeType::parse(&mime)
        .unwrap_or_else(|| MimeType::parse("text/plain;charset=US-ASCII").expect("valid"));
    Some(DataUrl { mime, body })
}

/// `…;` + spaces + `base64` (case-insensitive) at the end: the MIME type
/// without that suffix.
fn strip_base64_suffix(mime: &str) -> Option<String> {
    let bytes = mime.as_bytes();
    if bytes.len() < 6 || !bytes[bytes.len() - 6..].eq_ignore_ascii_case(b"base64") {
        return None;
    }
    let rest = mime[..mime.len() - 6].trim_end_matches(' ');
    rest.strip_suffix(';').map(str::to_owned)
}

/// URL Standard "percent-decode" over bytes.
fn percent_decode(input: &[u8]) -> Vec<u8> {
    let hex = |byte: u8| char::from(byte).to_digit(16);
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            if let (Some(&high), Some(&low)) = (input.get(index + 1), input.get(index + 2)) {
                if let (Some(high), Some(low)) = (hex(high), hex(low)) {
                    output.push(u8::try_from(high * 16 + low).expect("two hex digits"));
                    index += 3;
                    continue;
                }
            }
        }
        output.push(input[index]);
        index += 1;
    }
    output
}

/// Infra "forgiving-base64 decode".
pub fn forgiving_base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .bytes()
        .filter(|&byte| !matches!(byte, b'\t' | b'\n' | 0x0c | b'\r' | b' '))
        .collect();
    if !input.is_ascii() {
        return None;
    }
    if data.len() % 4 == 0 {
        for _ in 0..2 {
            if data.last() == Some(&b'=') {
                data.pop();
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let value = |byte: u8| -> Option<u32> {
        Some(u32::from(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    };
    let mut output = Vec::with_capacity(data.len() / 4 * 3 + 2);
    let mut buffer = 0u32;
    let mut bits = 0;
    for &byte in &data {
        buffer = (buffer << 6) | value(byte)?;
        bits += 6;
        if bits == 24 {
            output.extend_from_slice(&buffer.to_be_bytes()[1..]);
            buffer = 0;
            bits = 0;
        }
    }
    match bits {
        12 => output.push(u8::try_from(buffer >> 4).expect("8 bits")),
        18 => {
            let bytes = (buffer >> 2).to_be_bytes();
            output.extend_from_slice(&bytes[2..]);
        }
        _ => {}
    }
    Some(output)
}
