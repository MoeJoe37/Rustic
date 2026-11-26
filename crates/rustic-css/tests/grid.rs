//! Grid properties (CSS Grid Layout 2 §7–§8) and the box alignment values
//! grid layout adds (CSS Box Alignment 3 `justify-items`, `justify-self`,
//! `place-*`).

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
    // Track lists.
    ("grid-template-columns: none", "grid-template-columns: none"),
    (
        "grid-template-columns: 100px 1fr AUTO",
        "grid-template-columns: 100px 1fr auto",
    ),
    (
        "grid-template-rows: [a] 10% [b c] minmax(10px, 2.5fr) [d]",
        "grid-template-rows: [a] 10% [b c] minmax(10px, 2.5fr) [d]",
    ),
    (
        "grid-template-columns: fit-content(40%) min-content max-content",
        "grid-template-columns: fit-content(40%) min-content max-content",
    ),
    (
        "grid-template-columns: repeat(3, [x] 1fr)",
        "grid-template-columns: repeat(3, [x] 1fr)",
    ),
    (
        "grid-template-columns: [s] 10px repeat(auto-fill, minmax(5em, 1fr)) [e]",
        "grid-template-columns: [s] 10px repeat(auto-fill, minmax(5em, 1fr)) [e]",
    ),
    (
        "grid-template-columns: repeat(auto-fit, 20px 30px)",
        "grid-template-columns: repeat(auto-fit, 20px 30px)",
    ),
    (
        "grid-template-columns: repeat(99999, 1px)",
        "grid-template-columns: repeat(10000, 1px)",
    ),
    ("grid-template-columns: 0", "grid-template-columns: 0px"),
    ("grid-auto-rows: auto", "grid-auto-rows: auto"),
    (
        "grid-auto-columns: 1fr minmax(min-content, 10px)",
        "grid-auto-columns: 1fr minmax(min-content, 10px)",
    ),
    // Subgrids.
    ("grid-template-rows: subgrid", "grid-template-rows: subgrid"),
    (
        "grid-template-columns: SUBGRID [a] [] [b c]",
        "grid-template-columns: subgrid [a] [] [b c]",
    ),
    (
        "grid-template-columns: subgrid [a] repeat(auto-fill, [b] []) repeat(2, [c])",
        "grid-template-columns: subgrid [a] repeat(auto-fill, [b] []) repeat(2, [c])",
    ),
    (
        "grid-template: subgrid / subgrid [x]",
        "grid-template-rows: subgrid; grid-template-columns: subgrid [x]; grid-template-areas: none",
    ),
    // Areas.
    ("grid-template-areas: none", "grid-template-areas: none"),
    (
        "grid-template-areas: \"a  a b\" \". ... b\"",
        "grid-template-areas: \"a a b\" \". . b\"",
    ),
    (
        "grid-template-areas: \"1st 2nd\"",
        "grid-template-areas: \"1st 2nd\"",
    ),
    // Auto flow.
    ("grid-auto-flow: row", "grid-auto-flow: row"),
    ("grid-auto-flow: dense row", "grid-auto-flow: dense"),
    (
        "grid-auto-flow: dense column",
        "grid-auto-flow: column dense",
    ),
    // Lines.
    ("grid-row-start: auto", "grid-row-start: auto"),
    ("grid-row-start: -3", "grid-row-start: -3"),
    ("grid-column-end: a 2", "grid-column-end: 2 a"),
    ("grid-column-end: span 2", "grid-column-end: span 2"),
    ("grid-row-end: b span", "grid-row-end: span b"),
    ("grid-row-end: span 1 b", "grid-row-end: span b"),
    ("grid-row-end: 3 span", "grid-row-end: span 3"),
    // Line shorthands copy a bare name into the omitted end.
    ("grid-row: a", "grid-row-start: a; grid-row-end: a"),
    ("grid-row: 2", "grid-row-start: 2; grid-row-end: auto"),
    (
        "grid-column: 1 / span 2",
        "grid-column-start: 1; grid-column-end: span 2",
    ),
    (
        "grid-area: a",
        "grid-row-start: a; grid-column-start: a; grid-row-end: a; grid-column-end: a",
    ),
    (
        "grid-area: 1 / b",
        "grid-row-start: 1; grid-column-start: b; grid-row-end: auto; grid-column-end: b",
    ),
    (
        "grid-area: 1 / 2 / 3 / 4",
        "grid-row-start: 1; grid-column-start: 2; grid-row-end: 3; grid-column-end: 4",
    ),
    // grid-template.
    (
        "grid-template: none",
        "grid-template-rows: none; grid-template-columns: none; grid-template-areas: none",
    ),
    (
        "grid-template: 10px / 20px 1fr",
        "grid-template-rows: 10px; grid-template-columns: 20px 1fr; grid-template-areas: none",
    ),
    (
        "grid-template: [top] \"a b\" 30px [mid] [x] \"c d\" / 1fr 2fr",
        "grid-template-rows: [top] 30px [mid x] auto; grid-template-columns: 1fr 2fr; \
         grid-template-areas: \"a b\" \"c d\"",
    ),
    // grid.
    (
        "grid: 10px / auto-flow dense 20px",
        "grid-template-rows: 10px; grid-template-columns: none; grid-template-areas: none; \
         grid-auto-rows: auto; grid-auto-columns: 20px; grid-auto-flow: column dense",
    ),
    (
        "grid: auto-flow / 1fr 1fr",
        "grid-template-rows: none; grid-template-columns: 1fr 1fr; grid-template-areas: none; \
         grid-auto-rows: auto; grid-auto-columns: auto; grid-auto-flow: row",
    ),
    (
        "grid: 1fr / 2fr",
        "grid-template-rows: 1fr; grid-template-columns: 2fr; grid-template-areas: none; \
         grid-auto-rows: auto; grid-auto-columns: auto; grid-auto-flow: row",
    ),
    // Alignment.
    ("justify-items: legacy", "justify-items: legacy"),
    (
        "justify-items: center legacy",
        "justify-items: legacy center",
    ),
    ("justify-items: safe end", "justify-items: safe end"),
    ("justify-self: auto", "justify-self: auto"),
    ("justify-self: left", "justify-self: left"),
    ("justify-self: last baseline", "justify-self: last baseline"),
    (
        "place-items: center",
        "align-items: center; justify-items: center",
    ),
    (
        "place-self: end stretch",
        "align-self: end; justify-self: stretch",
    ),
    (
        "place-content: baseline",
        "align-content: baseline; justify-content: start",
    ),
    (
        "place-content: space-between center",
        "align-content: space-between; justify-content: center",
    ),
    // Legacy gap aliases.
    ("grid-gap: 1px 2px", "row-gap: 1px; column-gap: 2px"),
    ("grid-row-gap: 3px", "row-gap: 3px"),
];

