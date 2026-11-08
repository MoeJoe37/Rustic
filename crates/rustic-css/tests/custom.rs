//! Custom property declarations and `var()` at parse time (CSS-06).

use rustic_css::{
    CssParser, CustomValue, DeclaredValue, MAX_SUBSTITUTED_VALUES, PropertyId, PropertyName,
    parse_declaration,
};

fn declarations(source: &str) -> Vec<rustic_css::PropertyDeclaration> {
    let declaration = CssParser::default()
        .declaration(source)
        .unwrap()
        .value
        .unwrap_or_else(|| panic!("{source:?} is not a declaration"));
    parse_declaration(&declaration).unwrap_or_else(|e| panic!("{source:?}: {e}"))
}

fn custom(source: &str) -> CustomValue {
    match declarations(&format!("--v: {source}")).remove(0).value {
        DeclaredValue::Custom(value) => value,
        other => panic!("{other:?}"),
    }
}

#[test]
fn custom_properties_keep_their_tokens_and_case() {
    for (input, want) in [
        ("--Main-Color: #F00", "--Main-Color: #F00"),
        ("--x:   a   b  ", "--x: a b"),
        ("--x: {a; b}", "--x: {a; b}"),
        ("--x: 1px!important", "--x: 1px !important"),
        ("--x: initial", "--x: initial"),
        ("--x:", "--x: "),
        ("--x: var(--y, )", "--x: var(--y, )"),
        ("--\\31 x: 1", "--1x: 1"),
    ] {
        let parsed = declarations(input);
        assert_eq!(parsed.len(), 1, "{input}");
        assert_eq!(parsed[0].to_string(), want, "{input}");
    }
    let parsed = declarations("--x: inherit");
    assert!(matches!(parsed[0].value, DeclaredValue::CssWide(_)));
    assert_eq!(parsed[0].property.custom(), Some("--x"));
}

#[test]
fn var_values_are_kept_for_every_longhand() {
    let parsed = declarations("margin: var(--a) 2px");
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[0].property, PropertyId::MarginTop);
    assert_eq!(
        parsed[3].property,
        PropertyName::Longhand(PropertyId::MarginLeft)
    );
    for d in &parsed {
        let DeclaredValue::Unparsed(value) = &d.value else {
            panic!("{d:?}")
        };
        assert_eq!(value.property(), "margin");
    }
    assert_eq!(parsed[2].to_string(), "margin-bottom: var(--a) 2px");
    // Case-insensitive function name; nested in other functions.
    assert_eq!(declarations("width: calc(VAR(--w) * 2)").len(), 1);
}

#[test]
fn references_and_substitution() {
    let value = custom("var(--a) f(var(--b, var(--c))) x");
    assert!(value.has_references());
    assert_eq!(value.references(), ["--a", "--b", "--c"]);
    let a = custom("1px 2px");
    let lookup = |name: &str| (name == "--a").then_some(&a);
    let mut budget = MAX_SUBSTITUTED_VALUES;
    // --b and --c are missing and there is no final fallback.
    assert!(value.substitute(&lookup, &mut budget).is_none());
    let value = custom("var(--a) f(var(--b, var(--c, z))) x");
    let out = value.substitute(&lookup, &mut budget).unwrap();
    assert_eq!(out.to_string(), "1px 2px f(z) x");
    assert!(!out.has_references());
    // Tokens never merge: adjacent substitutions get a comment between.
    let value = custom("var(--a)var(--a)");
    let out = value.substitute(&lookup, &mut budget).unwrap();
    assert_eq!(out.to_string(), "1px 2px/**/1px 2px");
}

#[test]
fn substitution_is_bounded() {
    let big = custom(&vec!["x"; 1000].join(" "));
    let lookup = |_: &str| Some(&big);
    let value = custom(&vec!["var(--a)"; 100].join(" "));
    let mut budget = MAX_SUBSTITUTED_VALUES;
    assert!(value.substitute(&lookup, &mut budget).is_none());
    // Depth: a deeply nested value cannot be nested further.
    let deep = custom(&format!("{}x{}", "(".repeat(100), ")".repeat(100)));
    let lookup = |_: &str| Some(&deep);
    let mut budget = MAX_SUBSTITUTED_VALUES;
    let value = custom(&format!("{}var(--a){}", "f(".repeat(20), ")".repeat(20)));
    assert!(value.substitute(&lookup, &mut budget).is_some());
    let value = custom(&format!("{}var(--a){}", "f(".repeat(40), ")".repeat(40)));
    assert!(value.substitute(&lookup, &mut budget).is_none());
}
