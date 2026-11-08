//! The CSS-03c property catalog: positioning, overflow, visibility, text,
//! text decoration, the `font` shorthand and backgrounds.

use rustic_css::{CssParser, PropertyError, parse_declaration};

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
    // Positioning and layout.
    ("position: STICKY", "position: sticky"),
    ("top: -10px", "top: -10px"),
    ("left: calc(50% - 1em)", "left: calc(50% - 1em)"),
    ("right: auto", "right: auto"),
    (
        "inset: 1px 2%",
        "top: 1px; right: 2%; bottom: 1px; left: 2%",
    ),
    ("z-index: -5", "z-index: -5"),
    ("z-index: auto", "z-index: auto"),
    ("z-index: 99999999999", "z-index: 2147483647"),
    ("z-index: calc(2.5)", "z-index: 3"),
    ("z-index: calc(-2.5)", "z-index: -2"),
    ("float: inline-start", "float: inline-start"),
    ("clear: both", "clear: both"),
    ("overflow: hidden", "overflow-x: hidden; overflow-y: hidden"),
    (
        "overflow: clip scroll",
        "overflow-x: clip; overflow-y: scroll",
    ),
    ("overflow-y: auto", "overflow-y: auto"),
    ("visibility: collapse", "visibility: collapse"),
    ("opacity: 0.5", "opacity: 0.5"),
    ("opacity: 50%", "opacity: 0.5"),
    ("opacity: 2", "opacity: 2"),
    ("opacity: calc(1 / 4)", "opacity: 0.25"),
    ("vertical-align: text-top", "vertical-align: text-top"),
    ("vertical-align: -25%", "vertical-align: -25%"),
    ("vertical-align: 2px", "vertical-align: 2px"),
    // Text.
    ("direction: rtl", "direction: rtl"),
    ("text-align: match-parent", "text-align: match-parent"),
    ("text-indent: -2em", "text-indent: -2em"),
    ("text-transform: uppercase", "text-transform: uppercase"),
    (
        "text-transform: full-size-kana capitalize full-width",
        "text-transform: capitalize full-width full-size-kana",
    ),
    ("text-transform: none", "text-transform: none"),
    ("letter-spacing: -0.05em", "letter-spacing: -0.05em"),
    ("word-spacing: normal", "word-spacing: normal"),
    ("word-spacing: 10%", "word-spacing: 10%"),
    ("white-space: pre-wrap", "white-space: pre-wrap"),
    (
        "text-decoration-line: line-through underline",
        "text-decoration-line: underline line-through",
    ),
    ("text-decoration-line: none", "text-decoration-line: none"),
    ("text-decoration-style: wavy", "text-decoration-style: wavy"),
    (
        "text-decoration-color: red",
        "text-decoration-color: rgb(255, 0, 0)",
    ),
    (
        "text-decoration: underline",
        "text-decoration-line: underline; text-decoration-style: solid; text-decoration-color: currentcolor",
    ),
    (
        "text-decoration: red dotted overline underline",
        "text-decoration-line: underline overline; text-decoration-style: dotted; text-decoration-color: rgb(255, 0, 0)",
    ),
    (
        "text-decoration: none blue",
        "text-decoration-line: none; text-decoration-style: solid; text-decoration-color: rgb(0, 0, 255)",
    ),
    // Fonts.
    (
        "font-variant-caps: all-small-caps",
        "font-variant-caps: all-small-caps",
    ),
    ("font-stretch: condensed", "font-stretch: condensed"),
    ("font-stretch: 87.5%", "font-stretch: 87.5%"),
    (
        "font: 12px serif",
        "font-style: normal; font-variant-caps: normal; font-weight: normal; font-stretch: normal; font-size: 12px; line-height: normal; font-family: serif",
    ),
    (
        "font: italic small-caps bold condensed 16px/1.5 \"Fira Sans\", sans-serif",
        "font-style: italic; font-variant-caps: small-caps; font-weight: bold; font-stretch: condensed; font-size: 16px; line-height: 1.5; font-family: \"Fira Sans\", sans-serif",
    ),
    (
        "font: 700 normal italic large / 120% Times New Roman",
        "font-style: italic; font-variant-caps: normal; font-weight: 700; font-stretch: normal; font-size: large; line-height: 120%; font-family: \"Times New Roman\"",
    ),
    (
        "font: normal normal normal normal 0 monospace",
        "font-style: normal; font-variant-caps: normal; font-weight: normal; font-stretch: normal; font-size: 0px; line-height: normal; font-family: monospace",
    ),
    (
        "font: calc(10px + 2px)/calc(1 + 1) bold",
        "font-style: normal; font-variant-caps: normal; font-weight: normal; font-stretch: normal; font-size: calc(12px); line-height: 2; font-family: \"bold\"",
    ),
    // Backgrounds.
    (
        "background-image: url(a.png)",
        "background-image: url(\"a.png\")",
    ),
    (
        "background-image: none, URL( 'b c.png' )",
        "background-image: none, url(\"b c.png\")",
    ),
    (
        "background-position: left",
        "background-position: left center",
    ),
    (
        "background-position: top",
        "background-position: center top",
    ),
    (
        "background-position: 10px",
        "background-position: 10px center",
    ),
    (
        "background-position: top left",
        "background-position: left top",
    ),
    (
        "background-position: center right",
        "background-position: right center",
    ),
    (
        "background-position: 10% 3em",
        "background-position: 10% 3em",
    ),
    (
        "background-position: left 10px",
        "background-position: left 10px",
    ),
    (
        "background-position: 10px bottom",
        "background-position: 10px bottom",
    ),
    (
        "background-position: bottom 10px right",
        "background-position: right bottom 10px",
    ),
    (
        "background-position: right 5% bottom 10px, center",
        "background-position: right 5% bottom 10px, center center",
    ),
    ("background-size: cover", "background-size: cover"),
    ("background-size: 50% auto", "background-size: 50%"),
    (
        "background-size: auto 10px, contain",
        "background-size: auto 10px, contain",
    ),
    ("background-repeat: repeat-x", "background-repeat: repeat-x"),
    (
        "background-repeat: no-repeat repeat",
        "background-repeat: repeat-y",
    ),
    (
        "background-repeat: space round",
        "background-repeat: space round",
    ),
    ("background-repeat: round round", "background-repeat: round"),
    (
        "background-attachment: fixed, local",
        "background-attachment: fixed, local",
    ),
    (
        "background-origin: content-box",
        "background-origin: content-box",
    ),
    (
        "background-clip: padding-box",
        "background-clip: padding-box",
    ),
    (
        "background: red",
        "background-color: rgb(255, 0, 0); background-image: none; background-position: 0% 0%; background-size: auto; background-repeat: repeat; background-attachment: scroll; background-origin: padding-box; background-clip: border-box",
    ),
    (
        "background: url(x.png) no-repeat right 10px top / 50px 2em fixed content-box #00f",
        "background-color: rgb(0, 0, 255); background-image: url(\"x.png\"); background-position: right 10px top; background-size: 50px 2em; background-repeat: no-repeat; background-attachment: fixed; background-origin: content-box; background-clip: content-box",
    ),
    (
        "background: url(a) center / cover, padding-box border-box repeat-y url(b) green",
        "background-color: rgb(0, 128, 0); background-image: url(\"a\"), url(\"b\"); background-position: center center, 0% 0%; background-size: cover, auto; background-repeat: repeat, repeat-y; background-attachment: scroll, scroll; background-origin: padding-box, padding-box; background-clip: border-box, border-box",
    ),
    (
        "background: none",
        "background-color: rgba(0, 0, 0, 0); background-image: none; background-position: 0% 0%; background-size: auto; background-repeat: repeat; background-attachment: scroll; background-origin: padding-box; background-clip: border-box",
    ),
    (
        "background: inherit",
        "background-color: inherit; background-image: inherit; background-position: inherit; background-size: inherit; background-repeat: inherit; background-attachment: inherit; background-origin: inherit; background-clip: inherit",
    ),
];

