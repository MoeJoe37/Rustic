//! JS-01 lexical grammar through the public [`Lexer`]: goals, literals,
//! comments and line terminators.

use rustic_js::{Goal, Lexer, Punct, Source, TokenKind};

fn units(text: &str) -> Vec<u16> {
    Source::from_str_limited(text, usize::MAX)
        .unwrap()
        .units()
        .to_vec()
}

fn tokens(text: &str, goal: Goal) -> Vec<TokenKind> {
    let units = units(text);
    let mut lexer = Lexer::new(&units, true);
    let mut out = Vec::new();
    loop {
        let token = lexer.next_token(goal).expect("test input lexes");
        if token.kind == TokenKind::Eof {
            return out;
        }
        out.push(token.kind);
    }
}

fn one(text: &str) -> TokenKind {
    let mut all = tokens(text, Goal::RegExp);
    assert_eq!(all.len(), 1, "`{text}` gave {all:?}");
    all.remove(0)
}

fn number(text: &str) -> f64 {
    match one(text) {
        TokenKind::Number { value, .. } => value,
        other => panic!("`{text}` gave {other:?}"),
    }
}

fn string(text: &str) -> Vec<u16> {
    match one(text) {
        TokenKind::String { value, .. } => value,
        other => panic!("`{text}` gave {other:?}"),
    }
}

#[test]
fn the_goal_decides_between_division_and_regular_expressions() {
    assert_eq!(tokens("/a/g", Goal::Div)[0], TokenKind::Punct(Punct::Slash));
    assert_eq!(
        one("/a[/]b/gu"),
        TokenKind::RegExp {
            body: units("a[/]b"),
            flags: "gu".into(),
        }
    );
}

#[test]
fn numeric_literals_cover_every_radix_separators_and_bigint() {
    assert_eq!(number("0x1F"), 31.0);
    assert_eq!(number("0o17"), 15.0);
    assert_eq!(number("0b101"), 5.0);
    assert_eq!(number("1_000_000"), 1e6);
    assert_eq!(number(".5e-3"), 0.0005);
    assert_eq!(
        one("017"),
        TokenKind::Number {
            value: 15.0,
            legacy: true
        }
    );
    assert_eq!(number("09.5"), 9.5);
    assert_eq!(
        one("0xFFn"),
        TokenKind::BigInt {
            radix: 16,
            digits: "ff".into()
        }
    );
    // Correct rounding of a long decimal literal.
    assert_eq!(number("0.1000000000000000055511151231257827"), 0.1);
    let units = units("1__0");
    assert!(Lexer::new(&units, true).next_token(Goal::RegExp).is_err());
}

#[test]
fn string_escapes_decode_to_utf16() {
    assert_eq!(string(r"'\x41\u0042\u{1F600}'"), units("AB😀"));
    assert_eq!(string(r"'\uD800'"), vec![0xD800]);
    assert_eq!(string("'a\\\nb'"), units("ab"));
    match one(r"'\101'") {
        TokenKind::String {
            value,
            legacy_escape,
        } => {
            assert_eq!(value, units("A"));
            assert!(legacy_escape);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn templates_keep_cooked_and_raw_values() {
    let TokenKind::Template(part) = one("`a\\x41\r\nb`") else {
        panic!("not a template");
    };
    assert_eq!(part.cooked, Some(units("aA\nb")));
    assert_eq!(part.raw, units("a\\x41\nb"));
    assert!(part.tail);
    // An invalid escape has no cooked value (allowed when tagged).
    let TokenKind::Template(part) = one("`\\unicode`") else {
        panic!("not a template");
    };
    assert_eq!(part.cooked, None);
}

#[test]
fn comments_and_line_terminators_set_newline_before() {
    let units = units("a /* x\u{2028} */ b // c\nd <!-- e\n--> f\ng");
    let mut lexer = Lexer::new(&units, true);
    let mut seen = Vec::new();
    loop {
        let token = lexer.next_token(Goal::Div).unwrap();
        if token.kind == TokenKind::Eof {
            break;
        }
        seen.push((token.kind.clone(), token.newline_before));
    }
    let names: Vec<(String, bool)> = seen
        .into_iter()
        .map(|(kind, newline)| match kind {
            TokenKind::Name { name, .. } => (name.into(), newline),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        [
            ("a".into(), false),
            ("b".into(), true),
            ("d".into(), true),
            ("g".into(), true),
        ]
    );
    // Modules have no HTML-like comments.
    let units = units_of("a <!-- b");
    let mut lexer = Lexer::new(&units, false);
    lexer.next_token(Goal::Div).unwrap();
    assert_eq!(
        lexer.next_token(Goal::Div).unwrap().kind,
        TokenKind::Punct(Punct::Lt)
    );
}

fn units_of(text: &str) -> Vec<u16> {
    units(text)
}

#[test]
fn identifiers_decode_escapes_and_unicode() {
    assert_eq!(
        one(r"\u0061bc"),
        TokenKind::Name {
            name: "abc".into(),
            escaped: true
        }
    );
    assert_eq!(
        one("℮゛"),
        TokenKind::Name {
            name: "℮゛".into(),
            escaped: false
        }
    );
    assert_eq!(one("#priv"), TokenKind::PrivateName("priv".into()));
    let units = units("#!shebang\nx");
    assert_eq!(
        Lexer::new(&units, true)
            .next_token(Goal::RegExp)
            .unwrap()
            .kind,
        TokenKind::Name {
            name: "x".into(),
            escaped: false
        }
    );
}
