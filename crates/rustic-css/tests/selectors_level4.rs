//! Selector syntax added by CSS-07a: `:has()`, form-state, `:lang()` and
//! `:dir()` pseudo-classes, the nesting selector `&`, more pseudo-elements and
//! user-action pseudo-classes after a pseudo-element.

use std::sync::Arc;

use rustic_css::{
    Combinator, CssParser, ElementState, FormState, MAX_SELECTOR_NESTING, NamespaceMap,
    PseudoClass, PseudoElement, SelectorErrorKind, SelectorList, SimpleSelector, Specificity,
    parse_nested_selector_list,
};

fn parse(source: &str) -> Result<SelectorList, SelectorErrorKind> {
    CssParser::default()
        .selector_list(source, &NamespaceMap::new())
        .expect("within limits")
        .value
        .map_err(|e| e.kind)
}

fn nested(source: &str, parent: &SelectorList) -> Result<SelectorList, SelectorErrorKind> {
    let values = CssParser::default()
        .component_values(source)
        .expect("within limits")
        .value;
    parse_nested_selector_list(&values, &NamespaceMap::new(), &Arc::new(parent.clone()))
        .map_err(|e| e.kind)
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
    // :has() takes its most specific argument, like :is().
    ("a:has(> img)", "a:has(> img)", (0, 0, 2)),
    (":has(+ #x, ~ .y)", ":has(+ #x, ~ .y)", (1, 0, 0)),
    (":has(  b  c )", ":has(b c)", (0, 0, 2)),
    (":has(> a + b ~ c d)", ":has(> a + b ~ c d)", (0, 0, 4)),
    (":has(:is(a, #b))", ":has(:is(a, #b))", (1, 0, 0)),
    (":not(:has(p))", ":not(:has(p))", (0, 0, 1)),
    // Form states.
    (
        "input:checked:enabled:required",
        "input:checked:enabled:required",
        (0, 3, 1),
    ),
    (
        ":DISABLED:indeterminate:default:optional",
        ":disabled:indeterminate:default:optional",
        (0, 4, 0),
    ),
    (
        ":read-only:read-write:placeholder-shown",
        ":read-only:read-write:placeholder-shown",
        (0, 3, 0),
    ),
    // :lang() serializes ranges as strings; :dir() accepts any ident.
    (":lang(en)", ":lang(\"en\")", (0, 1, 0)),
    (
        ":lang( de-DE , \"*-CH\", '' )",
        ":lang(\"de-DE\", \"*-CH\", \"\")",
        (0, 1, 0),
    ),
    (":dir(RTL)", ":dir(rtl)", (0, 1, 0)),
    (":dir(sideways)", ":dir(sideways)", (0, 1, 0)),
    // The nesting selector outside a nested rule is :scope with zero
    // specificity.
    ("&", "&", (0, 0, 0)),
    ("& > a", "& > a", (0, 0, 1)),
    ("div&.x", "div&.x", (0, 1, 1)),
    (":is(&)", ":is(&)", (0, 0, 0)),
    // Pseudo-elements and the user-action states that may follow them.
    ("::placeholder", "::placeholder", (0, 0, 1)),
    (
        "input::file-selector-button",
        "input::file-selector-button",
        (0, 0, 2),
    ),
    ("dialog::backdrop", "dialog::backdrop", (0, 0, 2)),
    ("::selection", "::selection", (0, 0, 1)),
    ("::target-text", "::target-text", (0, 0, 1)),
    (
        "::spelling-error, ::grammar-error",
        "::spelling-error, ::grammar-error",
        (0, 0, 1),
    ),
    ("a::before:hover", "a::before:hover", (0, 1, 2)),
    (
        "::after:focus:active:focus-visible",
        "::after:focus:active:focus-visible",
        (0, 3, 1),
    ),
    (":before:hover", "::before:hover", (0, 1, 1)),
    (
        "::marker:not(:hover):is(:focus, :active)",
        "::marker:not(:hover):is(:focus, :active)",
        (0, 2, 1),
    ),
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
        // :has() is unforgiving, cannot nest (even through :is()) and cannot
        // contain pseudo-elements.
        (":has()", Empty),
        (":has(a,)", Empty),
        (":has(>)", DanglingCombinator),
        (":has(a >)", DanglingCombinator),
        (":has(!!)", UnexpectedToken),
        (":has(:has(a))", UnsupportedPseudoClass),
        (":has(::before)", MisplacedPseudoElement),
        (":has(a, ::before)", MisplacedPseudoElement),
        // :lang() and :dir() arguments.
        (":lang()", UnexpectedToken),
        (":lang(en fr)", UnexpectedToken),
        (":lang(1)", UnexpectedToken),
        (":lang(en,)", UnexpectedToken),
        (":dir()", UnexpectedToken),
        (":dir(ltr rtl)", UnexpectedToken),
        (":dir(\"ltr\")", UnexpectedToken),
        // A type selector must come first in a compound.
        ("&div", UnexpectedToken),
        // Pseudo-element states.
        ("::before:first-child", InvalidPseudoElementState),
        ("::before:target", InvalidPseudoElementState),
        ("::selection:hover", InvalidPseudoElementState),
        ("::before:is(:hover, .a)", InvalidPseudoElementState),
        ("::before:hover::after", MisplacedPseudoElement),
        ("::before:hover a", MisplacedPseudoElement),
        ("::before:nope", UnsupportedPseudoClass),
        (":selection", UnsupportedPseudoClass),
    ];
    let mut failures = Vec::new();
    for &(input, kind) in cases {
        let got = parse(input).map(|l| l.to_string());
        if got != Err(kind) {
            failures.push(format!("{input:?}: got {got:?}, want {kind:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(parse(":is(:has(a))").unwrap().to_string(), ":is(:has(a))");
    assert_eq!(
        parse(":has(:is(:has(a), b))").unwrap().to_string(),
        ":has(:is(b))"
    );
}

#[test]
fn parsed_values_are_typed() {
    let list = parse("input:checked:lang(fr):dir(ltr):has(> b)::before:hover").unwrap();
    let selector = &list.selectors()[0];
    let simple = selector.compounds()[0].simple_selectors();
    assert_eq!(
        simple[1],
        SimpleSelector::PseudoClass(PseudoClass::Form(FormState::Checked))
    );
    assert_eq!(
        simple[2],
        SimpleSelector::PseudoClass(PseudoClass::Lang(vec!["fr".to_owned()]))
    );
    assert_eq!(
        simple[3],
        SimpleSelector::PseudoClass(PseudoClass::Dir("ltr".to_owned()))
    );
    let SimpleSelector::PseudoClass(PseudoClass::Has(has)) = &simple[4] else {
        panic!("expected :has()");
    };
    let relative = &has.selectors()[0];
    assert_eq!(relative.leading_combinator(), Combinator::Child);
    assert_eq!(
        relative.selector().compounds()[0].simple_selectors(),
        &[SimpleSelector::RelativeAnchor]
    );
    assert_eq!(selector.pseudo_element(), Some(PseudoElement::Before));
    assert_eq!(
        selector.pseudo_element_states(),
        &[PseudoClass::State(ElementState::Hover)]
    );
}

#[test]
fn nested_selectors_resolve_against_the_parent() {
    let parent = parse(".a, #b, p::before").unwrap();
    // `(input, serialization, specificity of the first selector)`; the
    // parent's highest specificity is (1, 0, 0), and its pseudo-element
    // selector is dropped.
    let cases: &[(&str, &str, (u32, u32, u32))] = &[
        ("c", "& c", (1, 0, 1)),
        ("> c", "& > c", (1, 0, 1)),
        ("+ c, ~ d", "& + c, & ~ d", (1, 0, 1)),
        ("&.x", "&.x", (1, 1, 0)),
        ("c &", "c &", (1, 0, 1)),
        ("& &", "& &", (2, 0, 0)),
        (":is(&) c", ":is(&) c", (1, 0, 1)),
        ("> &", "& > &", (2, 0, 0)),
        ("&::after", "&::after", (1, 0, 1)),
        (":has(> &)", ":has(> &)", (1, 0, 0)),
    ];
    let mut failures = Vec::new();
    for &(input, serialized, (a, b, c)) in cases {
        match nested(input, &parent) {
            Ok(list) => {
                let text = list.to_string();
                let s = list.selectors()[0].specificity();
                if text != serialized || s != spec(a, b, c) {
                    failures.push(format!("{input:?}: got {text:?} {s:?}"));
                }
                // Serialization is a fixed point under the same parent.
                let again = nested(&text, &parent).map(|l| l.to_string());
                if again.as_deref() != Ok(text.as_str()) {
                    failures.push(format!("{input:?}: reparse gave {again:?}"));
                }
            }
            Err(e) => failures.push(format!("{input:?}: unexpected {e:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let resolved = nested("c", &parent).unwrap();
    let SimpleSelector::Nesting(Some(list)) =
        &resolved.selectors()[0].compounds()[0].simple_selectors()[0]
    else {
        panic!("expected a resolved &");
    };
    assert_eq!(list.to_string(), ".a, #b");
    assert_eq!(
        nested(">", &parent),
        Err(SelectorErrorKind::DanglingCombinator)
    );
    assert_eq!(nested("c,", &parent), Err(SelectorErrorKind::Empty));
    assert_eq!(
        nested("::before c", &parent),
        Err(SelectorErrorKind::MisplacedPseudoElement)
    );
}

#[test]
fn nested_parents_share_storage_and_respect_the_nesting_limit() {
    // Each level refers to the previous list instead of copying it, so a
    // deep chain of two-entry rules stays linear in size.
    let mut parent = parse("a, b").unwrap();
    let mut depth = 0;
    loop {
        match nested("& c, & d", &parent) {
            Ok(list) => {
                parent = list;
                depth += 1;
            }
            Err(kind) => {
                assert_eq!(kind, SelectorErrorKind::NestingLimit);
                break;
            }
        }
        assert!(depth <= MAX_SELECTOR_NESTING, "no limit");
    }
    assert_eq!(depth, MAX_SELECTOR_NESTING);
    assert!(parent.to_string().len() < 40);
}
