use std::collections::HashSet;

use rustic_base::CancellationToken;
use rustic_url::{Origin, ParseError, Url, UrlLimits, UrlParser, UrlPath};

fn parse(input: &str) -> Url {
    Url::parse(input, None).unwrap()
}

#[test]
fn absolute_special_urls_canonicalize_without_changing_encoded_components() {
    for (input, expected) in [
        (" HTTP://Example.COM:080 ", "http://example.com/"),
        ("https:example.test/a", "https://example.test/a"),
        ("ht\tt\r\nps://EXAMPLE.test:443/a", "https://example.test/a"),
        (
            r"https:\\example.test\one\two",
            "https://example.test/one/two",
        ),
        (
            "https://user:pa:ss@EXAMPLE.test:444/a?x=' y#x y`",
            "https://user:pa%3Ass@example.test:444/a?x=%27%20y#x%20y%60",
        ),
        ("http://a@b:c@host/", "http://a%40b:c@host/"),
        ("https://:@host:/", "https://host/"),
        ("wss://HOST:443/", "wss://host/"),
        ("ftp://host:21/", "ftp://host/"),
        (
            "https://%65xample.test/a/%2f?x=%zz#%",
            "https://example.test/a/%2f?x=%zz#%",
        ),
        ("https://host:0/", "https://host:0/"),
        ("https://host:65535/", "https://host:65535/"),
        ("https://host:0000000000000000000000443/", "https://host/"),
        (
            "https://host/مرحبا?q=é#😀",
            "https://host/%D9%85%D8%B1%D8%AD%D8%A8%D8%A7?q=%C3%A9#%F0%9F%98%80",
        ),
    ] {
        let url = parse(input);
        assert_eq!(url.serialize(), expected, "input {input:?}");
        assert_eq!(parse(expected), url, "roundtrip {input:?}");
    }
}

#[test]
fn relative_references_keep_base_immutable_and_normalize_dot_segments() {
    let base = parse("https://user:pass@host:444/a/b?old#fragment");
    for (input, expected) in [
        ("", "https://user:pass@host:444/a/b?old"),
        ("#", "https://user:pass@host:444/a/b?old#"),
        ("?", "https://user:pass@host:444/a/b?"),
        ("#new", "https://user:pass@host:444/a/b?old#new"),
        ("?new#new", "https://user:pass@host:444/a/b?new#new"),
        ("../c", "https://user:pass@host:444/c"),
        (".%2E/c", "https://user:pass@host:444/c"),
        ("/a/%2e%2e/d/.", "https://user:pass@host:444/d/"),
        ("../../../../../c", "https://user:pass@host:444/c"),
        ("//other:443/x", "https://other/x"),
        (r"\\other\x", "https://other/x"),
        ("https:c", "https://user:pass@host:444/a/c"),
        ("http:c", "http://c/"),
        ("/a//b/", "https://user:pass@host:444/a//b/"),
    ] {
        assert_eq!(
            Url::parse(input, Some(&base)).unwrap().serialize(),
            expected,
            "{input:?}"
        );
    }
    assert!(Url::parse("//host:65536", Some(&base)).is_err());
    assert_eq!(
        base.serialize(),
        "https://user:pass@host:444/a/b?old#fragment"
    );
}

