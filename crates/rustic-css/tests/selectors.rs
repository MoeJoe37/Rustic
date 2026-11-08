//! Selector syntax, specificity and serialization (CSS-02).

use rustic_css::{
    CssParser, NamespaceMap, PseudoElement, SelectorErrorKind, SelectorList, Specificity,
};

fn parse_with(source: &str, namespaces: &NamespaceMap) -> Result<SelectorList, SelectorErrorKind> {
    CssParser::default()
        .selector_list(source, namespaces)
        .expect("within limits")
        .value
        .map_err(|e| e.kind)
}

fn parse(source: &str) -> Result<SelectorList, SelectorErrorKind> {
    parse_with(source, &NamespaceMap::new())
}

fn spec(ids: u32, classes: u32, types: u32) -> Specificity {
    Specificity {
        ids,
        classes,
        types,
    }
}

/// `(input, serialization, specificity of the first selector)`.
const VALID: &[(&str, &str, (u32, u32, u32))] = &[
    ("*", "*", (0, 0, 0)),
    ("DIV", "DIV", (0, 0, 1)),
    ("  a  ,  b  ", "a, b", (0, 0, 1)),
    ("#x", "#x", (1, 0, 0)),
    (".a.b", ".a.b", (0, 2, 0)),
    ("*.a", ".a", (0, 1, 0)),
    ("a#b.c[d]:hover", "a#b.c[d]:hover", (1, 3, 1)),
    ("ul li", "ul li", (0, 0, 2)),
    ("ul>li", "ul > li", (0, 0, 2)),
    ("h1+p", "h1 + p", (0, 0, 2)),
    ("h1 ~ p", "h1 ~ p", (0, 0, 2)),
    ("a\t\n> b", "a > b", (0, 0, 2)),
    ("[a=b]", "[a=\"b\"]", (0, 1, 0)),
    ("[ a = 'b c' i ]", "[a=\"b c\" i]", (0, 1, 0)),
    (
        "[a~=b][a|=b][a^=b][a$=b][a*=b]",
        "[a~=\"b\"][a|=\"b\"][a^=\"b\"][a$=\"b\"][a*=\"b\"]",
        (0, 5, 0),
    ),
    ("[a=b S]", "[a=\"b\" s]", (0, 1, 0)),
    ("[*|a]", "[*|a]", (0, 1, 0)),
    ("[|a]", "[a]", (0, 1, 0)),
    ("*|*", "*", (0, 0, 0)),
    ("|p", "|p", (0, 0, 1)),
    (":ROOT", ":root", (0, 1, 0)),
    (
        ":first-child:last-of-type",
        ":first-child:last-of-type",
        (0, 2, 0),
    ),
    (":nth-child(2n+1)", ":nth-child(2n+1)", (0, 1, 0)),
    (":nth-child(odd)", ":nth-child(2n+1)", (0, 1, 0)),
    (":nth-child(  -n + 3  )", ":nth-child(-n+3)", (0, 1, 0)),
    (":nth-last-of-type(0n+5)", ":nth-last-of-type(5)", (0, 1, 0)),
    (
        ":nth-child(2n of .a, #b)",
        ":nth-child(2n of .a, #b)",
        (1, 1, 0),
    ),
    (":is(#a, .b, c)", ":is(#a, .b, c)", (1, 0, 0)),
    (":is(#a, ::before, !!)", ":is(#a)", (1, 0, 0)),
    (":is()", ":is()", (0, 0, 0)),
    (":where(#a, .b)", ":where(#a, .b)", (0, 0, 0)),
    ("a:not(.b, #c)", "a:not(.b, #c)", (1, 0, 1)),
    (
        ":not(:not(:is(a > b)))",
        ":not(:not(:is(a > b)))",
        (0, 0, 2),
    ),
    ("::before", "::before", (0, 0, 1)),
    ("p:first-line", "p::first-line", (0, 0, 2)),
    ("li::MARKER", "li::marker", (0, 0, 2)),
    ("a ::after", "a ::after", (0, 0, 2)),
    (
        ":link:visited:any-link",
        ":link:visited:any-link",
        (0, 3, 0),
    ),
    (".\\31 23", ".\\31 23", (0, 1, 0)),
    ("#-\\33x", "#-\\33 x", (1, 0, 0)),
    ("[a=\"q\\\"\"]", "[a=\"q\\\"\"]", (0, 1, 0)),
];

