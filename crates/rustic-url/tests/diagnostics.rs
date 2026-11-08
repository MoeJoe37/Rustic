use rustic_url::{MAX_DIAGNOSTICS, ParseError, Url, UrlParser, ValidationError as V};

fn report(input: &str, base: Option<&str>) -> (Result<String, ParseError>, Vec<(V, usize)>) {
    let base = base.map(|base| Url::parse(base, None).unwrap());
    let report = UrlParser::default().parse_with_diagnostics(input, base.as_ref());
    assert_eq!(
        report.result(),
        Url::parse(input, base.as_ref())
            .as_ref()
            .map_err(|error| *error),
        "diagnostics must not change the result for {input:?}"
    );
    let diagnostics = report
        .diagnostics()
        .iter()
        .map(|diagnostic| (diagnostic.kind(), diagnostic.offset()))
        .collect();
    (report.result().map(Url::serialize), diagnostics)
}

fn diagnostics(input: &str) -> Vec<(V, usize)> {
    report(input, None).1
}

#[test]
fn valid_urls_report_nothing() {
    for input in [
        "https://example.test/a/b?c=d#e",
        "file:///C:/dir/file.txt",
        "file://server/share",
        "sc://opaque.host/x",
        "mailto:someone@example.test",
        "http://[::1]:8080/",
        "http://192.168.0.1/",
        "https://example.test/caf\u{e9}?q=\u{2603}#\u{1f600}",
    ] {
        assert_eq!(diagnostics(input), [], "{input}");
    }
}

#[test]
fn recoverable_errors_point_into_the_original_input() {
    assert_eq!(diagnostics("  https://h/"), [(V::InvalidUrlUnit, 0)]);
    assert_eq!(diagnostics("https://h/ \u{1}"), [(V::InvalidUrlUnit, 10)]);
    assert_eq!(
        diagnostics(" ht\ttps://h/a\nb"),
        [(V::InvalidUrlUnit, 0), (V::InvalidUrlUnit, 3)]
    );
    // Offsets after removed tabs/newlines still index the original string.
    assert_eq!(
        diagnostics("ht\ttps://h/^"),
        [(V::InvalidUrlUnit, 2), (V::InvalidUrlUnit, 11)]
    );
    assert_eq!(
        diagnostics("https:example.test"),
        [(V::SpecialSchemeMissingFollowingSolidus, 6)]
    );
    assert_eq!(
        diagnostics("https:///h"),
        [(V::SpecialSchemeMissingFollowingSolidus, 8)]
    );
    assert_eq!(
        diagnostics(r"https:\\h\a"),
        [
            (V::SpecialSchemeMissingFollowingSolidus, 6),
            (V::SpecialSchemeMissingFollowingSolidus, 6),
            (V::SpecialSchemeMissingFollowingSolidus, 7),
            (V::InvalidReverseSolidus, 9),
        ]
    );
    assert_eq!(diagnostics("https://u:p@h/"), [(V::InvalidCredentials, 11)]);
    assert_eq!(
        diagnostics("http://a@b@h/"),
        [(V::InvalidCredentials, 8), (V::InvalidCredentials, 10)]
    );
    assert_eq!(
        diagnostics("https://h/a|b?c`#d e%"),
        [
            (V::InvalidUrlUnit, 11),
            (V::InvalidUrlUnit, 15),
            (V::InvalidUrlUnit, 18),
            (V::InvalidUrlUnit, 20),
        ]
    );
    assert_eq!(diagnostics("https://h/%2g%41"), [(V::InvalidUrlUnit, 10)]);
    assert_eq!(diagnostics("sc:a b"), [(V::InvalidUrlUnit, 4)]);
    assert_eq!(diagnostics("sc://h{st/"), [(V::InvalidUrlUnit, 6)]);
    assert_eq!(diagnostics("sc:/a\\b"), [(V::InvalidUrlUnit, 5)]);
}

