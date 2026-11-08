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

fn assert_doctype(
    token: &Token,
    name: Option<&str>,
    public: Option<&str>,
    system: Option<&str>,
    force: bool,
) {
    let Token::Doctype(record) = token else {
        panic!("expected doctype, got {token:?}")
    };
    assert_eq!(
        (
            record.name(),
            record.public_id(),
            record.system_id(),
            record.force_quirks()
        ),
        (name, public, system, force)
    );
}

#[test]
fn comments_preserve_literal_references_and_recover_from_malformed_delimiters() {
    for (source, data, errors) in [
        ("<!--é🙂 &amp;\r\nx-->", "é🙂 &amp;\nx", vec![]),
        ("<!---->", "", vec![]),
        ("<!-->", "", vec![D::AbruptClosingOfEmptyComment]),
        ("<!--->", "", vec![D::AbruptClosingOfEmptyComment]),
        ("<!--a--!>", "a", vec![D::IncorrectlyClosedComment]),
        ("<!--a<!--b-->", "a<!--b", vec![D::NestedComment]),
        ("<!--a<!-->", "a<!", vec![]),
        ("<!--a-b--b--!b-->", "a-b--b--!b", vec![]),
        ("<!--\0-->", "\u{fffd}", vec![D::UnexpectedNull]),
    ] {
        let (tokens, actual) = parse(source);
        assert_eq!(
            tokens,
            [Token::Comment(data.to_owned()), Token::Eof],
            "{source:?}"
        );
        assert_eq!(actual, errors, "{source:?}");
    }
}

#[test]
fn eof_in_each_comment_state_emits_only_data_already_committed() {
    for (source, data) in [
        ("<!--", ""),
        ("<!---", ""),
        ("<!--x", "x"),
        ("<!--x<", "x<"),
        ("<!--x<!", "x<!"),
        ("<!--x<!-", "x<!"),
        ("<!--x<!--", "x<!"),
        ("<!--x-", "x"),
        ("<!--x--", "x"),
        ("<!--x--!", "x"),
    ] {
        assert_eq!(
            parse(source),
            (
                vec![Token::Comment(data.to_owned()), Token::Eof],
                vec![D::EofInComment]
            ),
            "{source}"
        );
    }
}

#[test]
fn unknown_declarations_and_html_cdata_recover_as_bogus_comments() {
    for (source, data, error) in [
        ("<!unknown>", "unknown", D::IncorrectlyOpenedComment),
        ("<!>", "", D::IncorrectlyOpenedComment),
        ("<!-x>", "-x", D::IncorrectlyOpenedComment),
        ("<!DOCT", "DOCT", D::IncorrectlyOpenedComment),
        ("<![CDA", "[CDA", D::IncorrectlyOpenedComment),
        ("<!", "", D::IncorrectlyOpenedComment),
        ("</!>", "!", D::InvalidTagStart),
        ("<![CDATA[x]]>", "[CDATA[x]]", D::CdataInHtmlContent),
        ("<![cdata[x]]>", "[cdata[x]]", D::IncorrectlyOpenedComment),
    ] {
        assert_eq!(
            parse(source),
            (
                vec![Token::Comment(data.to_owned()), Token::Eof],
                vec![error]
            ),
            "{source}"
        );
    }
    let (tokens, errors) = parse("<!DOCT\0>tail");
    assert_eq!(
        tokens,
        [
            Token::Comment("DOCT\u{fffd}".to_owned()),
            Token::Characters("tail".to_owned()),
            Token::Eof
        ]
    );
    assert_eq!(errors, [D::IncorrectlyOpenedComment, D::UnexpectedNull]);
}

