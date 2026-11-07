use rustic_base::CancellationToken;
use rustic_html::{Diagnostic, DiagnosticKind, HtmlError, HtmlTokenizer, Token, TokenizerLimits};

fn tokenize(chunks: &[&[u8]], limits: TokenizerLimits) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut tokenizer = HtmlTokenizer::new(limits, CancellationToken::default()).unwrap();
    let mut tokens = Vec::new();
    for chunk in chunks {
        tokens.extend(tokenizer.feed(chunk).unwrap());
    }
    tokens.extend(tokenizer.finish().unwrap());
    (tokens, tokenizer.diagnostics().to_vec())
}

fn character(value: &str) -> Token {
    Token::Characters(value.to_owned())
}

fn assert_start(token: &Token, name: &str, attributes: &[(&str, &str)], self_closing: bool) {
    let Token::StartTag(tag) = token else {
        panic!("expected a start tag, got {token:?}");
    };
    assert_eq!(tag.name(), name);
    assert_eq!(tag.self_closing(), self_closing);
    let actual: Vec<_> = tag
        .attributes()
        .iter()
        .map(|a| (a.name(), a.value()))
        .collect();
    assert_eq!(actual, attributes);
}

#[test]
fn tags_attributes_and_text_follow_the_declared_subset() {
    let (tokens, diagnostics) = tokenize(
        &["\u{feff}<DiV ID='First' id=second CLASS=AbC disabled data-É='🙂'>Hi\r\n中<BR />there</DIV>".as_bytes()],
        TokenizerLimits::default(),
    );
    assert_eq!(tokens.len(), 6);
    assert_start(
        &tokens[0],
        "div",
        &[
            ("id", "First"),
            ("class", "AbC"),
            ("disabled", ""),
            ("data-É", "🙂"),
        ],
        false,
    );
    assert_eq!(tokens[1], character("Hi\n中"));
    assert_start(&tokens[2], "br", &[], true);
    assert_eq!(tokens[3], character("there"));
    assert_eq!(tokens[4], Token::EndTag("div".to_owned()));
    assert_eq!(tokens[5..], [Token::Eof]);
    assert_eq!(
        diagnostics.iter().map(|d| d.kind).collect::<Vec<_>>(),
        [DiagnosticKind::DuplicateAttribute]
    );
}

#[test]
fn solidus_in_unquoted_value_is_data_not_a_self_closing_flag() {
    let (tokens, diagnostics) = tokenize(
        &[b"<a href=/path/><b href=/path/ />"],
        TokenizerLimits::default(),
    );
    assert_start(&tokens[0], "a", &[("href", "/path/")], false);
    assert_start(&tokens[1], "b", &[("href", "/path/")], true);
    assert!(diagnostics.is_empty());
}

#[test]
fn recovery_keeps_first_attribute_and_replaces_null_only_in_tag_data() {
    let (tokens, diagnostics) = tokenize(
        &[b"a\0<b\0 X\0='v\0' x\0=ignored></b q=1/>"],
        TokenizerLimits::default(),
    );
    assert_eq!(tokens[0], character("a\0"));
    assert_start(
        &tokens[1],
        "b\u{fffd}",
        &[("x\u{fffd}", "v\u{fffd}")],
        false,
    );
    assert_eq!(tokens[2], Token::EndTag("b".to_owned()));
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UnexpectedNull)
            .count(),
        5
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::DuplicateAttribute)
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::EndTagAttributes)
    );
    // The slash belongs to the unquoted end-tag attribute value.
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::EndTagSolidus)
    );
    let (_, diagnostics) = tokenize(&[b"</b/>"], TokenizerLimits::default());
    assert_eq!(diagnostics[0].kind, DiagnosticKind::EndTagSolidus);
}

#[test]
fn malformed_supported_states_recover_without_losing_following_input() {
    let (tokens, diagnostics) = tokenize(
        &[b"<3</><a =x q= r='v's=t / z=\"ok\" >"],
        TokenizerLimits::default(),
    );
    assert_eq!(tokens[0], character("<3"));
    assert_start(
        &tokens[1],
        "a",
        &[("=x", ""), ("q", "r='v's=t"), ("z", "ok")],
        false,
    );
    assert_eq!(tokens[2], Token::Eof);
    let kinds: Vec<_> = diagnostics.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds[0..3],
        [
            DiagnosticKind::InvalidTagStart,
            DiagnosticKind::MissingEndTagName,
            DiagnosticKind::UnexpectedEquals
        ]
    );
    assert!(kinds.contains(&DiagnosticKind::UnexpectedAttributeCharacter));
    assert!(kinds.contains(&DiagnosticKind::UnexpectedSolidus));

    let (tokens, diagnostics) = tokenize(&[b"<a x='v'y=2 z=>"], TokenizerLimits::default());
    assert_start(&tokens[0], "a", &[("x", "v"), ("y", "2"), ("z", "")], false);
    assert_eq!(
        diagnostics.iter().map(|d| d.kind).collect::<Vec<_>>(),
        [
            DiagnosticKind::MissingAttributeWhitespace,
            DiagnosticKind::MissingAttributeValue
        ]
    );
}

