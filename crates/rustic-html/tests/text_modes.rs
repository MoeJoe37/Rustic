use rustic_base::CancellationToken;
use rustic_html::{
    Diagnostic, DiagnosticKind as D, HtmlError, HtmlTokenizer, Token, TokenizerLimits,
    TokenizerMode as M,
};

fn context(source: &str, mode: M, last: Option<&str>) -> (Vec<Token>, Vec<D>) {
    let mut tokenizer = HtmlTokenizer::with_context(
        TokenizerLimits::default(),
        CancellationToken::default(),
        mode,
        last,
    )
    .unwrap();
    let mut tokens = tokenizer.feed(source.as_bytes()).unwrap();
    tokens.extend(tokenizer.finish().unwrap());
    (
        tokens,
        tokenizer.diagnostics().iter().map(|d| d.kind).collect(),
    )
}

fn chars(value: &str) -> Token {
    Token::Characters(value.to_owned())
}
fn end(value: &str) -> Token {
    Token::EndTag(value.to_owned())
}

#[test]
fn rcdata_and_rawtext_recognize_only_the_appropriate_end_tag() {
    for (mode, name, expected) in [
        (M::Rcdata, "title", "x&<b>"),
        (M::Rawtext, "style", "x&amp;<b>"),
    ] {
        let source = format!("x&amp;<b></{}>tail", name.to_ascii_uppercase());
        assert_eq!(
            context(&source, mode, Some(name)),
            (
                vec![chars(expected), end(name), chars("tail"), Token::Eof],
                vec![]
            )
        );
    }
    assert_eq!(
        context("</TiTlEx><not>&amp;</title>", M::Rcdata, Some("title")),
        (
            vec![chars("</TiTlEx><not>&"), end("title"), Token::Eof],
            vec![]
        )
    );
    assert_eq!(
        context("</title>", M::Rcdata, None),
        (vec![chars("</title>"), Token::Eof], vec![])
    );
}

#[test]
fn plaintext_keeps_markup_and_references_literal_through_eof() {
    assert_eq!(
        context("<b>&amp;</plaintext>\0", M::Plaintext, Some("plaintext")),
        (
            vec![chars("<b>&amp;</plaintext>\u{fffd}"), Token::Eof],
            vec![D::UnexpectedNull]
        )
    );
}

#[test]
fn script_rules_are_html_rules_and_do_not_parse_javascript_strings() {
    assert_eq!(
        context("let x='&amp;';</SCRIPT>tail", M::ScriptData, Some("script")),
        (
            vec![
                chars("let x='&amp;';"),
                end("script"),
                chars("tail"),
                Token::Eof
            ],
            vec![]
        )
    );
    assert_eq!(
        context("'before</script>after'", M::ScriptData, Some("script")),
        (
            vec![chars("'before"), end("script"), chars("after'"), Token::Eof],
            vec![]
        )
    );
    assert_eq!(
        context(
            "<!--<script>inside</script>still--></script>tail",
            M::ScriptData,
            Some("script")
        ),
        (
            vec![
                chars("<!--<script>inside</script>still-->"),
                end("script"),
                chars("tail"),
                Token::Eof
            ],
            vec![]
        )
    );
    assert_eq!(
        context(
            "<!--<scriptx>in</script>tail",
            M::ScriptData,
            Some("script")
        ),
        (
            vec![
                chars("<!--<scriptx>in"),
                end("script"),
                chars("tail"),
                Token::Eof
            ],
            vec![]
        )
    );
}

#[test]
fn eof_recovers_pending_end_tag_prefixes_and_script_escapes() {
    for source in ["<", "</", "</tit", "</title"] {
        assert_eq!(
            context(source, M::Rcdata, Some("title")),
            (vec![chars(source), Token::Eof], vec![])
        );
    }
    assert_eq!(
        context("</title ", M::Rcdata, Some("title")),
        (vec![Token::Eof], vec![D::EofInTag])
    );
    for source in ["<!", "<!-", "<!--x-->"] {
        assert_eq!(
            context(source, M::ScriptData, Some("script")),
            (vec![chars(source), Token::Eof], vec![])
        );
    }
    for source in [
        "<!--",
        "<!--<",
        "<!--</scr",
        "<!--<script",
        "<!--<script>",
        "<!--<script></",
    ] {
        assert_eq!(
            context(source, M::ScriptData, Some("script")),
            (
                vec![chars(source), Token::Eof],
                vec![D::EofInScriptHtmlCommentLikeText]
            ),
            "{source}"
        );
    }
    assert_eq!(
        context("<!--\0", M::ScriptData, Some("script")),
        (
            vec![chars("<!--\u{fffd}"), Token::Eof],
            vec![D::UnexpectedNull, D::EofInScriptHtmlCommentLikeText]
        )
    );
}

