//! Focused CSS syntax behavior: spans, recovery, limits and cancellation.

use rustic_base::CancellationToken;
use rustic_css::{
    BlockItem, ComponentValue, CssError, CssLimits, CssParser, DiagnosticKind as D, Rule, Token,
    parse_block_contents,
};

fn tokens(source: &str) -> Vec<Token> {
    CssParser::default()
        .tokenize(source)
        .unwrap()
        .value
        .into_iter()
        .map(|t| t.token)
        .collect()
}

fn kinds(source: &str) -> Vec<D> {
    CssParser::default()
        .tokenize(source)
        .unwrap()
        .diagnostics
        .iter()
        .map(|d| d.kind)
        .collect()
}

#[test]
fn spans_are_original_byte_offsets_after_preprocessing() {
    let source = "a\r\n/* c */é\u{0}b";
    let parsed = CssParser::default().tokenize(source).unwrap();
    let spans: Vec<_> = parsed.value.iter().map(|t| t.span.range()).collect();
    // `a`, CRLF whitespace, then the ident after the comment (NUL reads as U+FFFD).
    assert_eq!(spans, [0..1, 1..3, 10..14]);
    assert_eq!(parsed.value[2].token, Token::Ident("é\u{fffd}b".into()));
    assert_eq!(parsed.value[2].span.slice_str(source), Some("é\u{0}b"));
}