#[test]
fn hierarchical_and_opaque_paths_do_not_share_resolution_rules() {
    for (input, expected) in [
        (
            "custom://Example.TEST:80/a/../b",
            "custom://Example.TEST:80/b",
        ),
        ("custom://host", "custom://host"),
        ("custom:///path", "custom:///path"),
        ("custom:/a/.././/b", "custom:/.//b"),
        ("data:text/plain,hello world", "data:text/plain,hello world"),
        (
            "data:text/plain,hello  ?x#f",
            "data:text/plain,hello %20?x#f",
        ),
        ("about:blank", "about:blank"),
        ("custom:", "custom:"),
        (
            "mailto:user@example.test?subject='test'",
            "mailto:user@example.test?subject='test'",
        ),
    ] {
        let url = parse(input);
        assert_eq!(url.serialize(), expected);
        assert_eq!(parse(expected), url);
    }
    let base = parse("data:text/plain,hello?query#old");
    assert!(matches!(base.path(), UrlPath::Opaque(_)));
    assert_eq!(
        Url::parse("#new", Some(&base)).unwrap().serialize(),
        "data:text/plain,hello?query#new"
    );
    for input in ["", "relative", "?new", "/path"] {
        assert_eq!(Url::parse(input, Some(&base)), Err(ParseError::OpaqueBase));
    }
    let base = parse("custom:/a/b");
    assert_eq!(
        Url::parse("../c", Some(&base)).unwrap().serialize(),
        "custom:/c"
    );
}

#[test]
fn ipv4_short_decimal_octal_and_hex_forms_share_one_canonical_host() {
    for input in [
        "127.0.0.1",
        "127.1",
        "2130706433",
        "0177.0.0.01",
        "0x7f000001",
        "0x7f.1",
        "127.0.0.1.",
    ] {
        let url = parse(&format!("http://{input}/"));
        assert_eq!(url.host().unwrap().ipv4(), Some([127, 0, 0, 1]), "{input}");
        assert_eq!(url.serialize(), "http://127.0.0.1/");
    }
    assert_eq!(parse("http://0x/").serialize(), "http://0.0.0.0/");
    for host in [
        "09",
        "1.2.3.256",
        "1.2.65536",
        "1.16777216",
        "4294967296",
        "1.2.3.4.5",
        "256.0.0.1",
        "example.42",
        "0xffffffffffffffffffffffffffffffff",
    ] {
        assert_eq!(
            Url::parse(&format!("http://{host}/"), None),
            Err(ParseError::InvalidIpv4),
            "{host}"
        );
    }
}

#[test]
fn ipv6_uses_first_longest_zero_run_and_hex_embedded_ipv4_serialization() {
    for (host, expected) in [
        ("[0:0:0:0:0:0:0:0]", "[::]"),
        ("[0:0:1:0:0:2:3:4]", "[::1:0:0:2:3:4]"),
        ("[0:1:0:0:0:2:3:4]", "[0:1::2:3:4]"),
        ("[1:2:3:4:5:0:0:0]", "[1:2:3:4:5::]"),
        ("[2001:0DB8::0001]", "[2001:db8::1]"),
        ("[::ffff:192.0.2.128]", "[::ffff:c000:280]"),
        ("[1:2:3:4:5:6:192.0.2.128]", "[1:2:3:4:5:6:c000:280]"),
    ] {
        let url = parse(&format!("https://{host}/"));
        assert_eq!(url.host().unwrap().serialize(), expected);
        assert!(url.host().unwrap().ipv6().is_some());
        assert_eq!(parse(&url.serialize()), url);
    }
    for host in [
        "[1]",
        "[:::]",
        "[1::2::3]",
        "[1:2:3:4:5:6:7:8:9]",
        "[1:2:3:4:5:6:7]",
        "[::ffff:192.00.2.1]",
        "[::ffff:256.0.0.1]",
        "[192.0.2.1::]",
        "[fe80::1%eth0]",
        "[::1",
    ] {
        assert_eq!(
            Url::parse(&format!("https://{host}/"), None),
            Err(ParseError::InvalidIpv6),
            "{host}"
        );
    }
}

