use rustic_base::CancellationToken;
use rustic_html::{
    Diagnostic, DiagnosticKind as D, HtmlError, HtmlTokenizer, Token, TokenizerLimits,
};

fn tokenize(chunks: &[&[u8]], limits: TokenizerLimits) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut tokenizer = HtmlTokenizer::new(limits, CancellationToken::default()).unwrap();
    let mut tokens = Vec::new();
    for chunk in chunks {
        tokens.extend(tokenizer.feed(chunk).unwrap());
    }
    tokens.extend(tokenizer.finish().unwrap());
    (tokens, tokenizer.diagnostics().to_vec())
}

fn parse(input: &str) -> (Vec<Token>, Vec<D>) {
    let (tokens, errors) = tokenize(&[input.as_bytes()], TokenizerLimits::default());
    (tokens, errors.iter().map(|d| d.kind).collect())
}

fn text(value: &str) -> Vec<Token> {
    vec![Token::Characters(value.to_owned()), Token::Eof]
}

#[test]
fn longest_named_match_preserves_case_and_two_scalar_values() {
    let (tokens, errors) = parse("&notin;|&notit;|&NotEqualTilde;|&amp;|&AMP;|&unknown;");
    assert_eq!(tokens, text("∉|¬it;|≂\u{338}|&|&|&unknown;"));
    assert_eq!(
        errors,
        [
            D::MissingSemicolonAfterCharacterReference,
            D::UnknownNamedCharacterReference
        ]
    );
}

#[test]
fn attribute_ambiguity_and_decoded_quotes_do_not_change_tokenizer_state() {
    let (tokens, errors) = parse("<a x='&notit; &copy= &copy &notin;' y=&quot;&amp;>");
    let Token::StartTag(tag) = &tokens[0] else {
        panic!("{tokens:?}")
    };
    assert_eq!(
        tag.attributes()
            .iter()
            .map(|a| (a.name(), a.value()))
            .collect::<Vec<_>>(),
        [("x", "&notit; &copy= © ∉"), ("y", "\"&")]
    );
    assert_eq!(errors, [D::MissingSemicolonAfterCharacterReference]);
}

#[test]
fn decoded_markup_is_character_data_and_is_not_tokenized_again() {
    let (tokens, errors) = parse("&lt;script&gt;&amp;lt; &quot;&#39;&#62;&#60;");
    assert_eq!(tokens, text("<script>&lt; \"'><"));
    assert!(errors.is_empty());
    let (tokens, errors) = parse("A\r\n&#13;B");
    assert_eq!(tokens, text("A\n\rB"));
    assert_eq!(errors, [D::ControlCharacterReference]);
}

#[test]
fn numeric_references_apply_unicode_recovery_and_legacy_control_mapping() {
    for (source, value, error) in [
        ("&#128;", "€", Some(D::ControlCharacterReference)),
        ("&#x81;", "\u{81}", Some(D::ControlCharacterReference)),
        ("&#0;", "\u{fffd}", Some(D::NullCharacterReference)),
        ("&#xD800;", "\u{fffd}", Some(D::SurrogateCharacterReference)),
        (
            "&#1114112;",
            "\u{fffd}",
            Some(D::CharacterReferenceOutsideUnicodeRange),
        ),
        (
            "&#4294967296;",
            "\u{fffd}",
            Some(D::CharacterReferenceOutsideUnicodeRange),
        ),
        (
            "&#xFFFFFFFFFFFFFFFF;",
            "\u{fffd}",
            Some(D::CharacterReferenceOutsideUnicodeRange),
        ),
        (
            "&#xFDD0;",
            "\u{fdd0}",
            Some(D::NoncharacterCharacterReference),
        ),
        (
            "&#x10FFFF;",
            "\u{10ffff}",
            Some(D::NoncharacterCharacterReference),
        ),
        ("&#9;", "\t", None),
        ("&#11;", "\u{b}", Some(D::ControlCharacterReference)),
        ("&#x1f642;", "🙂", None),
        ("&#xFFFD;", "\u{fffd}", None),
    ] {
        let (tokens, errors) = parse(source);
        assert_eq!(tokens, text(value), "{source}");
        assert_eq!(errors, error.into_iter().collect::<Vec<_>>(), "{source}");
    }
    assert_eq!(
        parse("&#65x"),
        (text("Ax"), vec![D::MissingSemicolonAfterCharacterReference])
    );
    assert_eq!(
        parse("&#0"),
        (
            text("\u{fffd}"),
            vec![
                D::MissingSemicolonAfterCharacterReference,
                D::NullCharacterReference
            ]
        )
    );
}