#[test]
fn matching_end_tags_use_existing_attribute_recovery_and_budgets() {
    assert_eq!(
        context("</STYLE a='1'/>", M::Rawtext, Some("style")),
        (
            vec![end("style"), Token::Eof],
            vec![D::EndTagAttributes, D::EndTagSolidus]
        )
    );
}

fn drain_with_feedback(tokenizer: &mut HtmlTokenizer, output: &mut Vec<Token>) {
    while let Some(token) = tokenizer.next_token().unwrap() {
        let mode = match &token {
            Token::StartTag(tag) => match tag.name() {
                "title" | "textarea" => Some(M::Rcdata),
                "style" | "xmp" => Some(M::Rawtext),
                "script" => Some(M::ScriptData),
                "plaintext" => Some(M::Plaintext),
                _ => None,
            },
            _ => None,
        };
        if let Some(mode) = mode {
            tokenizer.set_text_mode(mode).unwrap();
        }
        output.push(token);
    }
}

fn drive(chunks: &[&[u8]], close_before_read: bool) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut tokenizer = HtmlTokenizer::new(
        TokenizerLimits {
            max_text_chunk_bytes: 5,
            ..TokenizerLimits::default()
        },
        CancellationToken::default(),
    )
    .unwrap();
    let mut output = Vec::new();
    for chunk in chunks {
        tokenizer.write(chunk).unwrap();
        if !close_before_read {
            drain_with_feedback(&mut tokenizer, &mut output);
        }
    }
    tokenizer.end().unwrap();
    drain_with_feedback(&mut tokenizer, &mut output);
    assert_eq!(output.last(), Some(&Token::Eof));
    assert_eq!(tokenizer.next_token().unwrap(), None);
    (output, tokenizer.diagnostics().to_vec())
}

#[test]
fn pull_feedback_applies_before_the_remainder_of_the_same_input_chunk() {
    let mut tokenizer = HtmlTokenizer::default();
    tokenizer.write(b"<title>&amp;<b></title><p>tail").unwrap();
    tokenizer.end().unwrap(); // Mode feedback must also work with already closed input.
    assert!(
        matches!(tokenizer.next_token().unwrap(),Some(Token::StartTag(tag)) if tag.name()=="title")
    );
    tokenizer.set_text_mode(M::Rcdata).unwrap();
    assert_eq!(tokenizer.next_token().unwrap(), Some(chars("&<b>")));
    assert_eq!(tokenizer.next_token().unwrap(), Some(end("title")));
    assert!(
        matches!(tokenizer.next_token().unwrap(),Some(Token::StartTag(tag)) if tag.name()=="p")
    );
    assert_eq!(tokenizer.next_token().unwrap(), Some(chars("tail")));
    assert_eq!(tokenizer.next_token().unwrap(), Some(Token::Eof));
    assert_eq!(tokenizer.next_token().unwrap(), None);
}

#[test]
fn mode_feedback_and_unicode_input_are_invariant_at_every_byte_split() {
    for source in [
        "\u{feff}<title>🙂\r\n&amp;<b></TiTlE><style>&amp;<b></style>",
        "<script><!--<ScRiPt>🙂</script>still--></script><textarea>&notit;</textarea>",
        "<plaintext>🙂<&amp;</plaintext>\0",
        "<title>&amp",
        "<script><!--</scr",
    ] {
        let bytes = source.as_bytes();
        let expected = drive(&[bytes], false);
        for split in 0..=bytes.len() {
            let chunks = [&bytes[..split], b"", &bytes[split..]];
            assert_eq!(drive(&chunks, false), expected, "{source:?}, split {split}");
            assert_eq!(
                drive(&chunks, true),
                expected,
                "{source:?}, queued split {split}"
            );
        }
        assert_eq!(drive(&bytes.chunks(1).collect::<Vec<_>>(), false), expected);
    }
}

