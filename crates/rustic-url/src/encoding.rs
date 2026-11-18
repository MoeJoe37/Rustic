use crate::ParseError;

#[derive(Clone, Copy)]
pub(crate) enum EncodeSet {
    Control,
    Fragment,
    Query,
    SpecialQuery,
    Path,
    UserInfo,
}

fn should_encode(byte: u8, set: EncodeSet) -> bool {
    let control = byte <= 0x1f || byte >= 0x7f;
    let query = control || matches!(byte, b' ' | b'"' | b'#' | b'<' | b'>');
    let path = query || matches!(byte, b'?' | b'^' | b'`' | b'{' | b'}');
    match set {
        EncodeSet::Control => control,
        EncodeSet::Fragment => control || matches!(byte, b' ' | b'"' | b'<' | b'>' | b'`'),
        EncodeSet::Query => query,
        EncodeSet::SpecialQuery => query || byte == b'\'',
        EncodeSet::Path => path,
        EncodeSet::UserInfo => {
            path || matches!(byte, b'/' | b':' | b';' | b'=' | b'@' | b'['..=b']' | b'|')
        }
    }
}

pub(crate) fn encode(input: &str, set: EncodeSet) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::new();
    for byte in input.bytes() {
        if should_encode(byte, set) {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 15)]));
        } else {
            output.push(char::from(byte));
        }
    }
    output
}

/// "Percent-encode after encoding" with the special-query percent-encode set:
/// encode `input` with `encoding`, percent-encode the bytes that need it, and
/// write each unmappable scalar value as `%26%23` + decimal + `%3B`.
pub(crate) fn encode_legacy_query(input: &str, encoding: rustic_encoding::Encoding) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoder = encoding.new_encoder();
    let mut bytes = Vec::with_capacity(input.len());
    let mut output = String::with_capacity(input.len());
    let flush = |bytes: &mut Vec<u8>, output: &mut String| {
        for &byte in bytes.iter() {
            if should_encode(byte, EncodeSet::SpecialQuery) {
                output.push('%');
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 15)]));
            } else {
                output.push(char::from(byte));
            }
        }
        bytes.clear();
    };
    for character in input.chars() {
        if let Err(unmappable) = encoder.encode_char(character, &mut bytes) {
            flush(&mut bytes, &mut output);
            output.push_str("%26%23");
            output.push_str(&u32::from(unmappable).to_string());
            output.push_str("%3B");
        }
    }
    encoder.finish(&mut bytes);
    flush(&mut bytes, &mut output);
    output
}

pub(crate) fn decode_host(input: &str) -> Result<String, ParseError> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).map_err(|_| ParseError::InvalidHost)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
