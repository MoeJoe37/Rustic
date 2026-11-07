use rustic_url::{ParseError, Url};

#[path = "fixtures/wpt_cases.rs"]
mod cases;

struct Case {
    index: usize,
    input: &'static str,
    base: Option<&'static str>,
    expected: Option<Expected>,
}

struct Expected {
    href: &'static str,
    protocol: &'static str,
    username: &'static str,
    password: &'static str,
    host: &'static str,
    hostname: &'static str,
    port: &'static str,
    pathname: &'static str,
    search: &'static str,
    hash: &'static str,
    origin: Option<&'static str>,
}

fn run(case: &Case) -> Result<Url, ParseError> {
    let base = case.base.map(|base| Url::parse(base, None)).transpose()?;
    Url::parse(case.input, base.as_ref())
}

fn compare(case: &Case) -> Vec<String> {
    let mut failures = Vec::new();
    match (run(case), &case.expected) {
        (Err(error), Some(_)) => failures.push(format!("unexpected error: {error}")),
        (Ok(url), None) => {
            failures.push(format!("accepted an expected failure: {}", url.serialize()))
        }
        (Err(_), None) => {}
        (Ok(url), Some(expected)) => {
            let hostname = url.host().map(|host| host.serialize()).unwrap_or_default();
            let port = url.port().map(|port| port.to_string()).unwrap_or_default();
            let host = if port.is_empty() {
                hostname.clone()
            } else {
                format!("{hostname}:{port}")
            };
            let search = url
                .query()
                .filter(|query| !query.is_empty())
                .map(|query| format!("?{query}"))
                .unwrap_or_default();
            let hash = url
                .fragment()
                .filter(|fragment| !fragment.is_empty())
                .map(|fragment| format!("#{fragment}"))
                .unwrap_or_default();
            for (field, actual, expected) in [
                ("href", url.serialize(), expected.href),
                ("protocol", format!("{}:", url.scheme()), expected.protocol),
                ("username", url.username().to_owned(), expected.username),
                ("password", url.password().to_owned(), expected.password),
                ("host", host, expected.host),
                ("hostname", hostname, expected.hostname),
                ("port", port, expected.port),
                ("pathname", url.pathname(), expected.pathname),
                ("search", search, expected.search),
                ("hash", hash, expected.hash),
            ] {
                if actual != expected {
                    failures.push(format!("{field}: expected {expected:?}, got {actual:?}"));
                }
            }
            if let Some(expected) = expected.origin {
                let actual = url.origin().serialize();
                if actual != expected {
                    failures.push(format!("origin: expected {expected:?}, got {actual:?}"));
                }
            }
            if Url::parse(&url.serialize(), None).as_ref() != Ok(&url) {
                failures.push("record changed after serialization/reparse".into());
            }
        }
    }
    failures
}

#[test]
fn pinned_upstream_url_subset_has_no_unexpected_failures() {
    assert_eq!(cases::CASES.len(), cases::SELECTED);
    assert_eq!(
        cases::CASES
            .iter()
            .filter(|case| case.expected.is_some())
            .count(),
        cases::POSITIVE
    );
    assert_eq!(cases::POSITIVE + cases::NEGATIVE, cases::SELECTED);
    let mut failed_cases = Vec::new();
    for case in cases::CASES {
        let failures = compare(case);
        if !failures.is_empty() {
            failed_cases.push(format!(
                "upstream array index {} input {:?}, base {:?}: {}",
                case.index,
                case.input,
                case.base,
                failures.join("; ")
            ));
        }
    }
    assert!(
        failed_cases.is_empty(),
        "{} / {} selected cases failed:\n{}",
        failed_cases.len(),
        cases::SELECTED,
        failed_cases.join("\n")
    );
    println!(
        "{} pinned cases passed: {} positive and {} negative",
        cases::SELECTED,
        cases::POSITIVE,
        cases::NEGATIVE
    );
}
