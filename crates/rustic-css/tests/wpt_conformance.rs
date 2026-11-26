//! The declared CSS conformance gate (`css.conformance`): WPT parsing cases
//! for selectors and supported properties, extracted at a pinned revision by
//! `tools/import_wpt_css_tests.py`, compared against a reviewed expectation
//! list.
//!
//! A case is in scope when it is a selector case, or a value case for a
//! property `rustic-css` supports, from a file that is not `.tentative`
//! (tentative files test proposals no specification requires yet). In-scope cases must pass unless listed in
//! `expected_failures.json` with a reason; a listed case that now passes
//! fails the gate too, so the list only shrinks deliberately (a ratchet).
//!
//! What a case checks:
//! * `valid-selector`: parses, and serializes to the expected text.
//! * `invalid-selector`: is rejected.
//! * `valid-value`: is accepted; for a longhand the declared value also
//!   serializes to the expected text. Shorthand serialization (CSSOM's
//!   "serialize a CSS value" from longhands) is not implemented, so a
//!   shorthand case checks acceptance only.
//! * `invalid-value`: is rejected.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use common::{Json, parse_json};
use rustic_css::{CssParser, NamespaceMap, PropertyError, PropertyName, parse_declaration};

const CASES: &str = include_str!("fixtures/wpt-css/cases.json");
const EXPECTED_FAILURES: &str = include_str!("fixtures/wpt-css/expected_failures.json");

/// The gate's denominator, pinned. Scope follows property support, so adding
/// or removing a supported property moves cases in or out of scope; this
/// makes every such change deliberate and visible instead of silently
/// growing or shrinking the denominator (docs/TECH_DEBT.md, TD-35).
const IN_SCOPE: usize = 2750;
const OUT_OF_SCOPE: usize = 1349;

fn string(json: &Json) -> &str {
    match json {
        Json::String(s) => s,
        other => panic!("expected a string, got {other:?}"),
    }
}

fn strings(json: &Json) -> Vec<&str> {
    match json {
        Json::Array(items) => items.iter().map(string).collect(),
        other => panic!("expected an array, got {other:?}"),
    }
}

struct Case<'a> {
    kind: &'a str,
    file: &'a str,
    property: &'a str,
    input: &'a str,
    expected: Vec<&'a str>,
}

impl Case<'_> {
    fn key(&self) -> (String, String, String) {
        (
            self.kind.to_owned(),
            self.property.to_owned(),
            self.input.to_owned(),
        )
    }
}

fn cases() -> Vec<Case<'static>> {
    // Leak once so cases can borrow from the parsed JSON for the test's life.
    let parsed: &'static [Json] = Box::leak(parse_json(CASES).into_boxed_slice());
    parsed
        .iter()
        .map(|row| match row {
            Json::Array(fields) if fields.len() == 5 => Case {
                kind: string(&fields[0]),
                file: string(&fields[1]),
                property: string(&fields[2]),
                input: string(&fields[3]),
                expected: strings(&fields[4]),
            },
            other => panic!("malformed case {other:?}"),
        })
        .collect()
}

/// What happened to one in-scope case.
enum Outcome {
    Pass,
    Fail(String),
}

/// `None` when the property is not supported (out of scope).
fn value_outcome(case: &Case<'_>) -> Option<Outcome> {
    let parser = CssParser::default();
    let source = format!("{}: {}", case.property, case.input);
    let declaration = parser
        .declaration(&source)
        .expect("within limits")
        .value
        .filter(|d| d.name.eq_ignore_ascii_case(case.property));
    // Support is decided by the property alone, independent of the value.
    let probe = parser
        .declaration(&format!("{}: initial", case.property))
        .expect("within limits")
        .value?;
    if parse_declaration(&probe) == Err(PropertyError::UnknownProperty) {
        return None;
    }
    let result = declaration
        .ok_or(PropertyError::InvalidValue)
        .and_then(|d| {
            // `element.style` rejects `!important` in a value.
            if d.important {
                Err(PropertyError::InvalidValue)
            } else {
                parse_declaration(&d)
            }
        });
    Some(match (case.kind, result) {
        ("invalid-value", Err(_)) => Outcome::Pass,
        ("invalid-value", Ok(longhands)) => Outcome::Fail(format!(
            "accepted as {}",
            longhands
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )),
        ("valid-value", Err(e)) => Outcome::Fail(format!("rejected: {e}")),
        ("valid-value", Ok(longhands)) => {
            let longhand = match longhands.as_slice() {
                [only] => match &only.property {
                    PropertyName::Longhand(id) if id.name().eq_ignore_ascii_case(case.property) => {
                        Some(only)
                    }
                    PropertyName::Custom(_) => Some(only),
                    _ => None,
                },
                _ => None,
            };
            match longhand {
                // A shorthand: acceptance is all this gate checks.
                None => Outcome::Pass,
                Some(declaration) => {
                    let text = declaration.value.to_string();
                    let ok = if case.expected.is_empty() {
                        text == case.input
                    } else {
                        case.expected.contains(&text.as_str())
                    };
                    if ok {
                        Outcome::Pass
                    } else {
                        Outcome::Fail(format!("serialized as {text:?}"))
                    }
                }
            }
        }
        (kind, _) => panic!("unexpected kind {kind}"),
    })
}

