//! `@container` preludes, container properties and query evaluation (CSS-06).

use rustic_css::{
    ContainerFacts, CssParser, CustomValue, MediaCondition, parse_container_query_list,
    parse_declaration, parse_media_query_list,
};

fn values(source: &str) -> Vec<rustic_css::Value> {
    CssParser::default().component_values(source).unwrap().value
}

fn prelude(source: &str) -> Option<String> {
    parse_container_query_list(&values(source)).map(|l| l.to_string())
}

#[test]
fn preludes_parse_and_serialize() {
    for (input, want) in [
        ("(width > 400px)", "(width > 400px)"),
        ("card (min-width: 10em)", "card (min-width: 10em)"),
        ("Card", "Card"),
        (
            "(inline-size>=1px) AND (block-size < 2px)",
            "(inline-size >= 1px) and (block-size < 2px)",
        ),
        ("not (orientation: portrait)", "not (orientation: portrait)"),
        (
            "(aspect-ratio: 16/9) or (height)",
            "(aspect-ratio: 16 / 9) or (height)",
        ),
        (
            "a (width > 1px), b (width < 1px)",
            "a (width > 1px), b (width < 1px)",
        ),
        ("style(--theme: dark)", "style(--theme: dark)"),
        ("style( --x )", "style(--x)"),
        (
            "style((--a: 1) and (--b:  2 3))",
            "style((--a: 1) and (--b: 2 3))",
        ),
        ("style(not (--a: 1))", "style(not (--a: 1))"),
        (
            "sidebar style(--x: 1) and (width > 0px)",
            "sidebar style(--x: 1) and (width > 0px)",
        ),
        // A media feature is not a container feature: general-enclosed.
        ("(color)", "(color)"),
        ("foo(bar)", "foo(bar)"),
        ("style(--x 1)", "style(--x 1)"),
        ("style((()))", "style((()))"),
        ("style(((--a: 1)))", "style(--a: 1)"),
    ] {
        assert_eq!(prelude(input).as_deref(), Some(want), "{input}");
    }
    for invalid in [
        "",
        "none (width > 1px)",
        "and",
        "a b",
        "(width > 1px) and",
        "(width > 1px) and (height > 1px) or (width)",
        "inherit (width > 1px)",
        "a (width > 1px),",
    ] {
        assert_eq!(prelude(invalid), None, "{invalid:?}");
    }
}

#[test]
fn container_features_stay_out_of_media_queries() {
    let list = parse_media_query_list(&values("(inline-size > 1px)"));
    assert_eq!(list.to_string(), "(inline-size > 1px)");
    assert!(matches!(
        &list.queries[0],
        rustic_css::MediaQuery::Query {
            condition: Some(MediaCondition::Unknown(_)),
            ..
        }
    ));
    let list = parse_media_query_list(&values("style(--x: 1)"));
    assert!(matches!(
        &list.queries[0],
        rustic_css::MediaQuery::Query {
            condition: Some(MediaCondition::Unknown(_)),
            ..
        }
    ));
}

fn evaluate(source: &str, width: Option<f64>, height: Option<f64>) -> Option<bool> {
    let list = parse_container_query_list(&values(source)).unwrap();
    let facts = ContainerFacts {
        width,
        height,
        font_size_px: 20.0,
        root_font_size_px: 10.0,
        viewport_width: 1000.0,
        viewport_height: 500.0,
    };
    let style = |name: &str, value: Option<&CustomValue>| match (name, value) {
        ("--on", None) => Some(true),
        ("--off", None) => Some(false),
        ("--theme", Some(v)) => Some(v.to_string() == "dark"),
        _ => None,
    };
    list.conditions[0]
        .query
        .as_ref()
        .unwrap()
        .evaluate_container(&facts, &style)
}

#[test]
fn queries_evaluate_three_valued() {
    let both = (Some(400.0), Some(300.0));
    for (query, want) in [
        ("(width > 399px)", Some(true)),
        ("(width = 400px)", Some(true)),
        ("(200px < inline-size < 500px)", Some(true)),
        ("(min-width: 21em)", Some(false)),
        ("(width >= 20em)", Some(true)),
        ("(width >= 40rem)", Some(true)),
        ("(width = 40vw)", Some(true)),
        ("(height > 60vh)", Some(false)),
        ("(aspect-ratio > 1)", Some(true)),
        ("(aspect-ratio: 4/3)", Some(true)),
        ("(orientation: landscape)", Some(true)),
        ("(orientation: portrait)", Some(false)),
        ("(block-size)", Some(true)),
        ("not (width > 1px)", Some(false)),
        ("(color) or (width > 1px)", Some(true)),
        ("(color) and (width > 1px)", None),
        ("style(--on) and style(--theme: dark)", Some(true)),
        ("style(--off) or style(not (--on))", Some(false)),
        ("style(--theme: light)", Some(false)),
        ("style(color: red)", None),
    ] {
        assert_eq!(evaluate(query, both.0, both.1), want, "{query}");
    }
    // Unknown sizes make size features unknown.
    assert_eq!(evaluate("(width > 1px)", None, None), None);
    assert_eq!(evaluate("(height > 1px)", Some(5.0), None), None);
    assert_eq!(evaluate("(aspect-ratio > 1)", Some(5.0), None), None);
    assert_eq!(evaluate("(width > 1px)", Some(5.0), None), Some(true));
}

#[test]
fn axes_follow_the_features() {
    let axes = |s: &str| parse_container_query_list(&values(s)).unwrap().conditions[0].axes();
    assert!(axes("(width > 1px)").inline && !axes("(width > 1px)").block);
    assert!(!axes("(block-size > 1px)").inline && axes("(block-size > 1px)").block);
    let both = axes("(orientation: portrait)");
    assert!(both.inline && both.block);
    let none = axes("style(--x)");
    assert!(!none.inline && !none.block);
}

fn declaration(source: &str) -> Result<String, rustic_css::PropertyError> {
    let declaration = CssParser::default()
        .declaration(source)
        .unwrap()
        .value
        .unwrap();
    parse_declaration(&declaration).map(|longhands| {
        longhands
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })
}

#[test]
fn container_properties() {
    for (input, want) in [
        ("container-type: inline-size", "container-type: inline-size"),
        ("container-type: SIZE", "container-type: size"),
        ("container-name: none", "container-name: none"),
        ("container-name: Main side", "container-name: Main side"),
        (
            "container: card",
            "container-name: card; container-type: normal",
        ),
        (
            "container: a b / size",
            "container-name: a b; container-type: size",
        ),
        (
            "container: none / inline-size",
            "container-name: none; container-type: inline-size",
        ),
    ] {
        assert_eq!(declaration(input).as_deref(), Ok(want), "{input}");
    }
    for invalid in [
        "container-type: scroll",
        "container-name: and",
        "container-name: a none",
        "container-name: 1px",
        "container: / size",
        "container: a / size size",
        "container-name: default",
    ] {
        assert!(declaration(invalid).is_err(), "{invalid}");
    }
}
