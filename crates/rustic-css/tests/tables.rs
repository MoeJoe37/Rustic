//! Table properties (CSS 2.2 §17 and CSS Tables 3): `border-collapse`,
//! `border-spacing`, `table-layout`, `caption-side` and `empty-cells`.

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

#[test]
fn table_values_parse_and_serialize() {
    for (source, expected) in [
        ("border-collapse: COLLAPSE", "border-collapse: collapse"),
        ("border-collapse: separate", "border-collapse: separate"),
        ("table-layout: fixed", "table-layout: fixed"),
        ("table-layout: auto", "table-layout: auto"),
        ("caption-side: bottom", "caption-side: bottom"),
        ("empty-cells: hide", "empty-cells: hide"),
        ("border-spacing: 0", "border-spacing: 0px"),
        ("border-spacing: 2px 2px", "border-spacing: 2px"),
        ("border-spacing: 1.5px 2em", "border-spacing: 1.5px 2em"),
        (
            "border-spacing: calc(10px + 0.5em) calc(10px - 0.5em)",
            "border-spacing: calc(0.5em + 10px) calc(-0.5em + 10px)",
        ),
        ("border-spacing: inherit", "border-spacing: inherit"),
    ] {
        assert_eq!(parse(source).as_deref(), Ok(expected), "{source}");
    }
}

#[test]
fn invalid_table_values_are_rejected() {
    for source in [
        "border-collapse: auto",
        "table-layout: none",
        "caption-side: left",
        "empty-cells: collapse",
        "border-spacing: 10%",
        "border-spacing: -20px",
        "border-spacing: 30",
        "border-spacing: 40px 50px 60px",
    ] {
        assert_eq!(parse(source), Err(PropertyError::InvalidValue), "{source}");
    }
}

#[test]
fn table_properties_inherit_as_specified() {
    for (property, inherited) in [
        (PropertyId::BorderCollapse, true),
        (PropertyId::BorderSpacing, true),
        (PropertyId::TableLayout, false),
        (PropertyId::CaptionSide, true),
        (PropertyId::EmptyCells, true),
    ] {
        assert_eq!(property.inherited(), inherited, "{property:?}");
    }
    assert_eq!(PropertyId::BorderSpacing.initial_value().to_string(), "0px");
}