const INVALID: &[&str] = &[
    "grid-template-columns: [a] [b] 10px",
    "grid-template-columns: [a]",
    "grid-template-columns: -1px",
    "grid-template-columns: -1fr",
    "grid-template-columns: minmax(1fr, 10px)",
    "grid-template-columns: repeat(0, 10px)",
    "grid-template-columns: repeat(auto-fill, 1fr)",
    "grid-template-columns: repeat(auto-fill, 10px) repeat(auto-fit, 10px)",
    "grid-template-columns: repeat(auto-fill, 10px) auto",
    "grid-template-columns: repeat(2, repeat(2, 10px))",
    "grid-template-columns: [span] 10px",
    "grid-template-columns: [auto] 10px",
    "grid-template-rows: subgrid subgrid",
    "grid-template-rows: subgrid none",
    "grid-template-rows: subgrid [a] 1px",
    "grid-template-rows: subgrid repeat(auto-fit, [a])",
    "grid-template-rows: subgrid repeat(2, 1px)",
    "grid-template-rows: subgrid repeat(0, [a])",
    "grid-template-rows: subgrid repeat(auto-fill, [a]) repeat(auto-fill, [b])",
    "grid-auto-rows: subgrid",
    "grid-template: \"a\" / subgrid",
    "grid-auto-rows: none",
    "grid-template-areas: \"a b\" \"a\"",
    "grid-template-areas: \"a b a\"",
    "grid-template-areas: \"a b\" \"b a\"",
    "grid-template-areas: \"a #\"",
    "grid-template-areas: \"\"",
    "grid-auto-flow: row column",
    "grid-auto-flow: dense dense",
    "grid-row-start: 0",
    "grid-row-start: span -1",
    "grid-row-start: span",
    "grid-row-start: 1 span a 2",
    "grid-row-start: 1 span a",
    "grid-row: 1 / 2 / 3",
    "grid-area: 1 / 2 / 3 / 4 / 5",
    "grid-template: 10px",
    "grid-template: \"a\" / repeat(auto-fill, 10px)",
    "grid: auto-flow / auto-flow",
    "justify-items: auto",
    "justify-items: legacy legacy",
    "justify-items: legacy start",
    "justify-self: legacy",
    "justify-content: self-start",
    "align-self: left",
    "place-items: legacy",
];

#[test]
fn grid_values_parse_and_serialize() {
    let mut failures = Vec::new();
    for (input, expected) in VALID {
        match parse(input) {
            Ok(text) if text == *expected => {}
            other => failures.push(format!("{input}: {other:?}")),
        }
    }
    for input in INVALID {
        if let Ok(text) = parse(input) {
            failures.push(format!("{input} was accepted as {text}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