#[test]
fn invalid_mode_changes_preserve_state_and_the_next_valid_opportunity() {
    let mut tokenizer = HtmlTokenizer::default();
    assert_eq!(tokenizer.next_token().unwrap(), None);
    assert_eq!(
        tokenizer.set_text_mode(M::Rcdata),
        Err(HtmlError::InvalidModeChange)
    );
    tokenizer.write(b"<title>").unwrap();
    assert!(matches!(
        tokenizer.next_token().unwrap(),
        Some(Token::StartTag(_))
    ));
    assert_eq!(tokenizer.next_token().unwrap(), None); // Asking for input does not advance it.
    tokenizer.write(b"").unwrap();
    tokenizer.set_text_mode(M::Rcdata).unwrap();
    assert_eq!(
        tokenizer.set_text_mode(M::Rawtext),
        Err(HtmlError::InvalidModeChange)
    );
    tokenizer.write(b"&am").unwrap();
    assert_eq!(tokenizer.next_token().unwrap(), None);
    assert_eq!(
        tokenizer.set_text_mode(M::Plaintext),
        Err(HtmlError::InvalidModeChange)
    );
    tokenizer.write(b"p;</title>").unwrap();
    tokenizer.end().unwrap();
    assert_eq!(tokenizer.next_token().unwrap(), Some(chars("&")));
    assert_eq!(
        tokenizer.set_text_mode(M::Data),
        Err(HtmlError::InvalidModeChange)
    );
    assert_eq!(tokenizer.next_token().unwrap(), Some(end("title")));
    assert_eq!(tokenizer.next_token().unwrap(), Some(Token::Eof));
    assert_eq!(
        tokenizer.set_text_mode(M::Data),
        Err(HtmlError::AlreadyFinished)
    );
}

#[test]
fn queued_input_compaction_preserves_order_across_partial_reads_and_appends() {
    let segment = "<x>🙂</x>";
    let mut tokenizer = HtmlTokenizer::default();
    let mut output = Vec::new();
    tokenizer.write(segment.repeat(64).as_bytes()).unwrap();
    for _ in 0..256 {
        tokenizer.write(segment.as_bytes()).unwrap();
        for _ in 0..4 {
            if let Some(token) = tokenizer.next_token().unwrap() {
                output.push(token);
            }
        }
    }
    tokenizer.end().unwrap();
    while let Some(token) = tokenizer.next_token().unwrap() {
        output.push(token);
    }
    assert_eq!(output.len(), 320 * 3 + 1);
    for tokens in output[..output.len() - 1].chunks_exact(3) {
        assert!(matches!(&tokens[0],Token::StartTag(tag) if tag.name()=="x"));
        assert_eq!(tokens[1], chars("🙂"));
        assert_eq!(tokens[2], end("x"));
    }
    assert_eq!(output.last(), Some(&Token::Eof));
}

