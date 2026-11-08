//! Pinned CSS syntax corpus: SimonSapin/css-parsing-tests (CC0-1.0).
//!
//! Each vendored file holds input/expected pairs for one CSS Syntax entry
//! point. Expected values are compared structurally after documented
//! adaptations where the corpus predates or differs from the current
//! specification; see `adapt`. Records that still differ for a stated
//! specification reason are listed in `KNOWN_DIFFERENCES`.

use rustic_css::{
    BlockItem, ComponentValue, CssParser, Declaration, DiagnosticKind, Numeric, Rule, Token, Value,
};

macro_rules! corpus {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("fixtures/css-parsing-tests/", $name)))),*]
    };
}

/// Revision 203ce36bffd617db7f118c551e32794561fb273d; see the manifest.
const FILES: &[(&str, &str)] = corpus![
    "component_value_list.json",
    "one_component_value.json",
    "declaration_list.json",
    "blocks_contents.json",
    "one_declaration.json",
    "one_rule.json",
    "rule_list.json",
    "stylesheet.json",
];

/// `(file, zero-based case index, reason)`.
const KNOWN_DIFFERENCES: &[(&str, usize, &str)] = &[(
    "declaration_list.json",
    8,
    "the corpus expects CSS Syntax 2021 'list of declarations' recovery; \
     current CSS Syntax with CSS Nesting parses `@ media screen {..}` as a nested \
     qualified rule, as blocks_contents.json case 8 expects",
)];

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
}

impl PartialEq for Json {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            // Numeric values compare by value: `-0` equals `0`.
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            _ => false,
        }
    }
}