#[test]
fn valid_selectors_serialize_and_have_spec_specificity() {
    let mut failures = Vec::new();
    for &(input, serialized, (a, b, c)) in VALID {
        match parse(input) {
            Ok(list) => {
                let text = list.to_string();
                let s = list.selectors()[0].specificity();
                if text != serialized || s != spec(a, b, c) {
                    failures.push(format!("{input:?}: got {text:?} {s:?}"));
                }
                // Serialization is a fixed point.
                let again = parse(&text).map(|l| l.to_string());
                if again.as_deref() != Ok(text.as_str()) {
                    failures.push(format!("{input:?}: reparse gave {again:?}"));
                }
            }
            Err(e) => failures.push(format!("{input:?}: unexpected {e:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn invalid_selectors_report_a_reason() {
    use SelectorErrorKind::*;
    let cases: &[(&str, SelectorErrorKind)] = &[
        ("", Empty),
        ("a,", Empty),
        (",a", Empty),
        ("a >", DanglingCombinator),
        ("> a", UnexpectedToken),
        ("a > > b", UnexpectedToken),
        ("#1x", UnexpectedToken),
        (". a", UnexpectedToken),
        ("a!", UnexpectedToken),
        ("[a=1]", InvalidAttribute),
        ("[a=b c]", InvalidAttribute),
        ("[a=b i j]", InvalidAttribute),
        ("[]", InvalidAttribute),
        ("[a==b]", InvalidAttribute),
        ("ns|a", UnknownNamespacePrefix),
        ("[ns|a]", UnknownNamespacePrefix),
        (":user-invalid", UnsupportedPseudoClass),
        (":hovered", UnsupportedPseudoClass),
        ("::cue", UnsupportedPseudoElement),
        (":marker", UnsupportedPseudoClass),
        ("::before.a", MisplacedPseudoElement),
        ("::before a", MisplacedPseudoElement),
        (":not(::before)", MisplacedPseudoElement),
        (":not()", Empty),
        (":not(a,)", Empty),
        (":nth-child(foo)", InvalidAnPlusB),
        (":nth-child(2n of)", InvalidAnPlusB),
        (":nth-of-type(2n of a)", InvalidAnPlusB),
        ("a || b", Unsupported),
    ];
    let mut failures = Vec::new();
    for &(input, kind) in cases {
        let got = parse(input).map(|l| l.to_string());
        if got != Err(kind) {
            failures.push(format!("{input:?}: got {got:?}, want {kind:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn namespaces_resolve_through_the_map() {
    let map = NamespaceMap::new()
        .with_default("http://www.w3.org/1999/xhtml")
        .with_prefix("svg", "http://www.w3.org/2000/svg");
    let list = parse_with("svg|rect, a, [svg|href], svg|*", &map).unwrap();
    assert_eq!(list.to_string(), "svg|rect, a, [svg|href], svg|*");
    assert_eq!(
        format!(
            "{:?}",
            list.selectors()[1].compounds()[0].simple_selectors()[0]
        ),
        "Type { namespace: Implicit(Some(\"http://www.w3.org/1999/xhtml\")), local: \"a\" }"
    );
    assert_eq!(
        parse_with("SVG|rect", &map),
        Err(SelectorErrorKind::UnknownNamespacePrefix)
    );
}

#[test]
fn pseudo_element_and_nesting_limits() {
    let list = parse("p::after").unwrap();
    assert_eq!(
        list.selectors()[0].pseudo_element(),
        Some(PseudoElement::After)
    );
    let deep = |n: usize| format!("{}a{}", ":is(".repeat(n), ")".repeat(n));
    assert!(parse(&deep(rustic_css::MAX_SELECTOR_NESTING)).is_ok());
    assert_eq!(
        parse(&deep(rustic_css::MAX_SELECTOR_NESTING + 1)),
        Err(SelectorErrorKind::NestingLimit)
    );
    // Forgiving lists never forgive the nesting limit.
    assert_eq!(
        parse(&format!(
            ":is(b, {})",
            deep(rustic_css::MAX_SELECTOR_NESTING)
        )),
        Err(SelectorErrorKind::NestingLimit)
    );
}

#[test]
fn error_spans_point_at_the_offending_value() {
    let source = "a > b:hovered";
    let e = CssParser::default()
        .selector_list(source, &NamespaceMap::new())
        .unwrap()
        .value
        .unwrap_err();
    assert_eq!(&source[e.span.range()], "hovered");
}
