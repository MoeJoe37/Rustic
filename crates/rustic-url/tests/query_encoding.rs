//! URL Standard "percent-encode after encoding": a document's legacy encoding
//! applies to the queries of special, non-WebSocket URLs only.

use rustic_base::CancellationToken;
use rustic_encoding::Encoding;
use rustic_url::{Url, UrlLimits, UrlParser};

fn parse(input: &str, base: Option<&str>, encoding: Encoding) -> String {
    let parser = UrlParser::new(UrlLimits::default(), CancellationToken::default())
        .with_query_encoding(encoding);
    let base = base.map(|base| Url::parse(base, None).unwrap());
    parser.parse(input, base.as_ref()).unwrap().serialize()
}

#[test]
fn special_queries_use_the_document_encoding() {
    let latin = Encoding::Windows1252;
    assert_eq!(
        parse("http://h/?\u{E9}\u{20AC}\u{2603}", None, latin),
        "http://h/?%E9%80%26%239731%3B"
    );
    assert_eq!(
        parse("https://h/p?a='b c'#\u{E9}", None, latin),
        "https://h/p?a=%27b%20c%27#%C3%A9"
    );
    assert_eq!(
        parse("?\u{E9}", Some("http://h/dir/page"), latin),
        "http://h/dir/page?%E9"
    );
    assert_eq!(parse("file:///x?\u{E9}", None, latin), "file:///x?%E9");
    assert_eq!(parse("http://h/\u{E9}?q", None, latin), "http://h/%C3%A9?q");
    assert_eq!(
        parse("http://h/?\u{3042}", None, Encoding::ShiftJis),
        "http://h/?%82%A0"
    );
    assert_eq!(
        parse("http://h/?\u{3042}", None, Encoding::Iso2022Jp),
        "http://h/?%1B$B$%22%1B(B"
    );
    assert_eq!(
        parse("http://h/?%E9%zz", None, Encoding::Gbk),
        "http://h/?%E9%zz"
    );
}

#[test]
fn other_urls_and_encodings_stay_utf8() {
    let latin = Encoding::Windows1252;
    assert_eq!(parse("ws://h/?\u{E9}", None, latin), "ws://h/?%C3%A9");
    assert_eq!(parse("wss://h/?\u{E9}", None, latin), "wss://h/?%C3%A9");
    assert_eq!(parse("x:/p?\u{E9}'", None, latin), "x:/p?%C3%A9'");
    assert_eq!(parse("mailto:a?\u{E9}", None, latin), "mailto:a?%C3%A9");
    // UTF-16 and replacement documents encode URLs as UTF-8.
    assert_eq!(
        parse("http://h/?\u{E9}", None, Encoding::Utf16Le),
        "http://h/?%C3%A9"
    );
    assert_eq!(
        parse("http://h/?\u{E9}", None, Encoding::Replacement),
        "http://h/?%C3%A9"
    );
    assert_eq!(
        parse("http://h/?\u{E9}", None, Encoding::Utf8),
        "http://h/?%C3%A9"
    );
    // A query copied from the base is not re-encoded.
    assert_eq!(
        parse("#f", Some("http://h/?%C3%A9"), latin),
        "http://h/?%C3%A9#f"
    );
    assert_eq!(parse("/p", Some("http://h/?x"), latin), "http://h/p");
}
