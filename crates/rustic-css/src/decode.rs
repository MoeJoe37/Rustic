//! Stylesheet byte decoding (CSS Syntax Level 3 §3.2 "The input byte stream").

use rustic_encoding::Encoding;

/// "Determine the fallback encoding" of a stylesheet, then "decode" its bytes:
/// a byte order mark wins over everything; otherwise the transport's label,
/// then an `@charset` rule in the first 1024 bytes, then the referring
/// document's encoding, then UTF-8.
///
/// `protocol_label` is the `charset` parameter of the response's
/// `Content-Type`, if any. `environment` is the referring document's
/// encoding, if any. An `@charset` naming UTF-16BE/LE selects UTF-8, since
/// a sheet that really were UTF-16 would carry a BOM. Errors become U+FFFD.
pub fn decode_stylesheet(
    bytes: &[u8],
    protocol_label: Option<&[u8]>,
    environment: Option<Encoding>,
) -> String {
    rustic_encoding::decode(
        bytes,
        stylesheet_fallback_encoding(bytes, protocol_label, environment),
    )
    .text
}

/// CSS Syntax §3.2 "determine the fallback encoding".
pub fn stylesheet_fallback_encoding(
    bytes: &[u8],
    protocol_label: Option<&[u8]>,
    environment: Option<Encoding>,
) -> Encoding {
    if let Some(encoding) = protocol_label.and_then(Encoding::for_label_bytes) {
        return encoding;
    }
    if let Some(encoding) = charset_rule_label(bytes).and_then(Encoding::for_label_bytes) {
        return if encoding.is_utf16() {
            Encoding::Utf8
        } else {
            encoding
        };
    }
    environment.unwrap_or(Encoding::Utf8)
}

/// The label of a leading `@charset "…";` rule, matched byte for byte within
/// the first 1024 bytes as the specification requires.
fn charset_rule_label(bytes: &[u8]) -> Option<&[u8]> {
    const PREFIX: &[u8] = b"@charset \"";
    let prefix = &bytes[..bytes.len().min(1024)];
    let rest = prefix.strip_prefix(PREFIX)?;
    let end = rest.iter().position(|&byte| byte == b'"')?;
    (rest.get(end + 1) == Some(&b';')).then(|| &rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_order() {
        let latin1 = b"@charset \"iso-8859-1\"; p::before { content: \"\xE9\" }";
        assert_eq!(
            stylesheet_fallback_encoding(latin1, None, None),
            Encoding::Windows1252
        );
        assert!(decode_stylesheet(latin1, None, None).contains("\"\u{E9}\""));
        // The transport label wins over @charset.
        assert_eq!(
            stylesheet_fallback_encoding(latin1, Some(b"koi8-r"), None),
            Encoding::Koi8R
        );
        // An unknown transport label is ignored.
        assert_eq!(
            stylesheet_fallback_encoding(latin1, Some(b"bogus"), None),
            Encoding::Windows1252
        );
        // The referring document's encoding comes next, then UTF-8.
        assert_eq!(
            stylesheet_fallback_encoding(b"p{}", None, Some(Encoding::ShiftJis)),
            Encoding::ShiftJis
        );
        assert_eq!(
            stylesheet_fallback_encoding(b"p{}", None, None),
            Encoding::Utf8
        );
        // A BOM overrides every label.
        assert_eq!(
            decode_stylesheet(b"\xEF\xBB\xBF@charset \"latin1\"; \xC3\xA9", None, None),
            "@charset \"latin1\"; \u{E9}"
        );
        assert_eq!(
            decode_stylesheet(b"\xFF\xFEa\x00", Some(b"latin1"), None),
            "a"
        );
    }

    #[test]
    fn charset_rule_is_matched_exactly() {
        assert_eq!(
            charset_rule_label(b"@charset \"utf-16\";"),
            Some(&b"utf-16"[..])
        );
        assert_eq!(
            stylesheet_fallback_encoding(b"@charset \"utf-16be\";", None, None),
            Encoding::Utf8
        );
        for not_a_rule in [
            &b"@charset 'latin1';"[..],
            b"@CHARSET \"latin1\";",
            b" @charset \"latin1\";",
            b"@charset  \"latin1\";",
            b"@charset \"latin1\"",
            b"@charset \"latin1\" ;",
            b"/**/@charset \"latin1\";",
        ] {
            assert_eq!(
                stylesheet_fallback_encoding(not_a_rule, None, None),
                Encoding::Utf8,
                "{not_a_rule:?}"
            );
        }
        // The rule must end within the first 1024 bytes.
        let mut long = b"@charset \"".to_vec();
        long.extend(std::iter::repeat_n(b'a', 1020));
        long.extend_from_slice(b"\";");
        assert_eq!(charset_rule_label(&long), None);
    }
}