#[test]
fn catalog_declarations_parse_to_typed_longhands() {
    let failures: Vec<_> = VALID
        .iter()
        .filter_map(|&(input, want)| {
            let got = parse(input);
            (got.as_deref() != Ok(want))
                .then(|| format!("{input:?}:\n  got  {got:?}\n  want {want:?}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn catalog_declarations_serialize_to_a_fixed_point() {
    for &(_, want) in VALID {
        for longhand in want.split("; ") {
            assert_eq!(parse(longhand).as_deref(), Ok(longhand), "{longhand:?}");
        }
    }
}

#[test]
fn invalid_catalog_declarations_are_rejected() {
    for input in [
        "position: absolute fixed",
        "position: center",
        "top: 10",
        "inset: 1px 2px 3px 4px 5px",
        "z-index: 1.5",
        "z-index: 1px",
        "z-index: auto 1",
        "float: center",
        "clear: all",
        "overflow: hidden hidden hidden",
        "overflow: overlay",
        "opacity: 1px",
        "opacity: calc(1px)",
        "vertical-align: center",
        "text-align: middle",
        "text-indent: hanging",
        "text-transform: uppercase lowercase",
        "text-transform: none uppercase",
        "text-transform: full-width full-width",
        "letter-spacing: 1",
        "white-space: wrap",
        "text-decoration-line: underline underline",
        "text-decoration-line: none underline",
        "text-decoration: underline red overline",
        "text-decoration: underline none",
        "text-decoration: solid dotted",
        "text-decoration: red blue",
        "font-variant-caps: small",
        "font-stretch: -10%",
        "font-stretch: 100",
        "font: 12px",
        "font: serif",
        "font: bold italic",
        "font: 12px/ serif",
        "font: 12px / serif",
        "font: caption",
        "font: normal normal normal normal normal 12px serif",
        "font: italic italic 12px serif",
        "font: 2000 12px serif",
        "font: 12px serif,",
        "background-image: linear-gradient(red, blue)",
        "background-image: url(a) url(b)",
        "background-image: none,",
        "background-position: top 10px",
        "background-position: left right",
        "background-position: 10px 20px 30px",
        "background-position: left 10px 20px",
        "background-position: center 10px top",
        "background-position: left 10px top 10px 20px",
        "background-size: -1px",
        "background-size: cover cover",
        "background-repeat: repeat-x repeat",
        "background-attachment: sticky",
        "background-origin: margin-box",
        "background: red, url(a)",
        "background: url(a) url(b)",
        "background: left / ",
        "background: border-box padding-box content-box",
        "background: red blue",
        "background: center / cover cover",
    ] {
        assert_eq!(parse(input), Err(PropertyError::InvalidValue), "{input:?}");
    }
}
