//! `@font-face` rules: descriptors, sources and which rules apply.

use rustic_css::{CssParser, MediaEnvironment};
use rustic_style::{
    Cascade, FontFaceSource, FontFaceStyle, Origin, StyleDiagnosticKind, StyleSheet,
};
use rustic_url::Url;

fn sheet(css: &str) -> (StyleSheet, Vec<StyleDiagnosticKind>) {
    let (mut sheet, diagnostics) =
        StyleSheet::parse(&CssParser::default(), css, Origin::Author).unwrap();
    sheet.set_base_url(Url::parse("https://example.com/css/site.css", None).ok());
    (sheet, diagnostics.into_iter().map(|d| d.kind).collect())
}

#[test]
fn font_face_descriptors_parse_into_ranges_and_resolved_sources() {
    let (sheet, diagnostics) = sheet(
        "@font-face {
            font-family: 'Open Sans';
            src: local(Open Sans Regular), url(fonts/a.woff2) format('woff2'),
                 url('b.svg') format('svg'), url(c.ttf) tech(color-COLRv1);
            font-weight: 300 700;
            font-style: oblique 10deg 20deg;
            font-stretch: condensed 125%;
            unicode-range: U+0000-00FF, U+4??, u+2026;
         }
         @font-face { font-family: Plain; src: url(p.otf) }",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let rule = &sheet.font_faces[0];
    assert_eq!(&*rule.family, "Open Sans");
    assert_eq!(rule.weight, (300.0, 700.0));
    assert_eq!(rule.style, FontFaceStyle::Oblique);
    assert_eq!(rule.stretch, (75.0, 125.0));
    assert_eq!(
        rule.unicode_range,
        vec![(0, 0xFF), (0x400, 0x4FF), (0x2026, 0x2026)]
    );
    assert!(rule.covers('\u{4AA}') && !rule.covers('\u{500}'));
    // Unsupported formats and technologies drop only their entry.
    assert_eq!(rule.sources.len(), 2);
    assert_eq!(
        rule.sources[0],
        FontFaceSource::Local("Open Sans Regular".into())
    );
    let FontFaceSource::Url { url: Some(url), .. } = &rule.sources[1] else {
        panic!("{:?}", rule.sources[1]);
    };
    assert_eq!(url.serialize(), "https://example.com/css/fonts/a.woff2");
    // Defaults: one weight, normal style and width, all of Unicode.
    let plain = &sheet.font_faces[1];
    assert_eq!(plain.weight, (400.0, 400.0));
    assert_eq!(plain.style, FontFaceStyle::Normal);
    assert_eq!(plain.stretch, (100.0, 100.0));
    assert_eq!(plain.unicode_range, vec![(0, 0x10_FFFF)]);
}

#[test]
fn invalid_font_face_rules_are_dropped_and_media_decides_which_apply() {
    let (sheet, diagnostics) = sheet(
        "@font-face { src: url(a.woff) }
         @font-face { font-family: A; src: url(a.woff) format(woff), bogus }
         @font-face { font-family: initial; src: url(a.woff) }
         @font-face { font-family: Screen; src: url(s.woff); font-weight: 2000 }
         @media print { @font-face { font-family: Print; src: url(p.woff) } }",
    );
    assert_eq!(
        diagnostics,
        vec![StyleDiagnosticKind::InvalidFontFaceRule; 3]
    );
    // An invalid descriptor keeps its initial value.
    assert_eq!(sheet.font_faces[0].weight, (400.0, 400.0));
    let mut cascade = Cascade::new();
    cascade.push(sheet);
    cascade.set_environment(MediaEnvironment::screen(800.0, 600.0));
    let families: Vec<&str> = cascade
        .font_faces()
        .iter()
        .map(|rule| &*rule.family)
        .collect();
    assert_eq!(families, ["Screen"]);
}
