//! Syntax definitions for registered custom properties (`@property`).

use rustic_css::{CssParser, CustomValue, PropertySyntax};

fn value(source: &str) -> CustomValue {
    let values = CssParser::default().component_values(source).unwrap().value;
    CustomValue::new(&values).unwrap()
}

#[test]
fn syntax_definitions_parse_and_serialize() {
    for (input, want) in [
        ("*", "*"),
        ("  *  ", "*"),
        ("<length>", "<length>"),
        ("<length> | auto", "<length> | auto"),
        ("<length-percentage>+", "<length-percentage>+"),
        ("<color>#", "<color>#"),
        ("<integer> | <number>", "<integer> | <number>"),
        ("<custom-ident>|none", "<custom-ident> | none"),
        ("big | small", "big | small"),
    ] {
        let syntax = PropertySyntax::parse(input).unwrap_or_else(|| panic!("{input:?}"));
        assert_eq!(syntax.to_string(), want, "{input:?}");
    }
}

#[test]
fn invalid_or_unsupported_syntax_definitions_are_rejected() {
    for input in [
        "",
        "|",
        "<length> |",
        "<length>++",
        "<angle>",
        "<image>",
        "<transform-list>",
        "<Length>",
        "initial",
        "inherit | <length>",
        "default",
        "1a",
        "<length> <number>",
        "* | <length>",
    ] {
        assert!(PropertySyntax::parse(input).is_none(), "{input:?}");
    }
}

#[test]
fn values_match_and_canonicalize() {
    for (syntax, input, want) in [
        ("<length>", "10px", Some("10px")),
        ("<length>", "0", Some("0px")),
        ("<length>", "2em", Some("2em")),
        ("<length>", "50%", None),
        ("<length>", "10px 20px", None),
        ("<length>", "calc(1px + 2px)", Some("calc(3px)")),
        ("<length-percentage>", "50%", Some("50%")),
        ("<length>+", "1px  2px 3px", Some("1px 2px 3px")),
        ("<length>+", "1px, 2px", None),
        ("<length>#", "1px,2px , 3px", Some("1px, 2px, 3px")),
        ("<length>#", "1px,", None),
        ("<number>", "1.50", Some("1.5")),
        ("<integer>", "3", Some("3")),
        ("<integer>", "3.5", None),
        ("<percentage>", "40%", Some("40%")),
        ("<color>", "red", Some("rgb(255, 0, 0)")),
        ("<color>", "#0f0c", Some("rgba(0, 255, 0, 0.8)")),
        ("<custom-ident>", "Foo", Some("Foo")),
        ("<custom-ident>", "inherit", None),
        ("big | small", "small", Some("small")),
        ("big | small", "Small", None),
        ("<length> | auto", "auto", Some("auto")),
        ("<length> | auto", "4px", Some("4px")),
    ] {
        let syntax = PropertySyntax::parse(syntax).unwrap();
        let got = syntax.parse_value(&value(input)).map(|v| v.to_string());
        assert_eq!(got.as_deref(), want, "{syntax} {input:?}");
        assert_eq!(syntax.matches(&value(input)), want.is_some());
    }
    assert!(PropertySyntax::Universal.matches(&value("anything ( at ) all")));
    assert!(
        PropertySyntax::Universal
            .parse_value(&value("1px"))
            .is_none()
    );
}

#[test]
fn font_relative_units_and_independence() {
    let lengths = PropertySyntax::parse("<length>+").unwrap();
    let check = |input: &str, root: bool| {
        let parsed = lengths.parse_value(&value(input)).unwrap();
        (
            parsed.font_relative(root),
            parsed.is_computationally_independent(),
        )
    };
    assert_eq!(check("1px 2in", false), ((false, false), true));
    assert_eq!(check("1em", false), ((true, false), false));
    assert_eq!(check("1px 2lh", false), ((true, true), false));
    assert_eq!(check("1rem", false), ((false, false), false));
    assert_eq!(check("1rem", true), ((true, false), false));
    assert_eq!(check("1rlh", true), ((true, true), false));
    assert_eq!(check("1vw", false), ((false, false), false));
    assert_eq!(check("calc(1px + 1ex)", false), ((true, false), false));
}