#[test]
fn relative_references_report_against_their_own_offsets() {
    let base = Some("https://h/dir/page");
    assert_eq!(
        report(r"\x", base),
        (
            Ok("https://h/x".into()),
            vec![(V::InvalidReverseSolidus, 0)]
        )
    );
    assert_eq!(
        report(r"\/\other", base).1,
        [
            (V::InvalidReverseSolidus, 0),
            (V::SpecialSchemeMissingFollowingSolidus, 2),
        ]
    );
    assert_eq!(
        report("https:x", base),
        (
            Ok("https://h/dir/x".into()),
            vec![(V::SpecialSchemeMissingFollowingSolidus, 6)]
        )
    );
    assert_eq!(
        report("x", Some("mailto:a")),
        (
            Err(ParseError::OpaqueBase),
            vec![(V::MissingSchemeNonRelativeUrl, 0)]
        )
    );
    assert_eq!(
        report("x", None),
        (
            Err(ParseError::MissingScheme),
            vec![(V::MissingSchemeNonRelativeUrl, 0)]
        )
    );
}

#[test]
fn failures_name_the_standard_error_and_location() {
    for (input, error, expected) in [
        (
            "https://u@/",
            ParseError::InvalidHost,
            vec![(V::InvalidCredentials, 9), (V::HostMissing, 10)],
        ),
        (
            "https:///",
            ParseError::InvalidHost,
            vec![
                (V::SpecialSchemeMissingFollowingSolidus, 8),
                (V::HostMissing, 9),
            ],
        ),
        (
            "sc://:1/",
            ParseError::InvalidHost,
            vec![(V::HostMissing, 5)],
        ),
        (
            "https://h:8a/",
            ParseError::InvalidPort,
            vec![(V::PortInvalid, 11)],
        ),
        (
            "https://h:65536/",
            ParseError::InvalidPort,
            vec![(V::PortOutOfRange, 10)],
        ),
        (
            "https://a^b/",
            ParseError::InvalidHost,
            vec![(V::DomainInvalidCodePoint, 8)],
        ),
        (
            "https://%ff/",
            ParseError::InvalidHost,
            vec![(V::DomainToAscii, 8)],
        ),
        (
            "https://a\u{200C}b/",
            ParseError::InvalidHost,
            vec![(V::DomainToAscii, 8)],
        ),
        (
            "https://\u{AD}/",
            ParseError::InvalidHost,
            vec![(V::DomainToAscii, 8)],
        ),
        (
            "https://a\u{FF0F}b/",
            ParseError::InvalidHost,
            vec![(V::DomainInvalidCodePoint, 8)],
        ),
        (
            "sc://a<b/",
            ParseError::InvalidHost,
            vec![(V::HostInvalidCodePoint, 5)],
        ),
        (
            "http://1.2.3.4.5/",
            ParseError::InvalidIpv4,
            vec![(V::Ipv4TooManyParts, 7)],
        ),
        (
            "http://1..2/",
            ParseError::InvalidIpv4,
            vec![(V::Ipv4NonNumericPart, 7)],
        ),
        (
            "http://1.2.3.256/",
            ParseError::InvalidIpv4,
            vec![(V::Ipv4OutOfRangePart, 7), (V::Ipv4OutOfRangePart, 7)],
        ),
        (
            "http://[::1/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6Unclosed, 7)],
        ),
        (
            "http://[::1]x/",
            ParseError::InvalidHost,
            vec![(V::Ipv6Unclosed, 7)],
        ),
        (
            "http://[:1]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6InvalidCompression, 7)],
        ),
        (
            "http://[1::2::3]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6MultipleCompression, 7)],
        ),
        (
            "http://[1:2:3:4:5:6:7:8:9]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6TooManyPieces, 7)],
        ),
        (
            "http://[1:2:3:4:5:6:7::8]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6TooManyPieces, 7)],
        ),
        (
            "http://[1:2]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6TooFewPieces, 7)],
        ),
        (
            "http://[1:2:]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6InvalidCodePoint, 7)],
        ),
        (
            "http://[12345::]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv6InvalidCodePoint, 7)],
        ),
        (
            "http://[1:2:3:4:5:6:7:1.2.3.4]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv4InIpv6TooManyPieces, 7)],
        ),
        (
            "http://[::1.2.3]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv4InIpv6TooFewParts, 7)],
        ),
        (
            "http://[::1.2.3.256]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv4InIpv6OutOfRangePart, 7)],
        ),
        (
            "http://[::01.2.3.4]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv4InIpv6InvalidCodePoint, 7)],
        ),
        (
            "http://[1.2.3.4::]/",
            ParseError::InvalidIpv6,
            vec![(V::Ipv4InIpv6InvalidCodePoint, 7)],
        ),
    ] {
        assert_eq!(report(input, None), (Err(error), expected), "{input}");
    }
}