#[test]
fn doctype_identifiers_preserve_missing_empty_case_and_literal_references() {
    let (tokens, errors) = parse("<!dOcTyPe HTML PUBLIC '' 'HTTPS://A.test/&x;'>");
    assert_doctype(
        &tokens[0],
        Some("html"),
        Some(""),
        Some("HTTPS://A.test/&x;"),
        false,
    );
    assert_eq!(tokens[1], Token::Eof);
    assert!(errors.is_empty());
    let (tokens, errors) = parse("<!DOCTYPE \0A SYSTEM 'x\0'>");
    assert_doctype(
        &tokens[0],
        Some("\u{fffd}a"),
        None,
        Some("x\u{fffd}"),
        false,
    );
    assert_eq!(errors, [D::UnexpectedNull, D::UnexpectedNull]);
    let (tokens, errors) = parse("<!DOCTYPEhtml PUBLIC\"A\"\"B\">");
    assert_doctype(&tokens[0], Some("html"), Some("A"), Some("B"), false);
    assert_eq!(
        errors,
        [
            D::MissingWhitespaceBeforeDoctypeName,
            D::MissingWhitespaceAfterDoctypePublicKeyword,
            D::MissingWhitespaceBetweenDoctypeIdentifiers
        ]
    );
}

#[test]
fn doctype_recovery_preserves_the_force_quirks_rules() {
    let (tokens, errors) = parse("<!DOCTYPE>");
    assert_doctype(&tokens[0], None, None, None, true);
    assert_eq!(errors, [D::MissingDoctypeName]);
    for (source, error) in [
        (
            "<!DOCTYPE html garbage>",
            D::InvalidSequenceAfterDoctypeName,
        ),
        ("<!DOCTYPE html PUBLIC >", D::MissingDoctypePublicIdentifier),
        (
            "<!DOCTYPE html PUBLIC unquoted>",
            D::MissingQuoteBeforeDoctypePublicIdentifier,
        ),
        ("<!DOCTYPE html SYSTEM >", D::MissingDoctypeSystemIdentifier),
        (
            "<!DOCTYPE html SYSTEM unquoted>",
            D::MissingQuoteBeforeDoctypeSystemIdentifier,
        ),
    ] {
        let (tokens, errors) = parse(source);
        assert_doctype(&tokens[0], Some("html"), None, None, true);
        assert_eq!(errors, [error]);
    }
    for source in [
        "<!DOCTYPE html SYSTEM '' garbage>",
        "<!DOCTYPE html SYSTEM '' garbage",
    ] {
        let (tokens, errors) = parse(source);
        assert_doctype(&tokens[0], Some("html"), None, Some(""), false);
        assert_eq!(errors, [D::UnexpectedCharacterAfterDoctypeSystemIdentifier]);
    }
    let (tokens, errors) = parse("<!DOCTYPE html PUBLIC 'x>tail");
    assert_doctype(&tokens[0], Some("html"), Some("x"), None, true);
    assert_eq!(tokens[1], Token::Characters("tail".to_owned()));
    assert_eq!(errors, [D::AbruptDoctypePublicIdentifier]);
}

#[test]
fn doctype_eof_distinguishes_partial_keywords_from_missing_delimiters() {
    for source in [
        "<!DOCTYPE",
        "<!DOCTYPE ",
        "<!DOCTYPE html",
        "<!DOCTYPE html ",
        "<!DOCTYPE html PUBLIC",
        "<!DOCTYPE html PUBLIC ",
        "<!DOCTYPE html PUBLIC '",
        "<!DOCTYPE html PUBLIC ''",
        "<!DOCTYPE html PUBLIC '' ",
        "<!DOCTYPE html SYSTEM",
        "<!DOCTYPE html SYSTEM '",
        "<!DOCTYPE html SYSTEM ''",
    ] {
        let (tokens, errors) = parse(source);
        let Token::Doctype(record) = &tokens[0] else {
            panic!("{source}")
        };
        assert!(record.force_quirks());
        assert_eq!(errors, [D::EofInDoctype], "{source}");
        assert_eq!(tokens[1], Token::Eof);
    }
    for prefix in [
        "P", "PU", "PUB", "PUBL", "PUBLI", "S", "SY", "SYS", "SYST", "SYSTE",
    ] {
        let (tokens, errors) = parse(&format!("<!DOCTYPE html {prefix}"));
        assert_doctype(&tokens[0], Some("html"), None, None, true);
        assert_eq!(errors, [D::InvalidSequenceAfterDoctypeName]);
    }
}