fn selector_outcome(case: &Case<'_>) -> Outcome {
    let parsed = CssParser::default()
        .selector_list(case.input, &NamespaceMap::new())
        .map(|p| p.value);
    match (case.kind, parsed) {
        (_, Err(limit)) => Outcome::Fail(format!("resource limit: {limit}")),
        ("invalid-selector", Ok(Err(_))) => Outcome::Pass,
        ("invalid-selector", Ok(Ok(list))) => Outcome::Fail(format!("accepted as {list}")),
        ("valid-selector", Ok(Err(e))) => Outcome::Fail(format!("rejected: {:?}", e.kind)),
        ("valid-selector", Ok(Ok(list))) => {
            let text = list.to_string();
            let ok = if case.expected.is_empty() {
                text == case.input
            } else {
                case.expected.contains(&text.as_str())
            };
            if ok {
                Outcome::Pass
            } else {
                Outcome::Fail(format!("serialized as {text:?}"))
            }
        }
        (kind, _) => panic!("unexpected kind {kind}"),
    }
}

/// `(kind, property, input) -> reason` from the expectation list.
fn expected_failures() -> BTreeMap<(String, String, String), String> {
    let mut out = BTreeMap::new();
    for row in parse_json(EXPECTED_FAILURES) {
        let fields = strings(&row);
        let [kind, property, input, reason] = fields.as_slice() else {
            panic!("malformed expectation {row:?}");
        };
        assert!(!reason.is_empty(), "expectation without a reason: {row:?}");
        let key = (
            (*kind).to_owned(),
            (*property).to_owned(),
            (*input).to_owned(),
        );
        assert!(
            out.insert(key, (*reason).to_owned()).is_none(),
            "duplicate expectation {row:?}"
        );
    }
    out
}

#[test]
fn wpt_css_parsing_cases_match_the_declared_expectations() {
    let expected = expected_failures();
    let mut seen = BTreeSet::new();
    let mut unexpected = Vec::new();
    let mut passing_now = BTreeSet::new();
    let mut failed = BTreeSet::new();
    let (mut in_scope, mut passed, mut out_of_scope) = (0usize, 0usize, 0usize);
    let mut per_kind: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for case in cases() {
        // `.tentative` files test proposals that no specification requires
        // yet; they are imported for visibility but never gate.
        if case.file.contains(".tentative.") {
            out_of_scope += 1;
            continue;
        }
        let outcome = if case.kind.ends_with("selector") {
            selector_outcome(&case)
        } else {
            match value_outcome(&case) {
                Some(outcome) => outcome,
                None => {
                    out_of_scope += 1;
                    continue;
                }
            }
        };
        in_scope += 1;
        let key = case.key();
        let listed = expected.contains_key(&key);
        seen.insert(key.clone());
        let entry = per_kind.entry(case.kind).or_default();
        entry.1 += 1;
        match outcome {
            Outcome::Pass => {
                passed += 1;
                entry.0 += 1;
                if listed {
                    passing_now.insert(key);
                }
            }
            Outcome::Fail(why) if !listed => unexpected.push(format!(
                "{} {} {:?} {:?}: {why}",
                case.kind, case.file, case.property, case.input
            )),
            Outcome::Fail(_) => {
                failed.insert(key);
            }
        }
    }
    // The same input can appear in several files: a listed key that passes
    // in any of them is reported as passing now.
    let stale: Vec<_> = expected
        .keys()
        .filter(|k| !seen.contains(*k))
        .map(|k| format!("{k:?}"))
        .collect();
    let mut report =
        format!("{passed}/{in_scope} in-scope WPT cases pass ({out_of_scope} out of scope); ");
    for (kind, (ok, total)) in &per_kind {
        let _ = write!(report, "{kind} {ok}/{total} ");
    }
    println!("{report}");
    assert!(
        unexpected.is_empty(),
        "{} unexpected failures (fix them or list them with a reason):\n{}",
        unexpected.len(),
        unexpected.join("\n")
    );
    assert!(
        passing_now.is_empty(),
        "listed failures that now pass (remove them from expected_failures.json):\n{:?}",
        passing_now
    );
    assert!(
        stale.is_empty(),
        "expectations that match no in-scope case:\n{}",
        stale.join("\n")
    );
    assert_eq!(expected.len(), failed.len(), "every listed key failed");
    assert_eq!(
        (in_scope, out_of_scope),
        (IN_SCOPE, OUT_OF_SCOPE),
        "the in-scope/out-of-scope split changed: update IN_SCOPE, OUT_OF_SCOPE and \
         the counts in docs/CSS.md together, and say why in the commit"
    );
}
