//! Typed property values and shorthand expansion (CSS-03).

use rustic_css::{CssParser, PropertyError, PropertyId, parse_declaration};

fn parse(source: &str) -> Result<String, PropertyError> {
    let declaration = CssParser::default()
        .declaration(source)
        .unwrap()
        .value
        .unwrap_or_else(|| panic!("{source:?} is not a declaration"));
    parse_declaration(&declaration).map(|longhands| {
        longhands
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })
}

const VALID: &[(&str, &str)] = &[
    ("DISPLAY: Block", "display: block"),
    ("display: inline flow-root", "display: inline-block"),
    ("display: flex block", "display: flex"),
    ("display: list-item", "display: list-item"),
    ("color: #0f08", "color: rgba(0, 255, 0, 0.533333)"),
    (
        "background-color: transparent",
        "background-color: rgba(0, 0, 0, 0)",
    ),
    ("width: 50%", "width: 50%"),
    ("height: 0", "height: 0px"),
    ("min-width: auto", "min-width: auto"),
    ("max-height: none", "max-height: none"),
    ("max-width: 12.5EM", "max-width: 12.5em"),
    ("margin-left: -1in", "margin-left: -1in"),
    (
        "margin: 1px",
        "margin-top: 1px; margin-right: 1px; margin-bottom: 1px; margin-left: 1px",
    ),
    (
        "margin: 1px 2px",
        "margin-top: 1px; margin-right: 2px; margin-bottom: 1px; margin-left: 2px",
    ),
    (
        "margin: 1px auto 3px",
        "margin-top: 1px; margin-right: auto; margin-bottom: 3px; margin-left: auto",
    ),
    (
        "padding: 1px 2% 3vh 4rem !important",
        "padding-top: 1px !important; padding-right: 2% !important; padding-bottom: 3vh !important; padding-left: 4rem !important",
    ),
    (
        "border-width: thin thick",
        "border-top-width: thin; border-right-width: thick; border-bottom-width: thin; border-left-width: thick",
    ),
    (
        "border-style: solid",
        "border-top-style: solid; border-right-style: solid; border-bottom-style: solid; border-left-style: solid",
    ),
    (
        "border-top: red 2px dashed",
        "border-top-width: 2px; border-top-style: dashed; border-top-color: rgb(255, 0, 0)",
    ),
    (
        "border-left: none",
        "border-left-width: medium; border-left-style: none; border-left-color: currentcolor",
    ),
    ("box-sizing: border-box", "box-sizing: border-box"),
    ("font-size: x-large", "font-size: x-large"),
    ("font-size: 120%", "font-size: 120%"),
    ("font-weight: 650", "font-weight: 650"),
    ("font-weight: BOLDER", "font-weight: bolder"),
    ("font-style: oblique", "font-style: oblique"),
    (
        "font-family: Times New Roman, 'Noto Serif', serif",
        "font-family: \"Times New Roman\", \"Noto Serif\", serif",
    ),
    ("line-height: 1.5", "line-height: 1.5"),
    ("line-height: 0", "line-height: 0"),
    ("line-height: 20px", "line-height: 20px"),
    ("color: inherit", "color: inherit"),
    ("width: calc(100% - 20px)", "width: calc(100% - 20px)"),
    ("width: CALC(1in)", "width: calc(96px)"),
    ("width: calc(-10px)", "width: calc(-10px)"),
    ("padding-left: min(1em, 5%)", "padding-left: min(1em, 5%)"),
    (
        "margin: calc(1px) auto",
        "margin-top: calc(1px); margin-right: auto; margin-bottom: calc(1px); margin-left: auto",
    ),
    (
        "font-size: clamp(12px, 2vw, 20px)",
        "font-size: clamp(12px, 2vw, 20px)",
    ),
    ("font-weight: calc(2000)", "font-weight: 1000"),
    ("font-weight: calc(100 * 3.5)", "font-weight: 350"),
    ("line-height: calc(1 + 0.5)", "line-height: 1.5"),
    ("line-height: calc(-1)", "line-height: 0"),
    (
        "line-height: calc(1em + 2px)",
        "line-height: calc(1em + 2px)",
    ),
    (
        "border-top-width: calc(1px + 0.5em)",
        "border-top-width: calc(0.5em + 1px)",
    ),
    (
        "margin: REVERT-LAYER",
        "margin-top: revert-layer; margin-right: revert-layer; margin-bottom: revert-layer; margin-left: revert-layer",
    ),
];

