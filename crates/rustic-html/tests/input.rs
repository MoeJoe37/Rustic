use rustic_base::CancellationToken;
use rustic_html::{HtmlError, Utf8Input};

fn decode(chunks: &[&[u8]]) -> String {
    let mut input = Utf8Input::new(4096, CancellationToken::default());
    let mut result = String::new();
    for chunk in chunks {
        result.push_str(&input.feed(chunk).unwrap());
    }
    result.push_str(&input.finish().unwrap());
    result
}

fn reference(bytes: &[u8]) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    decoded
        .strip_prefix('\u{feff}')
        .unwrap_or(&decoded)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

#[test]
fn utf8_preprocessing_is_independent_of_byte_boundaries() {
    let cases: &[&[u8]] = &[
        b"",
        "\u{feff}Rustic\r\n\r\r\n\n\0é中🙂\u{feff}".as_bytes(),
        b"\xef\xbb\xbf\xef\xbb\xbf\r\n",
        b"\xf0\x9f\x99",
        b"\xef\xbb",
        b"\xe0\x80\x80\xed\xa0\x80\xf4\x90\x80\x80\xc0\xaf",
        b"\xe2\x82X\xf0\x9f\r\n\xff\x80\xe2\x82\xac",
    ];
    for bytes in cases {
        let expected = reference(bytes);
        for split in 0..=bytes.len() {
            assert_eq!(decode(&[&bytes[..split], b"", &bytes[split..]]), expected);
        }
        let chunks: Vec<_> = bytes.chunks(1).collect();
        assert_eq!(decode(&chunks), expected);
    }
}

#[test]
fn every_two_byte_input_matches_an_independent_lossy_decode() {
    for first in 0..=u8::MAX {
        for second in 0..=u8::MAX {
            let bytes = [first, second];
            let expected = reference(&bytes);
            assert_eq!(decode(&[&bytes]), expected, "{bytes:x?}");
            assert_eq!(decode(&[&bytes[..1], &bytes[1..]]), expected, "{bytes:x?}");
        }
    }
}

#[test]
fn original_bytes_are_charged_before_bom_and_newline_removal() {
    let mut input = Utf8Input::new(5, CancellationToken::default());
    assert_eq!(input.feed(b"\xef\xbb").unwrap(), "");
    assert_eq!(input.feed(b"\xbf\r\n").unwrap(), "\n");
    assert_eq!(input.bytes_consumed(), 5);
    assert_eq!(input.feed(b"x"), Err(HtmlError::InputLimit));
    assert_eq!(input.bytes_consumed(), 5);
    assert_eq!(input.feed(b""), Err(HtmlError::Poisoned));
    assert_eq!(input.finish(), Err(HtmlError::Poisoned));

    let mut empty = Utf8Input::new(0, CancellationToken::default());
    assert_eq!(empty.feed(b"").unwrap(), "");
    assert_eq!(empty.finish().unwrap(), "");
}

#[test]
fn finish_flushes_a_truncated_scalar_once_and_closes_input() {
    let mut input = Utf8Input::new(3, CancellationToken::default());
    assert_eq!(input.feed(b"\xf0\x9f\x99").unwrap(), "");
    assert_eq!(input.finish().unwrap(), "\u{fffd}");
    assert_eq!(input.finish(), Err(HtmlError::AlreadyFinished));
    assert_eq!(input.feed(b""), Err(HtmlError::AlreadyFinished));
}

#[test]
fn cancellation_poisoning_applies_to_feed_and_finish() {
    let cancellation = CancellationToken::default();
    let mut input = Utf8Input::new(10, cancellation.clone());
    assert_eq!(input.feed(b"a").unwrap(), "a");
    cancellation.cancel();
    assert_eq!(input.feed(b"b"), Err(HtmlError::Cancelled));
    assert_eq!(input.bytes_consumed(), 1);
    assert_eq!(input.finish(), Err(HtmlError::Poisoned));

    let mut input = Utf8Input::new(0, cancellation);
    assert_eq!(input.finish(), Err(HtmlError::Cancelled));
    assert_eq!(input.feed(b""), Err(HtmlError::Poisoned));
}