#[test]
fn escapes_follow_the_standard_replacements() {
    // One whitespace after a hex escape belongs to the escape.
    assert_eq!(
        tokens(r"\0 \D800 \110000 \41 x \"),
        [
            Token::Ident("\u{fffd}\u{fffd}\u{fffd}Ax".into()),
            Token::Whitespace,
            Token::Ident("\u{fffd}".into()),
        ]
    );
    assert_eq!(kinds(r"a \"), [D::EofInEscape]);
    assert_eq!(tokens("\\\n"), [Token::Delim('\\'), Token::Whitespace]);
    assert_eq!(kinds("\\\n"), [D::InvalidEscape]);
}

#[test]
fn bad_strings_urls_and_unclosed_constructs_recover_with_diagnostics() {
    assert_eq!(
        tokens("'a\nb"),
        [
            Token::BadString,
            Token::Whitespace,
            Token::Ident("b".into())
        ]
    );
    assert_eq!(kinds("'a\nb"), [D::NewlineInString]);
    assert_eq!(
        tokens("url(a b) c"),
        [Token::BadUrl, Token::Whitespace, Token::Ident("c".into())]
    );
    assert_eq!(kinds("url( a\\)b "), [D::EofInUrl]);
    assert_eq!(tokens("url( a\\)b "), [Token::Url("a)b".into())]);
    assert_eq!(tokens("url( 'x' )")[0], Token::Function("url".into()));
    assert_eq!(kinds("/* open"), [D::EofInComment]);
    assert_eq!(kinds("'open"), [D::EofInString]);
    let parsed = CssParser::default().stylesheet("a { b: c").unwrap();
    assert_eq!(parsed.value.rules.len(), 1);
    assert_eq!(parsed.diagnostics[0].kind, D::EofInBlock);
    assert_eq!(parsed.diagnostics[0].span.range(), 2..8);
}

#[test]
fn numbers_keep_representation_type_and_finite_values() {
    let Token::Dimension(n, unit) = &tokens("+1.5e3px")[0] else {
        panic!()
    };
    assert_eq!(
        (n.repr.as_str(), n.value, n.integer, unit.as_str()),
        ("+1.5e3", 1500.0, false, "px")
    );
    let Token::Number(n) = &tokens("1e999")[0] else {
        panic!()
    };
    assert_eq!(n.value, f64::MAX);
    let Token::Percentage(n) = &tokens("-7%")[0] else {
        panic!()
    };
    assert!(n.integer && n.value == -7.0);
    assert_eq!(
        tokens("1e"),
        [Token::Dimension(
            rustic_css::Numeric {
                value: 1.0,
                integer: true,
                repr: "1".into()
            },
            "e".into()
        )]
    );
}

#[test]
fn style_rules_keep_raw_blocks_and_parse_nested_contents_later() {
    let source = "@media screen { a { color: red !IMPORTANT; &:hover { x: y } } }";
    let sheet = CssParser::default().stylesheet(source).unwrap();
    let Rule::At(media) = &sheet.value.rules[0] else {
        panic!()
    };
    assert_eq!(media.name, "media");
    assert_eq!(media.span.range(), 0..source.len());
    let inner = parse_block_contents(media.block.as_deref().unwrap(), 16);
    let [BlockItem::Rule(Rule::Qualified(style))] = inner.value.as_slice() else {
        panic!()
    };
    let contents = parse_block_contents(&style.block, 16).value;
    let BlockItem::Declaration(color) = &contents[0] else {
        panic!()
    };
    assert_eq!(
        (color.name.as_str(), color.important, color.value.len()),
        ("color", true, 1)
    );
    assert_eq!(color.span.slice_str(source), Some("color: red !IMPORTANT"));
    assert!(matches!(&contents[1], BlockItem::Rule(Rule::Qualified(_))));
    // A custom property keeps a `{}` block as its value.
    let custom = CssParser::default()
        .declaration("--x: a {b}")
        .unwrap()
        .value
        .unwrap();
    assert!(matches!(
        custom.value[2].value,
        ComponentValue::Block { .. }
    ));
}

#[test]
fn limits_are_exact_and_fatal() {
    let parser = |limits| CssParser::new(limits, CancellationToken::default()).unwrap();
    let limits = CssLimits {
        max_input_bytes: 3,
        ..CssLimits::default()
    };
    assert!(parser(limits).stylesheet("a{}").is_ok());
    assert_eq!(
        parser(limits).stylesheet("a{} ").map(drop),
        Err(CssError::InputTooLarge)
    );
    let limits = CssLimits {
        max_tokens: 3,
        ..CssLimits::default()
    };
    assert!(parser(limits).component_values("a b").is_ok());
    assert_eq!(
        parser(limits).component_values("a b ").map(drop),
        Err(CssError::TokenLimit)
    );
    let limits = CssLimits {
        max_nesting: 2,
        ..CssLimits::default()
    };
    assert!(parser(limits).component_values("([])").is_ok());
    assert_eq!(
        parser(limits).component_values("([{").map(drop),
        Err(CssError::NestingLimit)
    );
    let limits = CssLimits {
        max_diagnostics: 1,
        ..CssLimits::default()
    };
    let parsed = parser(limits).component_values("'a\n'b\n'c\n").unwrap();
    assert_eq!(
        (parsed.diagnostics.len(), parsed.dropped_diagnostics),
        (1, 2)
    );
    assert_eq!(
        CssParser::new(
            CssLimits {
                max_tokens: 0,
                ..CssLimits::default()
            },
            Default::default()
        )
        .map(drop),
        Err(CssError::InvalidLimits)
    );
    // Deep but bounded nesting builds and drops without recursion problems.
    let deep = "(".repeat(256);
    assert!(CssParser::default().component_values(&deep).is_ok());
    assert_eq!(
        CssParser::default()
            .component_values(&"(".repeat(257))
            .map(drop),
        Err(CssError::NestingLimit)
    );
}

#[test]
fn cancellation_stops_parsing() {
    let cancellation = CancellationToken::default();
    let parser = CssParser::new(CssLimits::default(), cancellation.clone()).unwrap();
    cancellation.cancel();
    assert_eq!(parser.stylesheet("a{}").map(drop), Err(CssError::Cancelled));
}

#[test]
fn single_entry_points_report_empty_extra_and_invalid() {
    let p = CssParser::default();
    let kinds = |d: &[rustic_css::Diagnostic]| d.iter().map(|d| d.kind).collect::<Vec<_>>();
    assert_eq!(kinds(&p.rule("  ").unwrap().diagnostics), [D::EmptyInput]);
    assert_eq!(
        kinds(&p.rule("a{} b{}").unwrap().diagnostics),
        [D::ExtraInput]
    );
    assert_eq!(kinds(&p.rule("a").unwrap().diagnostics), [D::EofInRule]);
    assert_eq!(
        kinds(&p.declaration("a b").unwrap().diagnostics),
        [D::InvalidDeclaration]
    );
    assert_eq!(
        kinds(&p.component_value("a b").unwrap().diagnostics),
        [D::ExtraInput]
    );
    assert!(p.rule_list("<!-- a{}").unwrap().value.len() == 1);
    assert_eq!(p.stylesheet("<!-- a{} -->").unwrap().value.rules.len(), 1);
}