#[test]
fn valid_declarations_parse_to_typed_longhands() {
    let failures: Vec<_> = VALID
        .iter()
        .filter_map(|&(input, want)| {
            let got = parse(input);
            (got.as_deref() != Ok(want)).then(|| format!("{input:?}: got {got:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn invalid_declarations_are_rejected() {
    use PropertyError::*;
    let cases = [
        ("cursor: pointer", UnknownProperty),
        ("cursor: var(--x)", UnknownProperty),
        ("display: block inline", InvalidValue),
        ("display: inline-block flow", InvalidValue),
        ("width: -1px", InvalidValue),
        ("width: 10", InvalidValue),
        ("width: 10furlongs", InvalidValue),
        ("width: calc(1px + 2)", InvalidValue),
        ("width: calc(1deg)", InvalidValue),
        ("border-top-width: calc(10%)", InvalidValue),
        ("font-weight: calc(1px)", InvalidValue),
        ("line-height: calc(1 + 1px)", InvalidValue),
        ("padding: -1px", InvalidValue),
        ("margin: 1px 2px 3px 4px 5px", InvalidValue),
        ("margin: 1px, 2px", InvalidValue),
        ("border-top-width: -1px", InvalidValue),
        ("border: solid solid", InvalidValue),
        ("font-weight: 0", InvalidValue),
        ("font-weight: 1001", InvalidValue),
        ("font-family: serif,", InvalidValue),
        ("font-family: inherit, serif", InvalidValue),
        ("font-family: 12px", InvalidValue),
        ("line-height: -1", InvalidValue),
        ("color: inherit inherit", InvalidValue),
        ("color: var(x)", InvalidValue),
        ("color: var()", InvalidValue),
        ("color: var(--x --y)", InvalidValue),
        ("color: var(--x) ]", InvalidValue),
        ("color: calc(var(1px))", InvalidValue),
        ("--x: a ]", InvalidValue),
    ];
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|&(input, kind)| {
            let got = parse(input);
            (got != Err(kind)).then(|| format!("{input:?}: got {got:?}, want {kind:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_longhand_has_an_initial_value_that_reparses() {
    let mut inherited = Vec::new();
    for &property in PropertyId::ALL {
        let initial = property.initial_value();
        let source = format!("{}: {}", property.name(), initial);
        assert_eq!(parse(&source), Ok(source.clone()), "{property:?}");
        assert_eq!(
            PropertyId::from_name(&property.name().to_uppercase()),
            Some(property)
        );
        if property.inherited() {
            inherited.push(property.name());
        }
    }
    assert_eq!(
        inherited,
        [
            "color",
            "font-size",
            "font-weight",
            "font-style",
            "font-family",
            "line-height",
            "font-variant-caps",
            "font-stretch",
            "visibility",
            "border-collapse",
            "border-spacing",
            "caption-side",
            "empty-cells",
            "list-style-type",
            "list-style-position",
            "direction",
            "text-align",
            "text-indent",
            "text-transform",
            "letter-spacing",
            "word-spacing",
            "line-break",
            "word-break",
            "overflow-wrap",
            "hyphens",
            "white-space",
            "writing-mode",
            "text-orientation",
            "text-combine-upright",
        ]
    );
}

#[test]
fn absolute_lengths_convert_to_pixels() {
    use rustic_css::{Length, LengthUnit};
    let px = |value, unit| Length { value, unit }.to_px();
    assert_eq!(px(1.0, LengthUnit::In), Some(96.0));
    assert_eq!(px(12.0, LengthUnit::Pt), Some(16.0));
    assert_eq!(px(1.0, LengthUnit::Pc), Some(16.0));
    assert!((px(2.54, LengthUnit::Cm).unwrap() - 96.0).abs() < 1e-9);
    assert_eq!(px(1.0, LengthUnit::Em), None);
}

#[test]
fn text_policies_parse_serialize_and_reject_invalid_keywords() {
    for (property, values) in [
        (
            "unicode-bidi",
            &[
                "normal",
                "embed",
                "bidi-override",
                "isolate",
                "isolate-override",
                "plaintext",
            ][..],
        ),
        (
            "line-break",
            &["auto", "loose", "normal", "strict", "anywhere"],
        ),
        (
            "word-break",
            &[
                "normal",
                "break-all",
                "keep-all",
                "break-word",
                "manual",
                "auto-phrase",
            ],
        ),
        ("overflow-wrap", &["normal", "break-word", "anywhere"]),
        ("hyphens", &["none", "manual", "auto"]),
    ] {
        for value in values {
            let s = format!("{property}: {value}");
            assert_eq!(parse(&s), Ok(s.clone()));
        }
        assert!(parse(&format!("{property}: invalid")).is_err());
        assert!(parse(&format!("{property}: normal normal")).is_err());
    }
    assert_eq!(
        parse("word-wrap: anywhere"),
        Ok("overflow-wrap: anywhere".into())
    );
    assert!(!PropertyId::UnicodeBidi.inherited());
}
