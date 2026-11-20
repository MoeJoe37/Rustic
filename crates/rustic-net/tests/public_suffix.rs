//! The Public Suffix List's own test vectors (`tests/tests.txt` at the
//! vendored revision) against the bundled list.

use rustic_net::site::PublicSuffixList;
use rustic_url::Url;

const CASES: &str = include_str!("fixtures/psl_tests.txt");

/// Domain-to-ASCII through the URL parser, as hosts reach the list.
fn ascii(domain: &str) -> Option<String> {
    if domain.starts_with('.') {
        return None;
    }
    let url = Url::parse(&format!("http://{domain}/"), None).ok()?;
    url.host()?.domain().map(str::to_owned)
}

#[test]
fn bundled_list_passes_the_upstream_test_vectors() {
    let list = PublicSuffixList::bundled();
    assert!(list.len() > 9000, "{}", list.len());
    let mut checked = 0;
    for line in CASES.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let (input, expected) = line.split_once(' ').expect("two fields");
        let expected = (expected != "null").then(|| ascii(expected).expect("valid expectation"));
        let actual = if input == "null" {
            None
        } else {
            ascii(input).and_then(|domain| list.registrable_domain(&domain).map(str::to_owned))
        };
        assert_eq!(actual, expected, "{input}");
        checked += 1;
    }
    assert_eq!(checked, 78);
}