#[test]
fn tuple_origins_ignore_credentials_paths_queries_and_fragments() {
    let origin = parse("HTTPS://user:secret@EXAMPLE.test:443/private?q=token#f").origin();
    assert_eq!(origin.serialize(), "https://example.test");
    assert_eq!(origin, parse("https://example.test/other").origin());
    for input in [
        "http://example.test",
        "https://example.test:444",
        "https://other.test",
        "wss://example.test",
        "https://example.test.",
    ] {
        assert!(!origin.is_same_origin(&parse(input).origin()), "{input}");
    }
    let mut origins = HashSet::new();
    origins.insert(origin.clone());
    origins.insert(parse("https://example.test/").origin());
    assert_eq!(origins.len(), 1);
    assert_eq!(
        parse("https://127.1/").origin(),
        parse("https://0x7f000001/").origin()
    );
}

#[test]
fn null_serialization_never_merges_opaque_origins() {
    let first = Origin::opaque();
    let retained = first.clone();
    let other = Origin::opaque();
    assert!(first.is_opaque());
    assert_eq!(first, retained);
    assert_ne!(first, other);
    assert_eq!(first.serialize(), other.serialize());
    let url = parse("about:blank");
    assert_ne!(url.origin(), url.origin());
    let mut keys = HashSet::new();
    keys.insert(first);
    keys.insert(retained);
    keys.insert(other);
    assert_eq!(keys.len(), 2);
}

#[test]
fn absent_and_empty_suffixes_remain_distinct() {
    let url = parse("https://host/?#");
    assert_eq!(url.query(), Some(""));
    assert_eq!(url.fragment(), Some(""));
    assert_eq!(url.serialize_without_fragment(), "https://host/?");
    assert_ne!(url, parse("https://host/"));
    assert_eq!(
        parse("https://u:p@host/a").serialize_without_credentials(),
        "https://host/a"
    );
}

#[test]
fn explicit_unsupported_features_and_invalid_hosts_fail_without_fallback() {
    assert_eq!(
        Url::parse("blob:https://host/id", None),
        Err(ParseError::UnsupportedBlob)
    );
    assert_eq!(
        Url::parse("https://مثال.test/", None),
        Err(ParseError::UnsupportedIdna)
    );
    assert_eq!(
        Url::parse("https://%C3%A9.test/", None),
        Err(ParseError::UnsupportedIdna)
    );
    for host in [
        "",
        "%00",
        "exa%23mple",
        "exa%2Fmple",
        "exa%3Ample",
        "%ff",
        "a%",
        "a b",
        "a^b",
        "a|b",
    ] {
        assert_eq!(
            Url::parse(&format!("https://{host}/"), None),
            Err(ParseError::InvalidHost),
            "{host}"
        );
    }
    for port in ["65536", "-1", "+80", "1.2", "0x80", "9999999999999999999"] {
        assert_eq!(
            Url::parse(&format!("https://host:{port}/"), None),
            Err(ParseError::InvalidPort)
        );
    }
    assert_eq!(Url::parse("relative", None), Err(ParseError::MissingScheme));
}

#[test]
fn input_output_segments_and_cancellation_have_exact_boundaries() {
    let input = "https://host/a";
    let limits = UrlLimits {
        max_input_bytes: input.len(),
        max_serialized_bytes: input.len(),
        max_path_segments: 1,
    };
    let parser = |limits| UrlParser::new(limits, CancellationToken::default());
    assert_eq!(
        parser(limits).parse(input, None).unwrap().serialize(),
        input
    );
    assert_eq!(
        parser(UrlLimits {
            max_input_bytes: input.len() - 1,
            ..limits
        })
        .parse(input, None),
        Err(ParseError::LimitExceeded)
    );
    assert_eq!(
        parser(UrlLimits {
            max_serialized_bytes: input.len() - 1,
            ..limits
        })
        .parse(input, None),
        Err(ParseError::LimitExceeded)
    );
    assert_eq!(
        parser(UrlLimits {
            max_path_segments: 0,
            ..limits
        })
        .parse(input, None),
        Err(ParseError::LimitExceeded)
    );
    let base = parse("https://host/a/b");
    assert_eq!(
        parser(limits).parse("x", Some(&base)),
        Err(ParseError::LimitExceeded)
    );
    assert_eq!(base.serialize(), "https://host/a/b");
    let token = CancellationToken::default();
    let cancelled = UrlParser::new(UrlLimits::default(), token.clone());
    token.cancel();
    assert_eq!(cancelled.parse(input, None), Err(ParseError::Cancelled));
    assert!(Url::parse(input, None).is_ok());
}

