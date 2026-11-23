//! The automation protocol's gate, codec and redaction (ADR 0072).

use rustic_devtools::authority::{Grant, Grants, METHODS, required_grant};
use rustic_devtools::json::{JsonError, MAX_DEPTH, Value};
use rustic_devtools::protocol::{ErrorCode, admit, event, response, valid_output};
use rustic_devtools::{base64, redact};

#[test]
fn json_round_trips_and_stays_on_one_line() {
    let text =
        r#"{"a":[1,-2.5,true,false,null],"b":"line\nbreak \u2028 \"q\" \u00e9 \ud83d\ude00"}"#;
    let value = Value::parse(text).unwrap();
    let line = value.to_json();
    assert!(!line.contains('\n') && !line.contains('\u{2028}'), "{line}");
    assert_eq!(Value::parse(&line).unwrap(), value);
    assert_eq!(
        value.get("b").and_then(Value::as_str),
        Some("line\nbreak \u{2028} \"q\" é 😀")
    );
}

#[test]
fn json_rejects_what_it_should() {
    for bad in [
        "",
        "{",
        "[1,]",
        "{\"a\":1,}",
        "01",
        "1.",
        "-",
        "\"\\x\"",
        "\"\u{1}\"",
        "nul",
        "[1] 2",
        "\"\\ud800\"",
        "\"\\udc00\"",
        "1e999",
        "{1:2}",
    ] {
        assert!(Value::parse(bad).is_err(), "{bad:?} should not parse");
    }
    assert_eq!(
        Value::parse(r#"{"a":1,"a":2}"#),
        Err(JsonError::DuplicateKey)
    );
    let deep = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
    assert_eq!(Value::parse(&deep), Err(JsonError::TooDeep));
    let ok = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
    assert!(Value::parse(&ok).is_ok());
}

#[test]
fn integers_are_checked() {
    assert_eq!(Value::parse("7").unwrap().as_u64(), Some(7));
    assert_eq!(Value::parse("7.5").unwrap().as_u64(), None);
    assert_eq!(Value::parse("-1").unwrap().as_u64(), None);
    assert_eq!(Value::parse("9007199254740992").unwrap().as_u64(), None);
}

#[test]
fn the_gate_admits_only_granted_known_methods() {
    let command = admit(r#"{"id":1,"method":"dom.document"}"#, Grants::DEFAULT).unwrap();
    assert_eq!((command.id, command.method.as_str()), (1, "dom.document"));
    assert_eq!(command.params, Value::Object(Vec::new()));
    let refused = admit(
        r#"{"id":2,"method":"runtime.evaluate","params":{"expression":"1"}}"#,
        Grants::DEFAULT,
    )
    .unwrap_err();
    assert_eq!((refused.id, refused.code), (Some(2), ErrorCode::NotGranted));
    let granted = Grants::DEFAULT.with(Grant::Evaluate);
    assert!(
        admit(
            r#"{"id":2,"method":"runtime.evaluate","params":{"expression":"1"}}"#,
            granted
        )
        .is_ok()
    );
    // Always-available methods work with no grants at all.
    assert!(admit(r#"{"id":3,"method":"session.capabilities"}"#, Grants::NONE).is_ok());
    assert_eq!(
        admit(r#"{"id":3,"method":"dom.document"}"#, Grants::NONE)
            .unwrap_err()
            .code,
        ErrorCode::NotGranted
    );
    for (line, code) in [
        ("not json", ErrorCode::Parse),
        ("[1]", ErrorCode::InvalidRequest),
        (r#"{"method":"page.info"}"#, ErrorCode::InvalidRequest),
        (
            r#"{"id":-1,"method":"page.info"}"#,
            ErrorCode::InvalidRequest,
        ),
        (r#"{"id":1}"#, ErrorCode::InvalidRequest),
        (
            r#"{"id":1,"method":"page.info","extra":1}"#,
            ErrorCode::InvalidRequest,
        ),
        (
            r#"{"id":1,"method":"page.info","params":[]}"#,
            ErrorCode::InvalidParams,
        ),
        (
            r#"{"id":1,"method":"browser.exec"}"#,
            ErrorCode::UnknownMethod,
        ),
    ] {
        assert_eq!(
            admit(line, Grants::DEFAULT).unwrap_err().code,
            code,
            "{line}"
        );
    }
    let long = format!(
        r#"{{"id":1,"method":"page.info","params":{{"x":"{}"}}}}"#,
        "a".repeat(1 << 20)
    );
    assert_eq!(
        admit(&long, Grants::DEFAULT).unwrap_err().code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn grants_parse_and_default_is_read_only() {
    assert_eq!(Grants::parse("none").unwrap(), Grants::NONE);
    let grants = Grants::parse("inspect,debug").unwrap();
    assert!(grants.contains(Grant::Inspect) && grants.contains(Grant::Debug));
    assert!(!grants.contains(Grant::Evaluate));
    assert!(Grants::parse("inspect,inspect").is_err());
    assert!(Grants::parse("root").is_err());
    assert!(Grants::parse("").is_err());
    for grant in [Grant::Network, Grant::Input, Grant::Evaluate, Grant::Debug] {
        assert!(!Grants::DEFAULT.contains(grant), "{grant:?} must be opt-in");
    }
    assert_eq!(Grants::from_bits(grants.bits()), grants);
    // Every method is named once and every grant guards something.
    for (index, (name, _)) in METHODS.iter().enumerate() {
        assert!(
            METHODS[index + 1..].iter().all(|(other, _)| other != name),
            "{name}"
        );
        assert!(required_grant(name).is_some());
    }
    for grant in Grant::ALL {
        assert!(METHODS.iter().any(|&(_, g)| g == Some(grant)), "{grant:?}");
    }
}

#[test]
fn output_lines_are_validated() {
    assert!(valid_output(&response(1, Value::Bool(true))));
    assert!(valid_output(&event("debugger.paused", Value::Null)));
    assert!(!valid_output("{\"id\":1}"));
    assert!(!valid_output("{\"id\":1,\"result\":1,\"error\":{}}"));
    assert!(!valid_output(
        "{\"id\":1,\"result\":1}\n{\"id\":2,\"result\":2}"
    ));
    assert!(!valid_output("[1]"));
    assert!(!valid_output("{\"event\":3}"));
}

#[test]
fn credentials_are_redacted() {
    for name in [
        "Authorization",
        "cookie",
        "Set-Cookie",
        "proxy-authorization",
    ] {
        assert!(redact::sensitive_header(name), "{name}");
        assert_eq!(redact::header_value(name, "secret"), redact::REDACTED);
    }
    assert_eq!(redact::header_value("content-type", "text/css"), "text/css");
    assert_eq!(
        redact::url("https://user:pw@example.test:8443/a@b?c=d@e#f"),
        "https://<redacted>@example.test:8443/a@b?c=d@e#f"
    );
    assert_eq!(
        redact::url("https://example.test/x"),
        "https://example.test/x"
    );
    assert_eq!(redact::url("file:///tmp/a"), "file:///tmp/a");
}

#[test]
fn base64_matches_rfc4648() {
    for (input, output) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64::encode(input.as_bytes()), output);
    }
}