#[test]
fn incomplete_and_unknown_references_preserve_literal_input() {
    for (source, errors) in [
        ("&", vec![]),
        ("&#", vec![D::AbsenceOfDigitsInNumericReference]),
        ("&#x", vec![D::AbsenceOfDigitsInNumericReference]),
        ("&#Xz;", vec![D::AbsenceOfDigitsInNumericReference]),
        ("&#;", vec![D::AbsenceOfDigitsInNumericReference]),
        ("&bogus", vec![]),
        ("&bogus;", vec![D::UnknownNamedCharacterReference]),
        ("&;", vec![]),
    ] {
        assert_eq!(parse(source), (text(source), errors), "{source}");
    }
    assert_eq!(
        parse("&amp"),
        (text("&"), vec![D::MissingSemicolonAfterCharacterReference])
    );
}

#[test]
fn eof_resolves_a_reference_before_unfinished_attribute_recovery() {
    assert_eq!(
        parse("<a x='&amp"),
        (
            vec![Token::Eof],
            vec![D::MissingSemicolonAfterCharacterReference, D::EofInTag]
        )
    );
    assert_eq!(
        parse("<a x=&#x"),
        (
            vec![Token::Eof],
            vec![D::AbsenceOfDigitsInNumericReference, D::EofInTag]
        )
    );
    assert_eq!(parse("<a x='&noti"), (vec![Token::Eof], vec![D::EofInTag]));
}

#[test]
fn reference_tokens_and_diagnostics_are_invariant_at_all_byte_splits() {
    for source in [
        "\u{feff}🙂\r\n&CounterClockwiseContourIntegral;&notit;&bogus;",
        "<a x='&copy= &copy &notin;' y=&quot;&amp;>",
        "&#x1f642;&#0;&#xD800;&#999999999999999999999999999999999999;",
        "&NotEqualTilde;&amp",
        "&&#xZ;",
        "<a x='&noti",
        "&#x",
        "&bogus",
    ] {
        let bytes = source.as_bytes();
        let limits = TokenizerLimits {
            max_text_chunk_bytes: 5,
            ..TokenizerLimits::default()
        };
        let expected = tokenize(&[bytes], limits);
        for split in 0..=bytes.len() {
            assert_eq!(
                tokenize(&[&bytes[..split], b"", &bytes[split..]], limits),
                expected,
                "{source:?}, split {split}"
            );
        }
        assert_eq!(
            tokenize(&bytes.chunks(1).collect::<Vec<_>>(), limits),
            expected
        );
    }
}

#[test]
fn missing_semicolon_offsets_follow_normalized_source_not_expanded_output() {
    let (tokens, errors) = tokenize(
        &["\u{feff}🙂\r\n&amp X".as_bytes()],
        TokenizerLimits::default(),
    );
    assert_eq!(tokens, text("🙂\n& X"));
    assert_eq!(
        errors,
        [Diagnostic {
            kind: D::MissingSemicolonAfterCharacterReference,
            normalized_offset: 6
        }]
    );
    let (_, errors) = tokenize(&[b"&noti;"], TokenizerLimits::default());
    assert_eq!(
        errors,
        [Diagnostic {
            kind: D::MissingSemicolonAfterCharacterReference,
            normalized_offset: 4
        }]
    );
}