#[test]
fn debug_and_errors_omit_credentials_and_private_payload() {
    let url = parse("https://username:password@host/private?q=secret#fragment");
    let debug = format!("{url:?}");
    for sensitive in ["username", "password", "private", "secret", "fragment"] {
        assert!(!debug.contains(sensitive));
    }
    assert!(
        !Url::parse("https://user:secret@host:bad/private", None)
            .unwrap_err()
            .to_string()
            .contains("secret")
    );
}

#[test]
fn file_urls_follow_drive_letter_host_and_localhost_rules() {
    for (input, expected) in [
        ("file:///C|/a/b", "file:///C:/a/b"),
        ("FILE://LOCALHOST/tmp/x", "file:///tmp/x"),
        ("file://server/share/doc", "file://server/share/doc"),
        ("file://C|/x", "file:///C:/x"),
        (r"file:\\server\share", "file://server/share"),
        ("file:///C:/../..", "file:///C:/"),
        ("file:///C:/a/%2e%2E/b", "file:///C:/b"),
        ("file:/x", "file:///x"),
        ("file:x", "file:///x"),
        ("file:", "file:///"),
        ("file:////x", "file:////x"),
        ("file:///a/C|/b", "file:///a/C|/b"),
        ("file://example.net/C:/", "file://example.net/C:/"),
        ("file://1.2.3.4/", "file://1.2.3.4/"),
        ("file://[::1]/a?b#c", "file://[::1]/a?b#c"),
    ] {
        let url = parse(input);
        assert_eq!(url.serialize(), expected, "{input}");
        assert_eq!(url.scheme(), "file");
        assert!(url.origin().is_opaque(), "{input}");
        assert_eq!(Url::parse(&url.serialize(), None).as_ref(), Ok(&url));
    }
    assert_eq!(
        parse("file:///x").host().map(|host| host.serialize()),
        Some(String::new())
    );
    for host in ["exa mple", "a^b", "%zz%"] {
        assert!(
            Url::parse(&format!("file://{host}/"), None).is_err(),
            "{host}"
        );
    }
    assert_eq!(
        Url::parse("file://h:80/", None),
        Err(ParseError::InvalidHost)
    );
}

#[test]
fn file_references_resolve_against_file_bases_only() {
    let base = parse("file:///C:/dir/page.html?q#f");
    for (input, expected) in [
        ("", "file:///C:/dir/page.html?q"),
        ("#x", "file:///C:/dir/page.html?q#x"),
        ("?y", "file:///C:/dir/page.html?y"),
        ("other", "file:///C:/dir/other"),
        ("../../../up", "file:///C:/up"),
        ("/abs", "file:///C:/abs"),
        ("D|/x", "file:///D:/x"),
        ("/D:/x", "file:///D:/x"),
        ("//host/x", "file://host/x"),
        ("file:rel", "file:///C:/dir/rel"),
        (r"\\\\x", "file:////x"),
    ] {
        assert_eq!(
            Url::parse(input, Some(&base)).unwrap().serialize(),
            expected,
            "{input:?}"
        );
    }
    let unc = parse("file://server/share/a");
    assert_eq!(
        Url::parse("b", Some(&unc)).unwrap().serialize(),
        "file://server/share/b"
    );
    assert_eq!(
        Url::parse("/b", Some(&unc)).unwrap().serialize(),
        "file://server/b"
    );
    let http = parse("https://host/a");
    assert_eq!(
        Url::parse("file:x", Some(&http)).unwrap().serialize(),
        "file:///x"
    );
    assert_eq!(base.serialize(), "file:///C:/dir/page.html?q#f");
}
