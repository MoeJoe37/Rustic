use rustic_text::bidi::{BidiInfo, BidiScope, Level, ScopeStyle};
use rustic_text::hyphen::hyphenation_points;
#[test]
fn virtual_css_scopes_match_explicit_uax_controls() {
    for (style, open, close) in [
        (ScopeStyle::Embed, "\u{202B}", "\u{202C}"),
        (ScopeStyle::Override, "\u{202E}", "\u{202C}"),
        (ScopeStyle::Isolate, "\u{2067}", "\u{2069}"),
        (
            ScopeStyle::IsolateOverride,
            "\u{2067}\u{202E}",
            "\u{202C}\u{2069}",
        ),
        (ScopeStyle::Plaintext, "\u{2068}", "\u{2069}"),
    ] {
        let text: Vec<_> = "x אב 12 y".chars().collect();
        let scopes = [BidiScope {
            range: 2..7,
            rtl: true,
            style,
        }];
        let actual = BidiInfo::with_scopes(&text, Some(Level::LTR), &scopes).unwrap();
        let explicit: Vec<_> = format!("x {open}אב 12{close} y").chars().collect();
        let expected = BidiInfo::new(&explicit, Some(Level::LTR));
        let count = open.chars().count();
        let mut indexes = vec![0, 1];
        indexes.extend(2 + count..7 + count);
        indexes.extend(7 + count + close.chars().count()..explicit.len());
        assert_eq!(
            actual.levels(),
            indexes
                .iter()
                .map(|&i| expected.levels()[i])
                .collect::<Vec<_>>()
        );
        assert_eq!(actual.levels().len(), text.len());
    }
}
#[test]
fn scopes_reopen_across_paragraphs_and_nested_ranges_validate() {
    let text: Vec<_> = "abc\ndef".chars().collect();
    let scope = BidiScope {
        range: 0..7,
        rtl: true,
        style: ScopeStyle::Override,
    };
    let actual = BidiInfo::with_scopes(&text, Some(Level::LTR), &[scope]).unwrap();
    assert!(actual.levels()[0].is_rtl() && actual.levels()[4].is_rtl());
    let crossing = [
        BidiScope {
            range: 0..4,
            rtl: true,
            style: ScopeStyle::Embed,
        },
        BidiScope {
            range: 2..5,
            rtl: false,
            style: ScopeStyle::Isolate,
        },
    ];
    assert!(BidiInfo::with_scopes(&text, None, &crossing).is_err());
    assert!(
        BidiInfo::with_scopes(
            &text,
            None,
            &[BidiScope {
                range: 0..8,
                rtl: false,
                style: ScopeStyle::Embed
            }]
        )
        .is_err()
    );
    assert!(
        BidiInfo::with_scopes(
            &text,
            None,
            &vec![
                BidiScope {
                    range: 0..7,
                    rtl: true,
                    style: ScopeStyle::Isolate
                };
                126
            ]
        )
        .is_err()
    );
}
#[test]
fn english_patterns_are_language_gated_and_bounded() {
    assert_eq!(hyphenation_points("hyphenation", "en"), [2, 6]);
    assert_eq!(hyphenation_points("HYPHENATION", "en-US"), [2, 6]);
    assert!(hyphenation_points("hyphenation", "de").is_empty());
    assert!(hyphenation_points("hyphenation", "").is_empty());
    assert!(hyphenation_points(&"x".repeat(257), "en").is_empty());
    assert!(hyphenation_points("café", "en").is_empty());
}
