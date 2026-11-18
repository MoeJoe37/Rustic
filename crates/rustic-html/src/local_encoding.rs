//! Bounded encoding selection for local HTML documents.

use rustic_base::CancellationToken;
use rustic_dom::DomError;
use rustic_encoding::{Encoding, bom_sniff};

use crate::{HtmlDocumentParser, HtmlError, ParseError, ParserLimits, Scripting, Tag};

/// Maximum raw prefix retained for local HTML encoding replay.
pub const LOCAL_ENCODING_PREFIX_BYTES: usize = 64 * 1024;

/// Select the decoder before the local document is published to script.
///
/// A BOM wins. Otherwise a private, tentative UTF-8 parser finds the first
/// effective HTML meta declaration in the prefix. The host can replay the
/// original bytes in the returned encoding without repeating author scripts.
pub fn select_local_encoding(
    prefix: &[u8],
    scripting: Scripting,
    cancellation: CancellationToken,
) -> Result<Encoding, ParseError> {
    cancellation.check().map_err(|_| HtmlError::Cancelled)?;
    if let Some((encoding, _)) = bom_sniff(prefix) {
        return Ok(encoding);
    }
    let prefix = &prefix[..prefix.len().min(LOCAL_ENCODING_PREFIX_BYTES)];
    if !prefix
        .windows(5)
        .any(|word| word.eq_ignore_ascii_case(b"<meta"))
    {
        return Ok(Encoding::Utf8);
    }
    let limits = ParserLimits {
        max_tree_diagnostics: 0,
        ..ParserLimits::default()
    };
    let mut provisional = HtmlDocumentParser::new_with_scripting(limits, cancellation, scripting)?;
    match provisional.probe_encoding(prefix) {
        Ok(encoding) => Ok(encoding.unwrap_or(Encoding::Utf8)),
        Err(
            ParseError::Document(DomError::DepthLimit | DomError::NodeLimit | DomError::DataLimit)
            | ParseError::Tokenizer(
                HtmlError::TokenLimit | HtmlError::TokenBytesLimit | HtmlError::AttributeLimit,
            ),
        ) => {
            // The definitive parser applies the host's independent limits.
            Ok(Encoding::Utf8)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn meta_encoding(tag: &Tag) -> Option<Encoding> {
    let get = |name: &str| {
        tag.attributes()
            .iter()
            .find(|attribute| attribute.name() == name)
            .map(|attribute| attribute.value())
    };
    let encoding = match get("charset") {
        Some(label) => Encoding::for_label(label),
        None if get("http-equiv")
            .is_some_and(|value| value.eq_ignore_ascii_case("content-type")) =>
        {
            charset_parameter(get("content")?.as_bytes())
        }
        None => None,
    }?;
    match encoding {
        Encoding::Utf16Le | Encoding::Utf16Be => Some(Encoding::Utf8),
        Encoding::XUserDefined => Some(Encoding::Windows1252),
        Encoding::Replacement => None,
        encoding => Some(encoding),
    }
}

fn charset_parameter(mut value: &[u8]) -> Option<Encoding> {
    const CHARSET: &[u8] = b"charset";
    while let Some(index) = value
        .windows(CHARSET.len())
        .position(|word| word.eq_ignore_ascii_case(CHARSET))
    {
        value = value[index + CHARSET.len()..].trim_ascii_start();
        let Some(after_equal) = value.strip_prefix(b"=") else {
            continue;
        };
        let value = after_equal.trim_ascii_start();
        let label = match value {
            [quote @ (b'\'' | b'"'), rest @ ..] => {
                &rest[..rest.iter().position(|byte| byte == quote)?]
            }
            _ => {
                &value[..value
                    .iter()
                    .position(|byte| byte.is_ascii_whitespace() || *byte == b';')
                    .unwrap_or(value.len())]
            }
        };
        return Encoding::for_label_bytes(label);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(bytes: &[u8], scripting: Scripting) -> Encoding {
        select_local_encoding(bytes, scripting, CancellationToken::default()).unwrap()
    }

    #[test]
    fn bom_wins_and_html_labels_are_normalized() {
        assert_eq!(
            select(b"\xef\xbb\xbf<meta charset=latin1>", Scripting::Enabled),
            Encoding::Utf8
        );
        assert_eq!(
            select(b"\xff\xfe<\0", Scripting::Disabled),
            Encoding::Utf16Le
        );
        assert_eq!(
            select(
                b"<meta charset=utf-16le><meta charset=latin1>",
                Scripting::Enabled
            ),
            Encoding::Utf8
        );
        assert_eq!(
            select(b"<meta charset=x-user-defined>", Scripting::Enabled),
            Encoding::Windows1252
        );
        assert_eq!(
            select(
                b"<meta charset=iso-2022-kr><meta charset=shift_jis>",
                Scripting::Enabled
            ),
            Encoding::ShiftJis
        );
    }

    #[test]
    fn declarations_follow_html_token_and_tree_boundaries() {
        let markup = br#"<!-- <meta charset=latin1> -->
            <script>const text = "<meta charset=latin1>";</script>
            <template><meta charset=latin1></template>
            <noscript><meta charset=latin1></noscript>
            <meta content="text/html; charset=latin1">
            <meta content="text/html; charset=shift_jis" http-equiv=CONTENT-TYPE>"#;
        assert_eq!(select(markup, Scripting::Enabled), Encoding::ShiftJis);
        assert_eq!(select(markup, Scripting::Disabled), Encoding::Windows1252);
    }

    #[test]
    fn the_prefix_and_encoding_lock_are_bounded() {
        let mut markup = b"<!--".to_vec();
        markup.resize(LOCAL_ENCODING_PREFIX_BYTES, b'x');
        markup.extend_from_slice(b"--><meta charset=latin1>");
        assert_eq!(select(&markup, Scripting::Enabled), Encoding::Utf8);
        let mut parser = HtmlDocumentParser::default();
        parser.feed(b"<p>").unwrap();
        assert_eq!(
            parser.set_initial_encoding(Encoding::Windows1252),
            Err(ParseError::EncodingLocked)
        );
        parser.feed("é".as_bytes()).unwrap();
        assert_eq!(parser.finish().unwrap().encoding(), Encoding::Utf8);
    }
}
