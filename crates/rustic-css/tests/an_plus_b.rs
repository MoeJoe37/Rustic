//! Pinned `An+B` corpus: SimonSapin/css-parsing-tests `An+B.json` (CC0-1.0).

mod common;

use common::{Json, parse_json};
use rustic_css::{CssParser, parse_an_plus_b};

/// Revision 203ce36bffd617db7f118c551e32794561fb273d; see the manifest.
const CORPUS: &str = include_str!("fixtures/css-parsing-tests/An+B.json");

#[test]
fn pinned_an_plus_b_corpus_matches() {
    let items = parse_json(CORPUS);
    let parser = CssParser::default();
    let mut failures = Vec::new();
    for (index, pair) in items.chunks(2).enumerate() {
        let Json::String(input) = &pair[0] else {
            panic!("case {index}: input is not a string");
        };
        let values = parser.component_values(input).unwrap().value;
        let actual = match parse_an_plus_b(&values) {
            Some((a, b)) => Json::Array(vec![Json::Number(a.into()), Json::Number(b.into())]),
            None => Json::Null,
        };
        if actual != pair[1] {
            failures.push(format!(
                "case {index} {input:?}: got {actual:?}, want {:?}",
                pair[1]
            ));
        }
    }
    assert!(items.len() / 2 > 100, "corpus unexpectedly small");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn an_plus_b_saturates_and_rejects_non_tokens() {
    let parser = CssParser::default();
    let parse = |s: &str| parse_an_plus_b(&parser.component_values(s).unwrap().value);
    assert_eq!(
        parse("99999999999n-99999999999"),
        Some((i32::MAX, i32::MIN))
    );
    assert_eq!(parse("n-99999999999999999999"), Some((1, i32::MIN)));
    assert_eq!(parse("(2n)"), None);
    assert_eq!(parse("2n+1 of a"), None);
    assert_eq!(parse("\\6e"), Some((1, 0)));
}
