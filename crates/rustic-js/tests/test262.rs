//! Test262 gates: the parse phase (JS-01) and execution (JS-07). The suite is not vendored: point
//! `RUSTIC_TEST262_DIR` at a checkout of the revision pinned in
//! `fixtures/test262_expected_failures.txt` and run
//! `cargo test --release -p rustic-js --test test262 -- --ignored`.
//! Every file outside the expected failures must pass, and every expected
//! failure must still fail, so the list never goes stale.

use std::collections::BTreeSet;
use std::path::PathBuf;

use rustic_js::ParseLimits;
use rustic_js::test262::{RunLimits, execute_tree, run_tree};

const EXPECTED: &str = include_str!("fixtures/test262_expected_failures.txt");
/// The `test/language` files that pass when executed, without that prefix.
const PASSING: &str = include_str!("fixtures/test262_language_passing.txt");

fn pinned_revision() -> &'static str {
    EXPECTED
        .lines()
        .find_map(|line| line.strip_prefix("# Revision: "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("expected failures name a revision")
}

#[test]
fn expected_failures_are_sorted_and_unique() {
    let paths: Vec<&str> = EXPECTED.lines().filter(|l| !l.starts_with('#')).collect();
    assert!(paths.windows(2).all(|w| w[0] < w[1]));
    assert!(paths.iter().all(|p| p.starts_with("test/language/")));
    assert_eq!(pinned_revision().len(), 40);
}

#[test]
#[ignore = "needs a Test262 checkout in RUSTIC_TEST262_DIR"]
fn test262_parse_phase_matches_expected_failures() {
    let root = PathBuf::from(
        std::env::var("RUSTIC_TEST262_DIR").expect("RUSTIC_TEST262_DIR names a Test262 checkout"),
    );
    // A detached checkout names its commit; a branch checkout is trusted.
    if let Ok(head) = std::fs::read_to_string(root.join(".git/HEAD")) {
        let head = head.trim();
        if !head.starts_with("ref:") {
            assert_eq!(
                head,
                pinned_revision(),
                "checkout is not at the pinned revision"
            );
        }
    }
    let report = run_tree(&root, &ParseLimits::default()).expect("Test262 tree is readable");
    let expected: BTreeSet<&str> = EXPECTED.lines().filter(|l| !l.starts_with('#')).collect();
    let actual: BTreeSet<&str> = report.failures.iter().map(|(p, _)| p.as_str()).collect();
    let new: Vec<String> = report
        .failures
        .iter()
        .filter(|(path, _)| !expected.contains(path.as_str()))
        .map(|(path, reason)| format!("{path}: {reason}"))
        .collect();
    let fixed: Vec<&&str> = expected.difference(&actual).collect();
    assert!(new.is_empty(), "new failures:\n{}", new.join("\n"));
    assert!(
        fixed.is_empty(),
        "now passing, remove from the list: {fixed:?}"
    );
    assert!(
        report.passed > 22_000,
        "only {} files passed",
        report.passed
    );
}

#[test]
fn passing_list_is_sorted_and_names_the_same_revision() {
    let paths: Vec<&str> = PASSING.lines().filter(|l| !l.starts_with('#')).collect();
    assert!(paths.windows(2).all(|w| w[0] < w[1]));
    assert!(paths.iter().all(|p| p.ends_with(".js")));
    let revision = PASSING
        .lines()
        .find_map(|line| line.strip_prefix("# Revision: "))
        .and_then(|rest| rest.split_whitespace().next());
    assert_eq!(revision, Some(pinned_revision()));
}

/// Executes `test/language` and compares the files that pass with the
/// committed list, exactly: a regression and an unrecorded improvement
/// both fail. `RUSTIC_TEST262_BLESS=1` rewrites the list instead.
#[test]
#[ignore = "needs a Test262 checkout in RUSTIC_TEST262_DIR"]
fn test262_language_execution_matches_the_passing_list() {
    let root = PathBuf::from(
        std::env::var("RUSTIC_TEST262_DIR").expect("RUSTIC_TEST262_DIR names a Test262 checkout"),
    );
    let report = execute_tree(&root, "test/language", RunLimits::default())
        .expect("Test262 tree is readable");
    let actual: BTreeSet<&str> = report
        .passed
        .iter()
        .map(|p| p.strip_prefix("test/language/").expect("inside the tree"))
        .collect();
    if std::env::var_os("RUSTIC_TEST262_BLESS").is_some() {
        let mut text = format!(
            "# Test262 test/language files that pass when executed (JS-07).\n\
             # Revision: {} of https://github.com/tc39/test262\n\
             # {} pass, {} fail, {} skipped (modules, async, proposals).\n\
             # Regenerate with RUSTIC_TEST262_BLESS=1; see docs/JS.md.\n",
            pinned_revision(),
            report.passed.len(),
            report.failed.len(),
            report.skipped.len()
        );
        for path in &actual {
            text.push_str(path);
            text.push('\n');
        }
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/test262_language_passing.txt");
        std::fs::write(fixture, text).expect("the fixture is writable");
        return;
    }
    let expected: BTreeSet<&str> = PASSING.lines().filter(|l| !l.starts_with('#')).collect();
    let regressed: Vec<String> = report
        .failed
        .iter()
        .filter_map(|(path, reason)| {
            let short = path.strip_prefix("test/language/")?;
            expected
                .contains(short)
                .then(|| format!("{short}: {reason}"))
        })
        .collect();
    let missing: Vec<&&str> = expected
        .difference(&actual)
        .filter(|p| !regressed.iter().any(|r| r.starts_with(**p)))
        .collect();
    let improved: Vec<&&str> = actual.difference(&expected).collect();
    assert!(
        regressed.is_empty(),
        "regressions:\n{}",
        regressed.join("\n")
    );
    assert!(missing.is_empty(), "listed but not run: {missing:?}");
    assert!(
        improved.is_empty(),
        "{} files now pass; bless the list (first: {:?})",
        improved.len(),
        improved.iter().take(10).collect::<Vec<_>>()
    );
}