fn character_data(tokens: &[Token]) -> String {
    tokens
        .iter()
        .filter_map(|token| match token {
            Token::Characters(value) => Some(value.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn failed_end_tag_and_script_word_probes_do_not_grow_with_literal_name_runs() {
    let limits = TokenizerLimits {
        max_token_bytes: 6,
        max_text_chunk_bytes: 6,
        ..TokenizerLimits::default()
    };
    let source = format!("</{}>", "a".repeat(10_000));
    let mut tokenizer = HtmlTokenizer::with_context(
        limits,
        CancellationToken::default(),
        M::Rawtext,
        Some("style"),
    )
    .unwrap();
    let mut tokens = tokenizer.feed(source.as_bytes()).unwrap();
    tokens.extend(tokenizer.finish().unwrap());
    assert_eq!(character_data(&tokens), source);
    assert!(tokenizer.diagnostics().is_empty());
    let literal = format!("<!--<{}>", "s".repeat(10_000));
    let mut tokenizer = HtmlTokenizer::with_context(
        limits,
        CancellationToken::default(),
        M::ScriptData,
        Some("script"),
    )
    .unwrap();
    let mut tokens = tokenizer
        .feed(format!("{literal}</script>").as_bytes())
        .unwrap();
    tokens.extend(tokenizer.finish().unwrap());
    assert_eq!(character_data(&tokens), literal);
    assert_eq!(tokens[tokens.len() - 2], end("script"));
    assert!(tokenizer.diagnostics().is_empty());
}

#[test]
fn explicit_context_is_bounded_and_mode_selection_is_not_guessed_from_tag_names() {
    let limits = TokenizerLimits {
        max_token_bytes: 3,
        max_text_chunk_bytes: 3,
        ..TokenizerLimits::default()
    };
    assert!(matches!(
        HtmlTokenizer::with_context(
            limits,
            CancellationToken::default(),
            M::Rcdata,
            Some("title")
        ),
        Err(HtmlError::TokenBytesLimit)
    ));
    assert_eq!(
        context("x</TITLE>", M::Rcdata, Some("TiTle")),
        (vec![chars("x"), end("title"), Token::Eof], vec![])
    );
    let mut tokenizer = HtmlTokenizer::default();
    let tokens = tokenizer.feed(b"<title><b>").unwrap();
    assert!(matches!(&tokens[1],Token::StartTag(tag) if tag.name()=="b"));
    let mut tokenizer = HtmlTokenizer::default();
    assert!(matches!(
        tokenizer.feed(b"<title>x").unwrap().as_slice(),
        [Token::StartTag(_)]
    ));
    assert_eq!(
        tokenizer.set_text_mode(M::Rcdata),
        Err(HtmlError::InvalidModeChange)
    );
}

#[test]
fn closing_input_is_separate_from_consuming_eof_and_never_emits_it_twice() {
    let mut tokenizer = HtmlTokenizer::default();
    tokenizer.write(b"x").unwrap();
    tokenizer.end().unwrap();
    assert_eq!(tokenizer.end(), Err(HtmlError::AlreadyFinished));
    assert_eq!(tokenizer.write(b"y"), Err(HtmlError::AlreadyFinished));
    assert_eq!(tokenizer.next_token().unwrap(), Some(chars("x")));
    assert_eq!(tokenizer.next_token().unwrap(), Some(Token::Eof));
    assert_eq!(tokenizer.next_token().unwrap(), None);
    assert_eq!(tokenizer.next_token().unwrap(), None);
    assert_eq!(tokenizer.finish(), Err(HtmlError::AlreadyFinished));
}

#[test]
fn text_driver_limits_and_cancellation_poison_future_reads_and_writes() {
    let limits = TokenizerLimits {
        max_text_chunk_bytes: 2,
        ..TokenizerLimits::default()
    };
    let mut tokenizer =
        HtmlTokenizer::with_context(limits, CancellationToken::default(), M::Plaintext, None)
            .unwrap();
    tokenizer.write(b"\0").unwrap();
    assert_eq!(tokenizer.next_token(), Err(HtmlError::TokenBytesLimit));
    assert_eq!(tokenizer.write(b"x"), Err(HtmlError::Poisoned));
    assert_eq!(tokenizer.next_token(), Err(HtmlError::Poisoned));
    let mut tokenizer = HtmlTokenizer::new(
        TokenizerLimits {
            max_input_bytes: 3,
            ..TokenizerLimits::default()
        },
        CancellationToken::default(),
    )
    .unwrap();
    tokenizer.write(b"abc").unwrap();
    assert_eq!(tokenizer.write(b"d"), Err(HtmlError::InputLimit));
    assert_eq!(tokenizer.next_token(), Err(HtmlError::Poisoned));
    let mut tokenizer = HtmlTokenizer::with_context(
        TokenizerLimits {
            max_diagnostics: 0,
            ..TokenizerLimits::default()
        },
        CancellationToken::default(),
        M::ScriptData,
        Some("script"),
    )
    .unwrap();
    tokenizer.write(b"<!--").unwrap();
    tokenizer.end().unwrap();
    assert_eq!(tokenizer.next_token(), Err(HtmlError::DiagnosticLimit));
    let cancellation = CancellationToken::default();
    let mut tokenizer = HtmlTokenizer::default();
    assert_eq!(tokenizer.finish().unwrap(), [Token::Eof]);
    let mut tokenizer =
        HtmlTokenizer::new(TokenizerLimits::default(), cancellation.clone()).unwrap();
    tokenizer.write(b"<title>text").unwrap();
    assert!(matches!(
        tokenizer.next_token().unwrap(),
        Some(Token::StartTag(_))
    ));
    cancellation.cancel();
    assert_eq!(tokenizer.next_token(), Err(HtmlError::Cancelled));
    assert_eq!(tokenizer.end(), Err(HtmlError::Poisoned));
}