#[test]
fn eof_recovery_is_explicit_for_every_unfinished_state() {
    for (bytes, literal) in [(b"<".as_slice(), "<"), (b"</".as_slice(), "</")] {
        let (tokens, diagnostics) = tokenize(&[bytes], TokenizerLimits::default());
        assert_eq!(tokens, [character(literal), Token::Eof]);
        assert_eq!(diagnostics[0].kind, DiagnosticKind::EofBeforeTagName);
        assert_eq!(diagnostics[0].normalized_offset, bytes.len());
    }
    for bytes in [
        "<a", "<a ", "<a x", "<a x ", "<a x=", "<a x='", "<a x=v", "<a x=''", "<a /",
    ] {
        let (tokens, diagnostics) = tokenize(&[bytes.as_bytes()], TokenizerLimits::default());
        assert_eq!(tokens, [Token::Eof], "{bytes}");
        assert_eq!(
            diagnostics.last().unwrap().kind,
            DiagnosticKind::EofInTag,
            "{bytes}"
        );
    }
}

#[test]
fn tokens_and_diagnostics_are_independent_of_chunk_boundaries() {
    let cases: &[&[u8]] = &[
        "\u{feff}🙂\r\n<a X='é中' x=2 y=z/>\0<3</>after</A >".as_bytes(),
        b"<a x='v'y=2 /z=>\xff\xe2\x82\xac\xf0\x9f\x99",
        b"<",
        b"</",
        b"<a x='unterminated",
        b"text\r\nmore\r\r\n",
    ];
    let limits = TokenizerLimits {
        max_text_chunk_bytes: 5,
        ..TokenizerLimits::default()
    };
    for bytes in cases {
        let expected = tokenize(&[bytes], limits);
        for split in 0..=bytes.len() {
            assert_eq!(
                tokenize(&[&bytes[..split], b"", &bytes[split..]], limits),
                expected,
                "split {split}: {bytes:x?}"
            );
        }
        assert_eq!(
            tokenize(&bytes.chunks(1).collect::<Vec<_>>(), limits),
            expected
        );
    }
}

#[test]
fn text_chunks_respect_utf8_boundaries_and_offsets_count_normalized_scalars() {
    let limits = TokenizerLimits {
        max_text_chunk_bytes: 5,
        ..TokenizerLimits::default()
    };
    let (tokens, diagnostics) = tokenize(&["\u{feff}🙂é\r\n\0abc".as_bytes()], limits);
    assert_eq!(
        tokens,
        [
            character("🙂"),
            character("é\n\0a"),
            character("bc"),
            Token::Eof
        ]
    );
    assert_eq!(
        diagnostics,
        [Diagnostic {
            kind: DiagnosticKind::UnexpectedNull,
            normalized_offset: 3
        }]
    );
}

#[test]
fn duplicate_attributes_are_diagnosed_at_the_end_of_the_name() {
    let (_, diagnostics) = tokenize(&[b"<a X=1 x=2>"], TokenizerLimits::default());
    assert_eq!(
        diagnostics,
        [Diagnostic {
            kind: DiagnosticKind::DuplicateAttribute,
            normalized_offset: 8
        }]
    );
    let (_, diagnostics) = tokenize(&[b"<a x x"], TokenizerLimits::default());
    assert_eq!(
        diagnostics,
        [
            Diagnostic {
                kind: DiagnosticKind::DuplicateAttribute,
                normalized_offset: 6
            },
            Diagnostic {
                kind: DiagnosticKind::EofInTag,
                normalized_offset: 6
            }
        ]
    );
}

fn assert_fatal(bytes: &[u8], limits: TokenizerLimits, expected: HtmlError) {
    let mut tokenizer = HtmlTokenizer::new(limits, CancellationToken::default()).unwrap();
    let result = tokenizer.feed(bytes).and_then(|_| tokenizer.finish());
    assert_eq!(result, Err(expected));
    assert_eq!(tokenizer.feed(b""), Err(HtmlError::Poisoned));
    assert_eq!(tokenizer.finish(), Err(HtmlError::Poisoned));
}

