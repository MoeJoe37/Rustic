//! URL Standard §5 `application/x-www-form-urlencoded`: the serializer
//! (with an output encoding, for HTML form submission) and the parser.

use rustic_encoding::Encoding;

/// The `application/x-www-form-urlencoded` percent-encode set: everything
/// but ASCII alphanumerics and `*-._` (space becomes `+`).
fn keep(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'-' | b'.' | b'_')
}

fn serialize_one(input: &str, encoding: Encoding, output: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoder = encoding.output_encoding().new_encoder();
    let mut bytes = Vec::new();
    let push = |bytes: &mut Vec<u8>, output: &mut String| {
        for &byte in bytes.iter() {
            if byte == b' ' {
                output.push('+');
            } else if keep(byte) {
                output.push(char::from(byte));
            } else {
                output.push('%');
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 15)]));
            }
        }
        bytes.clear();
    };
    for character in input.chars() {
        if let Err(unmappable) = encoder.encode_char(character, &mut bytes) {
            push(&mut bytes, output);
            output.push_str("%26%23");
            output.push_str(&u32::from(unmappable).to_string());
            output.push_str("%3B");
        }
    }
    encoder.finish(&mut bytes);
    push(&mut bytes, output);
}

/// §5.2 "application/x-www-form-urlencoded serializer" with `encoding`'s
/// output encoding (UTF-8 for UTF-16 and replacement). Unmappable
/// characters become `&#N;`, percent-encoded.
pub fn serialize_form(entries: &[(String, String)], encoding: Encoding) -> String {
    let mut output = String::new();
    for (index, (name, value)) in entries.iter().enumerate() {
        if index > 0 {
            output.push('&');
        }
        serialize_one(name, encoding, &mut output);
        output.push('=');
        serialize_one(value, encoding, &mut output);
    }
    output
}

/// §5.1 "application/x-www-form-urlencoded parser" over UTF-8 bytes.
pub fn parse_form(input: &[u8]) -> Vec<(String, String)> {
    let decode = |bytes: &[u8]| {
        let replaced: Vec<u8> = bytes
            .iter()
            .map(|&byte| if byte == b'+' { b' ' } else { byte })
            .collect();
        String::from_utf8_lossy(&percent_decode(&replaced)).into_owned()
    };
    input
        .split(|&byte| byte == b'&')
        .filter(|sequence| !sequence.is_empty())
        .map(
            |sequence| match sequence.iter().position(|&byte| byte == b'=') {
                Some(index) => (decode(&sequence[..index]), decode(&sequence[index + 1..])),
                None => (decode(sequence), String::new()),
            },
        )
        .collect()
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let hex = |byte: u8| char::from(byte).to_digit(16);
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            if let (Some(high), Some(low)) = (
                input.get(index + 1).copied().and_then(hex),
                input.get(index + 2).copied().and_then(hex),
            ) {
                output.push(u8::try_from(high * 16 + low).expect("two hex digits"));
                index += 3;
                continue;
            }
        }
        output.push(input[index]);
        index += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn serializes_with_plus_and_percent_encoding() {
        assert_eq!(
            serialize_form(
                &entries(&[("q", "a b&c=d"), ("é", "*-._~!")]),
                Encoding::Utf8
            ),
            "q=a+b%26c%3Dd&%C3%A9=*-._%7E%21"
        );
        assert_eq!(serialize_form(&[], Encoding::Utf8), "");
        assert_eq!(serialize_form(&entries(&[("", "")]), Encoding::Utf8), "=");
        // Line breaks are already normalized by the entry list.
        assert_eq!(
            serialize_form(&entries(&[("t", "a\r\nb")]), Encoding::Utf8),
            "t=a%0D%0Ab"
        );
    }

    #[test]
    fn legacy_encodings_and_unmappable_characters() {
        assert_eq!(
            serialize_form(&entries(&[("x", "é€")]), Encoding::Windows1252),
            "x=%E9%80"
        );
        assert_eq!(
            serialize_form(&entries(&[("x", "☃")]), Encoding::Windows1252),
            "x=%26%239731%3B"
        );
        assert_eq!(
            serialize_form(&entries(&[("x", "é")]), Encoding::Utf16Le),
            "x=%C3%A9"
        );
    }

    #[test]
    fn parser_round_trips_and_handles_edge_cases() {
        let list = entries(&[("a b", "c+d"), ("é", ""), ("=", "&")]);
        assert_eq!(
            parse_form(serialize_form(&list, Encoding::Utf8).as_bytes()),
            list
        );
        assert_eq!(
            parse_form(b"&&a&b=&=c&%zz=%41"),
            entries(&[("a", ""), ("b", ""), ("", "c"), ("%zz", "A")])
        );
        assert_eq!(parse_form(b"x=%FF"), entries(&[("x", "\u{fffd}")]));
    }
}