#[test]
fn ipv4_shorthand_parses_with_recoverable_errors() {
    assert_eq!(
        report("http://0x7f.1./", None),
        (
            Ok("http://127.0.0.1/".into()),
            vec![(V::Ipv4EmptyPart, 7), (V::Ipv4NonDecimalPart, 7)]
        )
    );
    assert_eq!(
        report("http://1.256/", None),
        (
            Ok("http://1.0.1.0/".into()),
            vec![(V::Ipv4OutOfRangePart, 7)]
        )
    );
    assert_eq!(
        diagnostics("http://010.0.0.1/"),
        [(V::Ipv4NonDecimalPart, 7)]
    );
}

#[test]
fn file_urls_report_drive_letter_and_slash_errors() {
    assert_eq!(
        report("file://C|/x", None),
        (
            Ok("file:///C:/x".into()),
            vec![(V::FileInvalidWindowsDriveLetterHost, 7)]
        )
    );
    assert_eq!(
        report("file:/x", None),
        (
            Ok("file:///x".into()),
            vec![(V::SpecialSchemeMissingFollowingSolidus, 5)]
        )
    );
    assert_eq!(
        diagnostics(r"file:\\h\x"),
        [
            (V::SpecialSchemeMissingFollowingSolidus, 5),
            (V::InvalidReverseSolidus, 5),
            (V::InvalidReverseSolidus, 6),
            (V::InvalidReverseSolidus, 8),
        ]
    );
    let base = Some("file:///C:/dir/page");
    assert_eq!(
        report("D|/x", base),
        (
            Ok("file:///D:/x".into()),
            // `|` is not a URL code point when the path state reads it.
            vec![
                (V::FileInvalidWindowsDriveLetter, 0),
                (V::InvalidUrlUnit, 1)
            ]
        )
    );
    assert_eq!(report("other", base).1, []);
    assert_eq!(
        report("file://h:1/", None),
        (
            Err(ParseError::InvalidHost),
            vec![(V::DomainInvalidCodePoint, 7)]
        )
    );
}

#[test]
fn diagnostics_are_bounded_and_carry_standard_names() {
    let input = format!("https://h/{}", "^".repeat(MAX_DIAGNOSTICS + 10));
    let report = UrlParser::default().parse_with_diagnostics(&input, None);
    assert!(report.result().is_ok());
    assert_eq!(report.diagnostics().len(), MAX_DIAGNOSTICS);
    assert!(report.truncated());
    assert!(
        !UrlParser::default()
            .parse_with_diagnostics("https://h/", None)
            .truncated()
    );
    assert_eq!(V::InvalidUrlUnit.name(), "invalid-URL-unit");
    assert_eq!(
        V::FileInvalidWindowsDriveLetterHost.to_string(),
        "file-invalid-Windows-drive-letter-host"
    );
    assert_eq!(
        V::Ipv4InIpv6TooFewParts.name(),
        "IPv4-in-IPv6-too-few-parts"
    );
    let url = UrlParser::default()
        .parse_with_diagnostics("https://user:secret@h/", None)
        .into_result()
        .unwrap();
    assert_eq!(url.username(), "user");
}

#[test]
fn every_offset_is_a_char_boundary_of_the_original_input() {
    for input in [
        "\u{2003}https://h/\u{e9}^",
        "\t\u{1f600}:x",
        "https://h/\u{e9}\n\u{e9} \u{1}",
        "sc://\u{e9}h{/",
    ] {
        for (_, offset) in diagnostics(input) {
            assert!(input.is_char_boundary(offset), "{input:?} {offset}");
        }
    }
}
