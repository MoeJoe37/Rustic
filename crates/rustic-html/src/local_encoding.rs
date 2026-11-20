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
    Ok(sniff_declared_encoding(prefix, scripting, cancellation)?.unwrap_or(Encoding::Utf8))
}

/// The encoding a byte order mark or the first effective `<meta>`
/// declaration in the bounded prefix names, or `None` when there is
/// neither (the caller then applies its transport or fallback encoding).
pub fn sniff_declared_encoding(
    prefix: &[u8],
    scripting: Scripting,
    cancellation: CancellationToken,
) -> Result<Option<Encoding>, ParseError> {
    cancellation.check().map_err(|_| HtmlError::Cancelled)?;
    if let Some((encoding, _)) = bom_sniff(prefix) {
        return Ok(Some(encoding));
    }
    let prefix = &prefix[..prefix.len().min(LOCAL_ENCODING_PREFIX_BYTES)];
    if !prefix
        .windows(5)
        .any(|word| word.eq_ignore_ascii_case(b"<meta"))
    {
        return Ok(None);
    }
    let limits = ParserLimits {
        max_tree_diagnostics: 0,
        ..ParserLimits::default()
    };
    let mut provisional = HtmlDocumentParser::new_with_scripting(limits, cancellation, scripting)?;
    match provisional.probe_encoding(prefix) {
        Ok(encoding) => Ok(encoding),
        Err(
            ParseError::Document(DomError::DepthLimit | DomError::NodeLimit | DomError::DataLimit)
            | ParseError::Tokenizer(
                HtmlError::TokenLimit | HtmlError::TokenBytesLimit | HtmlError::AttributeLimit,
            ),
        ) => {
            // The definitive parser applies the host's independent limits.
            Ok(None)
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
    meta_charset(get("charset"), get("http-equiv"), get("content"))
}

/// The encoding a `<meta>` element's `charset`, or `http-equiv` and
/// `content`, attributes declare (HTML §13.2.6.4.4 "in head", the `meta`
/// start tag), already mapped as "change the encoding" maps it.
pub fn meta_charset(
    charset: Option<&str>,
    http_equiv: Option<&str>,
    content: Option<&str>,
) -> Option<Encoding> {
    let encoding = match charset {
        Some(label) => Encoding::for_label(label),
        None if http_equiv.is_some_and(|value| value.eq_ignore_ascii_case("content-type")) => {
            charset_parameter(content?.as_bytes())
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

/// HTML §13.2.3.3 "suggested default encoding" for a user's locale (a BCP
/// 47 language tag such as `ru` or `zh-TW`): the fallback when neither a
/// byte order mark, the transport nor a `<meta>` names an encoding.
pub fn locale_fallback_encoding(locale: &str) -> Encoding {
    let mut subtags = locale.split(['-', '_']).map(str::to_ascii_lowercase);
    let language = subtags.next().unwrap_or_default();
    let rest: Vec<String> = subtags.collect();
    match language.as_str() {
        "ar" | "fa" => Encoding::Windows1256,
        "ba" | "be" | "bg" | "kk" | "ky" | "mk" | "ru" | "sah" | "sr" | "tg" | "tt" | "uk" => {
            Encoding::Windows1251
        }
        "cs" | "hr" | "sk" => Encoding::Windows1250,
        "el" => Encoding::Iso8859_7,
        "et" | "lt" | "lv" => Encoding::Windows1257,
        "he" => Encoding::Windows1255,
        "hu" | "pl" | "sl" => Encoding::Iso8859_2,
        "ja" => Encoding::ShiftJis,
        "ko" => Encoding::EucKr,
        "ku" | "tr" => Encoding::Windows1254,
        "th" => Encoding::Windows874,
        "vi" => Encoding::Windows1258,
        "zh" if rest
            .iter()
            .any(|subtag| matches!(subtag.as_str(), "tw" | "hk" | "mo" | "hant")) =>
        {
            Encoding::Big5
        }
        "zh" => Encoding::Gb18030,
        _ => Encoding::Windows1252,
    }
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

    #[test]
    fn declarations_and_locale_fallbacks() {
        let sniff = |bytes: &[u8]| {
            sniff_declared_encoding(bytes, Scripting::Disabled, CancellationToken::default())
                .unwrap()
        };
        assert_eq!(sniff(b"<p>no declaration"), None);
        assert_eq!(sniff(b"<meta charset=koi8-r>"), Some(Encoding::Koi8R));
        assert_eq!(
            meta_charset(
                None,
                Some("Content-Type"),
                Some("text/html; charset='big5'")
            ),
            Some(Encoding::Big5)
        );
        assert_eq!(
            meta_charset(Some("utf-16be"), None, None),
            Some(Encoding::Utf8)
        );
        assert_eq!(meta_charset(None, None, Some("charset=big5")), None);
        assert_eq!(locale_fallback_encoding("en-US"), Encoding::Windows1252);
        assert_eq!(locale_fallback_encoding("ru"), Encoding::Windows1251);
        assert_eq!(locale_fallback_encoding("ja-JP"), Encoding::ShiftJis);
        assert_eq!(locale_fallback_encoding("zh-TW"), Encoding::Big5);
        assert_eq!(locale_fallback_encoding("zh_Hant_HK"), Encoding::Big5);
        assert_eq!(locale_fallback_encoding("zh-CN"), Encoding::Gb18030);
        assert_eq!(locale_fallback_encoding("pl"), Encoding::Iso8859_2);
        assert_eq!(locale_fallback_encoding(""), Encoding::Windows1252);
    }
}
