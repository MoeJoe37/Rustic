//! Test262 parse-phase gate. The suite is not vendored: point
//! `RUSTIC_TEST262_DIR` at a checkout of the revision pinned in
//! `fixtures/test262_expected_failures.txt` and run
//! `cargo test --release -p rustic-js --test test262 -- --ignored`.
//! Every file outside the expected failures must pass, and every expected
//! failure must still fail, so the list never goes stale.

use std::collections::BTreeSet;
use std::path::PathBuf;

use rustic_js::ParseLimits;
use rustic_js::test262::run_tree;

const EXPECTED: &str = include_str!("fixtures/test262_expected_failures.txt");

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