#[test]
fn processing_instructions_preserve_targets_data_and_question_mark_boundaries() {
    let (tokens, errors) = parse("<?_Task-2 \tHello&\0??>tail");
    let Token::ProcessingInstruction(record) = &tokens[0] else {
        panic!("{tokens:?}")
    };
    assert_eq!(record.target(), "_Task-2");
    assert_eq!(record.data(), "Hello&\0?");
    assert_eq!(tokens[1], Token::Characters("tail".to_owned()));
    assert!(errors.is_empty());
    for (source, data, error) in [
        ("<?1x>", "?1x", D::InvalidFirstInstructionTarget),
        ("<?é>", "?é", D::InvalidFirstInstructionTarget),
        ("<?good.bad>", "?good.bad", D::InvalidInstructionTarget),
        (
            "<?XML version='1.0'?>",
            "?XML version='1.0'?",
            D::DisallowedInstructionTarget,
        ),
        (
            "<?xml-stylesheet>",
            "?xml-stylesheet",
            D::DisallowedInstructionTarget,
        ),
    ] {
        assert_eq!(
            parse(source),
            (
                vec![Token::Comment(data.to_owned()), Token::Eof],
                vec![error]
            ),
            "{source}"
        );
    }
}

#[test]
fn eof_discards_unfinished_processing_instructions_in_all_states() {
    for source in [
        "<?",
        "<?target",
        "<?target ",
        "<?target data",
        "<?target data?",
    ] {
        assert_eq!(
            parse(source),
            (vec![Token::Eof], vec![D::EofInProcessingInstruction])
        );
    }
}

#[test]
fn declarations_are_invariant_at_every_byte_split_and_one_byte_feeds() {
    let cases = [
        "\u{feff}before<!--🙂\r\n&x;\0<!--nested--!>after",
        "<!DOCTYPE HTML PUBLIC 'a\0&b' \"Case\">tail",
        "<!DOCTYPE html PUBLIC\"\"\"\">",
        "<!DOCTYPE html PUB>",
        "<!DOCTYPE html PUBL",
        "<!DOCT\0>tail",
        "<![CDATA[🙂]]>",
        "<?Task a\0??>tail",
        "<?xml-stylesheet type='x'?>",
        "<?good.🙂>",
        "<!--x<!--",
        "<!--x--!",
        "<!-",
        "</!>",
        "<?x ",
    ];
    for source in cases {
        let bytes = source.as_bytes();
        let expected = tokenize(&[bytes], TokenizerLimits::default());
        for split in 0..=bytes.len() {
            assert_eq!(
                tokenize(
                    &[&bytes[..split], b"", &bytes[split..]],
                    TokenizerLimits::default()
                ),
                expected,
                "{source:?}, split {split}"
            );
        }
        assert_eq!(
            tokenize(
                &bytes.chunks(1).collect::<Vec<_>>(),
                TokenizerLimits::default()
            ),
            expected
        );
    }
}

fn assert_fatal(source: &str, limits: TokenizerLimits, error: HtmlError) {
    let mut tokenizer = HtmlTokenizer::new(limits, CancellationToken::default()).unwrap();
    let result = tokenizer
        .feed(source.as_bytes())
        .and_then(|_| tokenizer.finish());
    assert_eq!(result, Err(error), "{source:?}");
    assert_eq!(tokenizer.feed(b""), Err(HtmlError::Poisoned));
    assert_eq!(tokenizer.finish(), Err(HtmlError::Poisoned));
}

