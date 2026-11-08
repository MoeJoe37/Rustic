//! Pinned UTS #46, Unicode normalization and WPT toascii conformance data.
//! Regenerate with tools/generate_idna_data.py; see docs/URL.md.

use rustic_url::{Url, idna};

#[path = "fixtures/idna_cases.rs"]
mod idna_cases;
#[path = "fixtures/nfc_cases.rs"]
mod nfc_cases;
#[path = "fixtures/toascii_cases.rs"]
mod toascii_cases;

struct Case {
    line: usize,
    input: &'static str,
    /// Expected ToUnicode output, or `None` when an error must be recorded.
    to_unicode: Option<&'static str>,
    /// Expected ToASCII output, or `None` when ToASCII must fail.
    to_ascii: Option<&'static str>,
}

fn report(kind: &str, failures: &[String], total: usize) {
    assert!(
        failures.is_empty(),
        "{} of {total} {kind} cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn unicode_idna_test_v2_to_ascii() {
    let mut failures = Vec::new();
    for case in idna_cases::CASES {
        let actual = idna::to_ascii(case.input);
        match (&actual, case.to_ascii) {
            (Ok(actual), Some(expected)) if actual == expected => {}
            (Err(_), None) => {}
            _ => failures.push(format!(
                "line {}: {:?} -> {actual:?}, expected {:?}",
                case.line, case.input, case.to_ascii
            )),
        }
    }
    report("IdnaTestV2 toAsciiN", &failures, idna_cases::CASES.len());
}

#[test]
fn unicode_idna_test_v2_to_unicode() {
    let mut failures = Vec::new();
    for case in idna_cases::CASES {
        let (output, result) = idna::to_unicode(case.input);
        let matches = match case.to_unicode {
            Some(expected) => result.is_ok() && output == expected,
            None => result.is_err(),
        };
        if !matches {
            failures.push(format!(
                "line {}: {:?} -> ({output:?}, {result:?}), expected {:?}",
                case.line, case.input, case.to_unicode
            ));
        }
    }
    report("IdnaTestV2 toUnicode", &failures, idna_cases::CASES.len());
}

#[test]
fn unicode_normalization_test_nfc() {
    // NFC is internal, so it is observed through ToUnicode on single labels:
    // inputs whose code points are all IDNA-valid and stay valid map to
    // themselves, leaving only NFC to change them. Others are skipped here
    // and covered by the module's own unit tests.
    let mut checked = 0;
    let mut failures = Vec::new();
    let entries: Vec<&str> = nfc_cases::NFC_CASES.lines().collect();
    assert_eq!(entries.len(), nfc_cases::NFC_CASE_COUNT);
    for entry in entries {
        let (source, expected) = entry.split_once('\t').unwrap_or((entry, entry));
        if !idna_identity(source) || !idna_identity(expected) {
            continue;
        }
        checked += 1;
        let (output, _) = idna::to_unicode(source);
        if output != expected {
            failures.push(format!("{source:?} -> {output:?}, expected {expected:?}"));
        }
    }
    assert!(checked > 10_000, "only {checked} NFC cases were observable");
    report("NormalizationTest NFC", &failures, checked);
}

/// Whether every code point maps to itself in UTS #46 (status valid or deviation).
fn idna_identity(value: &str) -> bool {
    value.chars().all(|value| {
        let encoded = value.to_string();
        let (output, _) = idna::to_unicode(&encoded);
        output == encoded && value != '.'
    })
}

#[test]
fn wpt_toascii_through_url_parser() {
    let mut failures = Vec::new();
    for &(input, expected) in toascii_cases::TOASCII_CASES {
        let parsed = Url::parse(&format!("https://{input}/x"), None);
        let host = parsed
            .as_ref()
            .ok()
            .and_then(|url| url.host().map(|host| host.serialize()));
        let ok = match (expected, &parsed) {
            (Some(expected), Ok(url)) => {
                host.as_deref() == Some(expected)
                    && url.serialize() == format!("https://{expected}/x")
            }
            (None, Err(_)) => true,
            _ => false,
        };
        if !ok {
            failures.push(format!("{input:?} -> {host:?}, expected {expected:?}"));
        }
    }
    report("WPT toascii", &failures, toascii_cases::TOASCII_CASES.len());
}

#[test]
fn url_parser_uses_idna_for_special_hosts() {
    let url = Url::parse("https://B\u{FC}cher.example/", None).expect("IDNA host");
    assert_eq!(url.serialize(), "https://xn--bcher-kva.example/");
    // Percent-decoded UTF-8 goes through IDNA too.
    let url = Url::parse("https://b%C3%BCcher.example/", None).expect("IDNA host");
    assert_eq!(
        url.host().and_then(|host| host.domain()),
        Some("xn--bcher-kva.example")
    );
    // Full-width digits and dots map to an IPv4 address.
    let url = Url::parse("http://\u{FF11}\u{FF12}\u{FF17}\u{FF0E}0.0.1/", None).expect("IPv4");
    assert_eq!(
        url.host().and_then(|host| host.ipv4()),
        Some([127, 0, 0, 1])
    );
    // Mapping to a forbidden domain code point fails after ToASCII.
    assert!(Url::parse("https://a\u{FF0F}b/", None).is_err());
    assert!(Url::parse("https://\u{AD}/", None).is_err());
    assert!(Url::parse("https://a\u{200C}b/", None).is_err());
    // Non-special URLs keep opaque, percent-encoded hosts.
    let url = Url::parse("foo://b\u{FC}cher/", None).expect("opaque host");
    assert_eq!(url.serialize(), "foo://b%C3%BCcher/");
}