fn assert_fatal(source: &str, limits: TokenizerLimits, error: HtmlError) {
    let mut tokenizer = HtmlTokenizer::new(limits, CancellationToken::default()).unwrap();
    assert_eq!(
        tokenizer
            .feed(source.as_bytes())
            .and_then(|_| tokenizer.finish()),
        Err(error)
    );
    assert_eq!(tokenizer.feed(b""), Err(HtmlError::Poisoned));
    assert_eq!(tokenizer.finish(), Err(HtmlError::Poisoned));
}

#[test]
fn expanded_payloads_and_reference_errors_respect_resource_limits() {
    let limits = TokenizerLimits {
        max_token_bytes: 5,
        max_text_chunk_bytes: 4,
        ..TokenizerLimits::default()
    };
    let (tokens, errors) = tokenize(&[b"<a x='&#128;'>"], limits);
    let Token::StartTag(tag) = &tokens[0] else {
        panic!("{tokens:?}")
    };
    assert_eq!(tag.attributes()[0].value(), "€");
    assert_eq!(errors.len(), 1);
    assert_fatal("<a x='&#128;z'>", limits, HtmlError::TokenBytesLimit);
    assert_fatal(
        "&Afr;",
        TokenizerLimits {
            max_text_chunk_bytes: 2,
            ..limits
        },
        HtmlError::TokenBytesLimit,
    );
    assert_fatal(
        "&amp;",
        TokenizerLimits {
            max_tokens: 1,
            ..limits
        },
        HtmlError::TokenLimit,
    );
    assert_fatal(
        "&amp",
        TokenizerLimits {
            max_diagnostics: 0,
            ..limits
        },
        HtmlError::DiagnosticLimit,
    );
    assert_fatal(
        "&amp;",
        TokenizerLimits {
            max_input_bytes: 4,
            ..limits
        },
        HtmlError::InputLimit,
    );
}

#[test]
fn discarded_duplicate_values_still_consume_reference_budgets_and_diagnostics() {
    let (tokens, errors) = parse("<a x=1 x=&#0;>");
    let Token::StartTag(tag) = &tokens[0] else {
        panic!("{tokens:?}")
    };
    assert_eq!(tag.attributes().len(), 1);
    assert_eq!(tag.attributes()[0].value(), "1");
    assert_eq!(errors, [D::DuplicateAttribute, D::NullCharacterReference]);
    assert_fatal(
        "<a x=1 x=&#0;>",
        TokenizerLimits {
            max_token_bytes: 6,
            max_text_chunk_bytes: 4,
            ..TokenizerLimits::default()
        },
        HtmlError::TokenBytesLimit,
    );
}

#[test]
fn long_numeric_inputs_do_not_overflow_and_remain_cancellable_and_bounded() {
    let zeros = format!("&#{};", "0".repeat(10_000));
    let (tokens, errors) = parse(&zeros);
    assert_eq!(tokens, text("\u{fffd}"));
    assert_eq!(errors, [D::NullCharacterReference]);
    let huge = format!("&#{};", "9".repeat(10_000));
    let (tokens, errors) = parse(&huge);
    assert_eq!(tokens, text("\u{fffd}"));
    assert_eq!(errors, [D::CharacterReferenceOutsideUnicodeRange]);
    assert_fatal(
        &huge,
        TokenizerLimits {
            max_input_bytes: 1024,
            ..TokenizerLimits::default()
        },
        HtmlError::InputLimit,
    );
    let cancellation = CancellationToken::default();
    let mut tokenizer =
        HtmlTokenizer::new(TokenizerLimits::default(), cancellation.clone()).unwrap();
    assert!(tokenizer.feed(b"&Count").unwrap().is_empty());
    cancellation.cancel();
    assert_eq!(
        tokenizer.feed(b"erClockwiseContourIntegral;"),
        Err(HtmlError::Cancelled)
    );
    assert_eq!(tokenizer.finish(), Err(HtmlError::Poisoned));
}