#[test]
fn declaration_payloads_share_exact_byte_budgets_and_reset_between_tokens() {
    let limits = TokenizerLimits {
        max_token_bytes: 8,
        max_text_chunk_bytes: 4,
        ..TokenizerLimits::default()
    };
    let (tokens, _) = tokenize(
        &["<!--🙂🙂--><!--12345678--><!DOCTYPE html PUBLIC 'p' 'abc'><?abc 12345>".as_bytes()],
        limits,
    );
    assert_eq!(tokens.len(), 5);
    assert_fatal("<!--🙂🙂x-->", limits, HtmlError::TokenBytesLimit);
    assert_fatal(
        "<!DOCTYPE html PUBLIC 'p' 'abcd'>",
        limits,
        HtmlError::TokenBytesLimit,
    );
    assert_fatal("<?abc 123456>", limits, HtmlError::TokenBytesLimit);
    // A partial declaration becomes payload at EOF. A PI-to-comment conversion
    // adds '?' to its already charged target rather than restarting accounting.
    let small = TokenizerLimits {
        max_token_bytes: 3,
        max_text_chunk_bytes: 1,
        ..limits
    };
    assert_fatal("<!DOCT", small, HtmlError::TokenBytesLimit);
    assert_fatal("<?abc.>", small, HtmlError::TokenBytesLimit);
    assert_fatal("<?xml>", small, HtmlError::TokenBytesLimit);
    assert_fatal("<!--a--!x-->", small, HtmlError::TokenBytesLimit);
    let (tokens, errors) = tokenize(
        &[b"<!DOCTYPE html SYSTEM '' ignored>"],
        TokenizerLimits {
            max_token_bytes: 4,
            max_text_chunk_bytes: 1,
            ..limits
        },
    );
    assert_doctype(&tokens[0], Some("html"), None, Some(""), false);
    assert_eq!(errors.len(), 1);
}

#[test]
fn declaration_tokens_diagnostics_and_raw_input_remain_globally_bounded() {
    let limits = TokenizerLimits::default();
    assert_fatal(
        "<!--a-->",
        TokenizerLimits {
            max_tokens: 1,
            ..limits
        },
        HtmlError::TokenLimit,
    );
    // Diagnostics beyond the retention limit are counted, never fatal (TD-01).
    let mut tokenizer = HtmlTokenizer::new(
        TokenizerLimits {
            max_diagnostics: 0,
            ..limits
        },
        CancellationToken::default(),
    )
    .unwrap();
    assert!(
        tokenizer
            .feed(b"<!--a--!>")
            .and_then(|_| tokenizer.finish())
            .is_ok()
    );
    assert!(tokenizer.diagnostics().is_empty());
    assert_eq!(tokenizer.dropped_diagnostics(), 1);
    assert_fatal(
        "<!DOCTYPE html SYSTEM '' ignored>",
        TokenizerLimits {
            max_input_bytes: 20,
            ..limits
        },
        HtmlError::InputLimit,
    );
    let mut tokenizer = HtmlTokenizer::new(
        TokenizerLimits {
            max_tokens: 1,
            ..limits
        },
        CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(
        tokenizer.feed(b"<!--a-->").unwrap(),
        [Token::Comment("a".to_owned())]
    );
    assert_eq!(tokenizer.feed(b"<!--b-->"), Err(HtmlError::TokenLimit));
    assert_eq!(tokenizer.finish(), Err(HtmlError::Poisoned));
}

#[test]
fn cancellation_terminates_a_partially_recognized_declaration() {
    let cancellation = CancellationToken::default();
    let mut tokenizer =
        HtmlTokenizer::new(TokenizerLimits::default(), cancellation.clone()).unwrap();
    assert!(tokenizer.feed(b"<!DOC").unwrap().is_empty());
    cancellation.cancel();
    assert_eq!(tokenizer.feed(b"TYPE html>"), Err(HtmlError::Cancelled));
    assert_eq!(tokenizer.finish(), Err(HtmlError::Poisoned));
}
