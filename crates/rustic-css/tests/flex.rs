//! Sizing keywords (CSS Sizing 3/4), flexbox properties (CSS Flexbox 1 §5–§8)
//! and the box alignment values flex layout uses (CSS Box Alignment 3).

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
    // Sizing keywords.
    ("width: MIN-CONTENT", "width: min-content"),
    ("height: max-content", "height: max-content"),
    ("min-width: fit-content", "min-width: fit-content"),
    ("width: fit-content(10%)", "width: fit-content(10%)"),
    (
        "max-width: fit-content(calc(10% - 0.5em))",
        "max-width: fit-content(calc(10% - 0.5em))",
    ),
    ("max-height: min-content", "max-height: min-content"),
    ("max-width: none", "max-width: none"),
    ("min-height: auto", "min-height: auto"),
    ("width: 0", "width: 0px"),
    // Flex container.
    (
        "flex-direction: column-reverse",
        "flex-direction: column-reverse",
    ),
    ("flex-wrap: wrap-reverse", "flex-wrap: wrap-reverse"),
    (
        "flex-flow: wrap column",
        "flex-direction: column; flex-wrap: wrap",
    ),
    (
        "flex-flow: row-reverse",
        "flex-direction: row-reverse; flex-wrap: nowrap",
    ),
    ("flex-flow: wrap", "flex-direction: row; flex-wrap: wrap"),
    ("row-gap: 10px", "row-gap: 10px"),
    ("column-gap: normal", "column-gap: normal"),
    ("gap: 5%", "row-gap: 5%; column-gap: 5%"),
    ("gap: 1px 2em", "row-gap: 1px; column-gap: 2em"),
    // Flex items.
    ("order: -3", "order: -3"),
    ("order: calc(1.5)", "order: 2"),
    ("flex-grow: 2.5", "flex-grow: 2.5"),
    ("flex-shrink: 0", "flex-shrink: 0"),
    ("flex-basis: content", "flex-basis: content"),
    ("flex-basis: auto", "flex-basis: auto"),
    ("flex-basis: 30%", "flex-basis: 30%"),
    ("flex-basis: max-content", "flex-basis: max-content"),
    (
        "flex: none",
        "flex-grow: 0; flex-shrink: 0; flex-basis: auto",
    ),
    (
        "flex: auto",
        "flex-grow: 1; flex-shrink: 1; flex-basis: auto",
    ),
    ("flex: 2", "flex-grow: 2; flex-shrink: 1; flex-basis: 0px"),
    ("flex: 0", "flex-grow: 0; flex-shrink: 1; flex-basis: 0px"),
    ("flex: 2 3", "flex-grow: 2; flex-shrink: 3; flex-basis: 0px"),
    (
        "flex: 1 0 auto",
        "flex-grow: 1; flex-shrink: 0; flex-basis: auto",
    ),
    (
        "flex: 1 1 0",
        "flex-grow: 1; flex-shrink: 1; flex-basis: 0px",
    ),
    (
        "flex: 10px",
        "flex-grow: 1; flex-shrink: 1; flex-basis: 10px",
    ),
    (
        "flex: content 2",
        "flex-grow: 2; flex-shrink: 1; flex-basis: content",
    ),
    (
        "flex: 20% 3 4",
        "flex-grow: 3; flex-shrink: 4; flex-basis: 20%",
    ),
    (
        "flex: 3 20%",
        "flex-grow: 3; flex-shrink: 1; flex-basis: 20%",
    ),
    (
        "flex: inherit",
        "flex-grow: inherit; flex-shrink: inherit; flex-basis: inherit",
    ),
    // Alignment.
    (
        "justify-content: space-between",
        "justify-content: space-between",
    ),
    (
        "justify-content: safe center",
        "justify-content: safe center",
    ),
    ("justify-content: left", "justify-content: left"),
    (
        "justify-content: unsafe flex-end",
        "justify-content: unsafe flex-end",
    ),
    ("align-content: space-evenly", "align-content: space-evenly"),
    ("align-content: first baseline", "align-content: baseline"),
    ("align-items: stretch", "align-items: stretch"),
    ("align-items: last baseline", "align-items: last baseline"),
    ("align-items: self-end", "align-items: self-end"),
    (
        "align-items: safe flex-start",
        "align-items: safe flex-start",
    ),
    ("align-self: auto", "align-self: auto"),
    ("align-self: center", "align-self: center"),
];

#[test]
fn flex_and_sizing_declarations_parse_to_typed_longhands() {
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
fn flex_and_sizing_longhands_serialize_to_a_fixed_point() {
    for &(_, want) in VALID {
        for longhand in want.split("; ") {
            assert_eq!(parse(longhand).as_deref(), Ok(longhand), "{longhand:?}");
        }
    }
}

#[test]
fn invalid_flex_and_sizing_declarations_are_rejected() {
    for input in [
        "width: min-content max-content",
        "width: fit-content()",
        "width: fit-content(-1px)",
        "width: fit-content(1px, 2px)",
        "width: stretch-content",
        "min-width: none",
        "max-width: auto",
        "flex-direction: horizontal",
        "flex-wrap: wrap nowrap",
        "flex-flow: row column",
        "flex-flow: wrap wrap",
        "flex-grow: -1",
        "flex-grow: 1px",
        "flex-shrink: auto",
        "flex-basis: -5px",
        "flex-basis: none",
        "flex: 1 2 3",
        "flex: none 1",
        "flex: 1px 2px",
        "flex: 1 2 3px 4",
        "flex: -1",
        "order: 1.5",
        "order: auto",
        "gap: -1px",
        "gap: 1px 2px 3px",
        "row-gap: auto",
        "justify-content: stretch-start",
        "justify-content: baseline",
        "justify-content: self-start",
        "justify-content: safe space-between",
        "justify-content: auto",
        "align-content: left",
        "align-items: auto",
        "align-items: space-between",
        "align-items: left",
        "align-items: safe stretch",
        "align-items: first",
        "align-self: safe auto",
        "align-self: baseline last",
    ] {
        assert!(parse(input).is_err(), "{input:?} should be rejected");
    }
}