#[test]
fn exact_resource_boundaries_are_enforced_before_further_output() {
    let limits = TokenizerLimits {
        max_input_bytes: 7,
        max_tokens: 2,
        max_token_bytes: 3,
        max_text_chunk_bytes: 3,
        max_attributes: 1,
        max_diagnostics: 0,
    };
    let (tokens, diagnostics) = tokenize(&[b"<a x=v>"], limits);
    assert_start(&tokens[0], "a", &[("x", "v")], false);
    assert_eq!(tokens[1], Token::Eof);
    assert!(diagnostics.is_empty());
    assert_fatal(
        b"<a x=vv>",
        TokenizerLimits {
            max_input_bytes: 8,
            ..limits
        },
        HtmlError::TokenBytesLimit,
    );
    assert_fatal(b"<a x=v> ", limits, HtmlError::InputLimit);
    assert_fatal(
        b"<a x x>",
        TokenizerLimits {
            max_diagnostics: 1,
            ..limits
        },
        HtmlError::AttributeLimit,
    );
    assert_fatal(b"\0", limits, HtmlError::DiagnosticLimit);
    assert_fatal(
        b"abcd",
        TokenizerLimits {
            max_tokens: 2,
            ..limits
        },
        HtmlError::TokenLimit,
    );
    assert_fatal("🙂".as_bytes(), limits, HtmlError::TokenBytesLimit);

    // Rejected duplicates consume both occurrence and cumulative tag-data limits.
    assert_fatal(
        b"<a x=1 x=22>",
        TokenizerLimits {
            max_token_bytes: 5,
            max_text_chunk_bytes: 5,
            ..TokenizerLimits::default()
        },
        HtmlError::TokenBytesLimit,
    );
    let (tokens, diagnostics) = tokenize(
        &[b"\0"],
        TokenizerLimits {
            max_diagnostics: 1,
            ..TokenizerLimits::default()
        },
    );
    assert_eq!(tokens, [character("\0"), Token::Eof]);
    assert_eq!(diagnostics.len(), 1);
}

#[test]
fn unsupported_syntax_fails_instead_of_silently_changing_meaning() {
    for bytes in ["a&b", "<a x='&amp;'>", "<a x=&gt;>"] {
        assert_fatal(
            bytes.as_bytes(),
            TokenizerLimits::default(),
            HtmlError::UnsupportedCharacterReference,
        );
    }
    let (tokens, _) = tokenize(&[b"<a&b x&y=z>"], TokenizerLimits::default());
    assert_start(&tokens[0], "a&b", &[("x&y", "z")], false);
}

#[test]
fn lifecycle_and_cancellation_terminate_the_stream() {
    let mut tokenizer = HtmlTokenizer::default();
    assert_eq!(tokenizer.finish().unwrap(), [Token::Eof]);
    assert_eq!(tokenizer.finish(), Err(HtmlError::AlreadyFinished));
    assert_eq!(tokenizer.feed(b""), Err(HtmlError::AlreadyFinished));

    let cancellation = CancellationToken::default();
    let mut tokenizer =
        HtmlTokenizer::new(TokenizerLimits::default(), cancellation.clone()).unwrap();
    assert_eq!(tokenizer.feed(b"<a>").unwrap().len(), 1);
    cancellation.cancel();
    assert_eq!(tokenizer.feed(b"x"), Err(HtmlError::Cancelled));
    assert_eq!(tokenizer.finish(), Err(HtmlError::Poisoned));
    let mut tokenizer = HtmlTokenizer::new(TokenizerLimits::default(), cancellation).unwrap();
    assert_eq!(tokenizer.finish(), Err(HtmlError::Cancelled));
}

#[test]
fn invalid_chunk_limits_are_rejected_but_zero_other_budgets_are_usable() {
    for limits in [
        TokenizerLimits {
            max_text_chunk_bytes: 0,
            ..TokenizerLimits::default()
        },
        TokenizerLimits {
            max_token_bytes: 1,
            max_text_chunk_bytes: 2,
            ..TokenizerLimits::default()
        },
    ] {
        assert!(matches!(
            HtmlTokenizer::new(limits, CancellationToken::default()),
            Err(HtmlError::InvalidLimits)
        ));
    }
    let limits = TokenizerLimits {
        max_input_bytes: 0,
        max_tokens: 1,
        max_attributes: 0,
        max_diagnostics: 0,
        ..TokenizerLimits::default()
    };
    assert_eq!(tokenize(&[b""], limits), (vec![Token::Eof], vec![]));
    assert_fatal(
        b"",
        TokenizerLimits {
            max_tokens: 0,
            ..limits
        },
        HtmlError::TokenLimit,
    );
}