/// A minimal JSON reader for the vendored files (no objects needed).
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn skip(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Json {
        self.skip();
        match self.bytes[self.pos] {
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip();
                    if self.bytes[self.pos] == b']' {
                        self.pos += 1;
                        return Json::Array(items);
                    }
                    items.push(self.value());
                    self.skip();
                    if self.bytes[self.pos] == b',' {
                        self.pos += 1;
                    }
                }
            }
            b'"' => Json::String(self.string()),
            b'n' => self.word("null", Json::Null),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            _ => {
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && matches!(
                        self.bytes[self.pos],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.pos += 1;
                }
                let text = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
                Json::Number(text.parse().unwrap_or_else(|_| panic!("bad number {text}")))
            }
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Json {
        assert!(self.bytes[self.pos..].starts_with(word.as_bytes()));
        self.pos += word.len();
        value
    }

    fn hex4(&mut self) -> u32 {
        let text = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4]).unwrap();
        self.pos += 4;
        u32::from_str_radix(text, 16).unwrap()
    }

    fn string(&mut self) -> String {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let rest = std::str::from_utf8(&self.bytes[self.pos..]).unwrap();
            let c = rest.chars().next().unwrap();
            self.pos += c.len_utf8();
            match c {
                '"' => return out,
                '\\' => {
                    let e = self.bytes[self.pos];
                    self.pos += 1;
                    match e {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'f' => out.push('\u{c}'),
                        b'b' => out.push('\u{8}'),
                        b'u' => {
                            let mut unit = self.hex4();
                            if (0xd800..0xdc00).contains(&unit)
                                && self.bytes[self.pos..].starts_with(b"\\u")
                            {
                                self.pos += 2;
                                let low = self.hex4();
                                unit = 0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
                            }
                            out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                        }
                        other => out.push(other as char),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

fn parse_json(source: &str) -> Vec<Json> {
    let mut reader = Reader {
        bytes: source.as_bytes(),
        pos: 0,
    };
    match reader.value() {
        Json::Array(items) => items,
        _ => panic!("corpus file is not an array"),
    }
}

fn s(value: &str) -> Json {
    Json::String(value.into())
}

fn arr(items: impl IntoIterator<Item = Json>) -> Json {
    Json::Array(items.into_iter().collect())
}

fn numeric(kind: &str, n: &Numeric, unit: Option<&str>) -> Json {
    let kind_flag = if n.integer { "integer" } else { "number" };
    let mut items = vec![s(kind), s(&n.repr), Json::Number(n.value), s(kind_flag)];
    items.extend(unit.map(s));
    arr(items)
}

fn token(token: &Token) -> Json {
    match token {
        Token::Ident(v) => arr([s("ident"), s(v)]),
        Token::AtKeyword(v) => arr([s("at-keyword"), s(v)]),
        Token::Hash { value, id } => arr([
            s("hash"),
            s(value),
            s(if *id { "id" } else { "unrestricted" }),
        ]),
        Token::String(v) => arr([s("string"), s(v)]),
        Token::BadString => arr([s("error"), s("bad-string")]),
        Token::Url(v) => arr([s("url"), s(v)]),
        Token::BadUrl => arr([s("error"), s("bad-url")]),
        Token::Delim(c) => s(&c.to_string()),
        Token::Number(n) => numeric("number", n, None),
        Token::Percentage(n) => numeric("percentage", n, None),
        Token::Dimension(n, unit) => numeric("dimension", n, Some(unit)),
        Token::Whitespace => s(" "),
        Token::Cdo => s("<!--"),
        Token::Cdc => s("-->"),
        Token::Colon => s(":"),
        Token::Semicolon => s(";"),
        Token::Comma => s(","),
        Token::CloseParen => arr([s("error"), s(")")]),
        Token::CloseSquare => arr([s("error"), s("]")]),
        Token::CloseCurly => arr([s("error"), s("}")]),
        Token::Function(_) | Token::OpenParen | Token::OpenSquare | Token::OpenCurly => {
            unreachable!("grouped into component values")
        }
    }
}

fn value(v: &Value) -> Json {
    match &v.value {
        ComponentValue::Token(t) => token(t),
        ComponentValue::Function { name, arguments } => arr([s("function"), s(name)]
            .into_iter()
            .chain(arguments.iter().map(value))),
        ComponentValue::Block { kind, contents } => {
            let label = match kind {
                rustic_css::BlockKind::Curly => "{}",
                rustic_css::BlockKind::Square => "[]",
                rustic_css::BlockKind::Paren => "()",
            };
            arr(std::iter::once(s(label)).chain(contents.iter().map(value)))
        }
    }
}

fn values(list: &[Value]) -> Json {
    arr(list.iter().map(value))
}

fn rule(r: &Rule) -> Json {
    match r {
        Rule::Qualified(q) => arr([s("qualified rule"), values(&q.prelude), values(&q.block)]),
        Rule::At(a) => arr([
            s("at-rule"),
            s(&a.name),
            values(&a.prelude),
            a.block.as_deref().map_or(Json::Null, values),
        ]),
    }
}

fn declaration(d: &Declaration) -> Json {
    arr([
        s("declaration"),
        s(&d.name),
        values(&d.value),
        Json::Bool(d.important),
    ])
}

fn item(i: &BlockItem) -> Json {
    match i {
        BlockItem::Declaration(d) => declaration(d),
        BlockItem::Rule(r) => rule(r),
    }
}

fn error(kinds: &[DiagnosticKind]) -> Json {
    let label = if kinds.contains(&DiagnosticKind::EmptyInput) {
        "empty"
    } else if kinds.contains(&DiagnosticKind::ExtraInput) {
        "extra-input"
    } else {
        "invalid"
    };
    arr([s("error"), s(label)])
}

fn actual(file: &str, input: &str) -> Json {
    let parser = CssParser::default();
    match file {
        "component_value_list.json" => values(&parser.component_values(input).unwrap().value),
        "one_component_value.json" => {
            let p = parser.component_value(input).unwrap();
            let kinds: Vec<_> = p.diagnostics.iter().map(|d| d.kind).collect();
            p.value.as_ref().map_or_else(|| error(&kinds), value)
        }
        "declaration_list.json" | "blocks_contents.json" => {
            arr(parser.block_contents(input).unwrap().value.iter().map(item))
        }
        "one_declaration.json" => {
            let p = parser.declaration(input).unwrap();
            let kinds: Vec<_> = p.diagnostics.iter().map(|d| d.kind).collect();
            p.value.as_ref().map_or_else(|| error(&kinds), declaration)
        }
        "one_rule.json" => {
            let p = parser.rule(input).unwrap();
            let kinds: Vec<_> = p.diagnostics.iter().map(|d| d.kind).collect();
            p.value.as_ref().map_or_else(|| error(&kinds), rule)
        }
        "rule_list.json" => arr(parser.rule_list(input).unwrap().value.iter().map(rule)),
        "stylesheet.json" => arr(parser
            .stylesheet(input)
            .unwrap()
            .value
            .rules
            .iter()
            .map(rule)),
        other => panic!("unknown corpus file {other}"),
    }
}

fn is_error(json: &Json, kinds: &[&str]) -> bool {
    matches!(json, Json::Array(items) if items.len() == 2
        && items[0] == s("error")
        && kinds.iter().any(|k| items[1] == s(k)))
}

/// Adapt the corpus to the current specification and this AST:
///
/// - EOF parse errors in strings/URLs and "invalid" recovery markers inside
///   lists are diagnostics here, not list items, so they are removed;
/// - CSS Syntax 3 has no match/column tokens: `~=` etc. are two delims;
/// - declaration values have leading/trailing whitespace removed, as
///   "consume a declaration" specifies (the corpus keeps it).
fn adapt(json: Json) -> Json {
    let Json::Array(items) = json else {
        return json;
    };
    if items.len() == 4 && items[0] == s("declaration") {
        let mut items = items;
        if let Json::Array(value) = &mut items[2] {
            while value.first() == Some(&s(" ")) {
                value.remove(0);
            }
            while value.last() == Some(&s(" ")) {
                value.pop();
            }
        }
        items[2] = adapt(items[2].clone());
        return Json::Array(items);
    }
    let mut out = Vec::new();
    for item in items {
        if is_error(&item, &["eof-in-string", "eof-in-url", "invalid"]) {
            continue;
        }
        match &item {
            Json::String(text)
                if matches!(text.as_str(), "~=" | "|=" | "^=" | "$=" | "*=" | "||") =>
            {
                out.extend(text.chars().map(|c| s(&c.to_string())));
            }
            _ => out.push(adapt(item)),
        }
    }
    Json::Array(out)
}

fn contains_unicode_range(json: &Json) -> bool {
    match json {
        Json::Array(items) => {
            items.first() == Some(&s("unicode-range")) || items.iter().any(contains_unicode_range)
        }
        _ => false,
    }
}

#[test]
fn pinned_css_parsing_corpus_matches_after_documented_adaptations() {
    let (mut passed, mut excluded, mut failures) = (0, 0, Vec::new());
    for (file, source) in FILES {
        let pairs = parse_json(source);
        for (index, pair) in pairs.chunks(2).enumerate() {
            let Json::String(input) = &pair[0] else {
                panic!("{file} #{index}: input is not a string")
            };
            // Unicode-range tokens were removed from CSS Syntax; grammars that
            // need ranges reparse ordinary tokens, so these records are excluded.
            if contains_unicode_range(&pair[1]) {
                excluded += 1;
                continue;
            }
            let whole_error = is_error(&pair[1], &["empty", "invalid", "extra-input"]);
            let expected = if whole_error {
                pair[1].clone()
            } else {
                adapt(pair[1].clone())
            };
            let got = actual(file, input);
            let known = KNOWN_DIFFERENCES
                .iter()
                .any(|(f, i, _)| f == file && *i == index);
            match (got == expected, known) {
                (true, false) => passed += 1,
                (false, true) => {}
                (true, true) => failures.push(format!("{file} #{index}: unexpectedly passes")),
                (false, false) => failures.push(format!(
                    "{file} #{index}: {input:?}\n  expected {expected:?}\n  actual   {got:?}"
                )),
            }
        }
    }
    eprintln!(
        "css-parsing-tests: {passed} passed, {} known differences, {excluded} excluded",
        KNOWN_DIFFERENCES.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures\n{}",
        failures.len(),
        failures.join("\n")
    );
}
