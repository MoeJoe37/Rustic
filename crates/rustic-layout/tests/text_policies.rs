//! CSS policies measured in real line geometry and positioned glyphs.
use rustic_base::CancellationToken;
use rustic_layout::{BlockLayout, BoxTree, CssSize, FlowError, LayoutLimits};
use rustic_style::ComputeContext;
use rustic_text::{FontDatabase, FontLimits};
use std::sync::Arc;
mod common;
fn with_fonts(
    html: &str,
    css: &str,
    width: f64,
    fonts: FontDatabase,
) -> Result<BlockLayout, FlowError> {
    let context = ComputeContext::with_fonts(Arc::new(rustic_text::bundled::test_database()));
    let (doc, styles) =
        common::page_with_context(html, &format!("*{{margin:0;font-size:10px}}{css}"), context);
    let tree = BoxTree::build(
        &doc,
        doc.root(),
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    BlockLayout::compute_with_fonts(
        &tree,
        CssSize::new(width, 400.0).unwrap(),
        LayoutLimits::default(),
        &fonts,
        &CancellationToken::default(),
    )
}
fn layout(html: &str, css: &str, width: f64) -> BlockLayout {
    with_fonts(html, css, width, rustic_text::bundled::test_database()).unwrap()
}
#[test]
fn break_all_keep_all_and_anywhere_use_grapheme_boundaries() {
    assert_eq!(
        layout("<p>xxxx", "p{word-break:break-all}", 10.0)
            .lines()
            .len(),
        2
    );
    assert_eq!(layout("<p>xxxx", "", 10.0).lines().len(), 1);
    assert_eq!(
        layout("<p>水水水", "p{word-break:keep-all}", 5.0)
            .lines()
            .len(),
        1
    );
    assert_eq!(
        layout("<p>x\u{A0}x", "p{line-break:anywhere}", 5.0)
            .lines()
            .len(),
        3
    );
    let l = layout("<p>x\u{301}xx", "p{word-break:break-all}", 5.0);
    assert_eq!(l.texts()[0].text(), 0..2);
    assert_eq!(
        layout("<p>xxxx", "p{white-space:nowrap;line-break:anywhere}", 5.0)
            .lines()
            .len(),
        1
    );
}
#[test]
fn emergency_wraps_do_not_change_min_content_but_anywhere_does() {
    let width = |policy: &str| {
        layout(
            "<p>xxxx",
            &format!("p{{width:min-content;{policy}}}"),
            100.0,
        )
        .fragments()[2]
            .content()
            .size()
            .width()
            .get()
    };
    assert_eq!(width("overflow-wrap:break-word"), 20.0);
    assert_eq!(width("overflow-wrap:anywhere"), 5.0);
    assert_eq!(width("word-break:break-word"), 5.0);
    assert_eq!(
        layout("<p>xxxx", "p{overflow-wrap:break-word}", 10.0)
            .lines()
            .len(),
        2
    );
    let l = layout("<p>xx xxxx", "p{overflow-wrap:break-word}", 20.0);
    assert_eq!(l.texts()[0].text(), 0..2);
}
#[test]
fn cjk_punctuation_obeys_strictness_and_declared_language() {
    assert_eq!(
        layout("<p lang=ja>水ぁ水", "p{line-break:loose}", 5.0)
            .lines()
            .len(),
        3
    );
    assert_eq!(
        layout("<p lang=ja>水ぁ水", "p{line-break:normal}", 5.0)
            .lines()
            .len(),
        2
    );
    assert_eq!(
        layout("<p lang=ja>水〜水", "p{line-break:normal}", 5.0)
            .lines()
            .len(),
        3
    );
    assert_eq!(
        layout("<p lang=ja>水〜水", "p{line-break:strict}", 5.0)
            .lines()
            .len(),
        2
    );
    assert_eq!(
        layout("<p>水〜水", "p{line-break:normal}", 5.0)
            .lines()
            .len(),
        2
    );
}
#[test]
fn manual_hyphens_are_measured_and_drawn_only_at_selected_breaks() {
    let narrow = layout("<p>xx&shy;xx", "", 16.0);
    assert_eq!(narrow.lines().len(), 2);
    let glyphs: usize = narrow.texts().iter().map(|t| t.glyphs().len()).sum();
    assert_eq!(glyphs, 5);
    assert_eq!(
        narrow
            .texts()
            .iter()
            .filter(|t| t.line() == 0)
            .map(|t| t.rect().size().width().get())
            .sum::<f64>(),
        15.0
    );
    let wide = layout("<p>xx&shy;xx", "", 100.0);
    assert_eq!(
        wide.texts().iter().map(|t| t.glyphs().len()).sum::<usize>(),
        4
    );
    assert_eq!(
        layout("<p>xx&shy;xx", "p{hyphens:none}", 10.0)
            .lines()
            .len(),
        1
    );
    assert_eq!(
        layout("<p>xx&shy;xx", "p{word-break:break-all}", 10.0)
            .texts()
            .iter()
            .map(|t| t.glyphs().len())
            .sum::<usize>(),
        4
    );
}
#[test]
fn automatic_hyphenation_requires_known_language_and_respects_explicit_hints() {
    assert_eq!(
        layout("<p lang=en>hyphenation", "p{hyphens:auto}", 35.0)
            .lines()
            .len(),
        2
    );
    assert_eq!(
        layout("<p>hyphenation", "p{hyphens:auto}", 35.0)
            .lines()
            .len(),
        1
    );
    assert_eq!(
        layout("<p lang=de>hyphenation", "p{hyphens:auto}", 35.0)
            .lines()
            .len(),
        1
    );
    assert_eq!(
        layout(
            "<main lang=en><span style='display:contents'><p>hyphenation",
            "p{hyphens:auto}",
            35.0
        )
        .lines()
        .len(),
        2
    );
    assert_eq!(
        layout(
            "<main lang=en><p lang=''>hyphenation",
            "p{hyphens:auto}",
            35.0
        )
        .lines()
        .len(),
        1
    );
    let l = layout("<p lang=en>hyphena&shy;tion", "p{hyphens:auto}", 35.0);
    assert_eq!(l.lines().len(), 1);
}
#[test]
fn inline_bidi_scopes_keep_source_text_and_override_levels() {
    let l = layout("<p>a<bdo dir=rtl>xxx</bdo>z", "", 100.0);
    let span = l.texts().iter().find(|t| t.text() == (0..3)).unwrap();
    assert!(span.bidi_level() % 2 == 1);
    let l = layout(
        "<p dir=rtl><span style='unicode-bidi:isolate;direction:ltr'>xxx</span>",
        "",
        100.0,
    );
    assert!(l.texts().iter().any(|t| t.bidi_level() % 2 == 0));
    let l = layout("<p style='unicode-bidi:plaintext'>אב", "", 50.0);
    assert!(l.texts()[0].rect().origin().x.get() > 30.0);
}
#[test]
fn opentype_mark_offsets_reach_absolute_glyph_positions() {
    let mut fonts = FontDatabase::new();
    fonts
        .load(
            include_bytes!("../../rustic-text/tests/fonts/shaping/RusticCFFTest.otf").to_vec(),
            &FontLimits::default(),
        )
        .unwrap();
    fonts.set_fallback_family("Rustic CFF Test");
    let l = with_fonts(
        "<p><span>x\u{301}</span>",
        "span{position:relative;left:7px;top:3px}",
        100.0,
        fonts,
    )
    .unwrap();
    let t = &l.texts()[0];
    assert_eq!(t.glyphs().len(), 2);
    assert!(t.glyphs()[1].y < t.baseline().get() - 5.0);
    assert!(t.glyphs()[0].x >= 7.0);
    assert_eq!(t.glyphs()[0].y, t.baseline().get());
}
#[test]
fn arabic_joining_survives_source_node_boundaries() {
    let make = || {
        let mut d = FontDatabase::new();
        d.load(
            include_bytes!("../../rustic-text/tests/fonts/shaping/Arabic.ttf").to_vec(),
            &FontLimits::default(),
        )
        .unwrap();
        d.set_fallback_family("Rustic Arabic Fixture");
        d
    };
    let together = with_fonts("<p dir=rtl>بب", "", 100.0, make()).unwrap();
    let split = with_fonts("<p dir=rtl>ب<span>ب</span>", "", 100.0, make()).unwrap();
    let ids = |l: BlockLayout| {
        l.texts()
            .iter()
            .flat_map(|t| t.glyphs().iter().map(|g| g.glyph))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(together), ids(split));
}
#[test]
fn unsupported_phrase_segmentation_fails_explicitly() {
    assert!(
        with_fonts(
            "<p>hello world",
            "p{word-break:auto-phrase}",
            20.0,
            rustic_text::bundled::test_database()
        )
        .is_err()
    );
}

#[test]
fn selected_subtree_inherits_ancestor_language_without_mutating_source() {
    let (doc, styles) = common::page("<main lang=en><p>hyphenation", "");
    let root=styles.iter().find(|s|matches!(doc.node(s.node).unwrap().kind(),rustic_dom::NodeKind::Element(e) if e.name().local()=="p")).unwrap().node;
    let tree = BoxTree::build(
        &doc,
        root,
        &styles,
        LayoutLimits::default(),
        &CancellationToken::default(),
    )
    .unwrap();
    assert!(tree.boxes().iter().all(|b| b.language() == Some("en")));
    assert_eq!(
        String::from_utf16(tree.boxes().last().unwrap().text()).unwrap(),
        "hyphenation"
    );
}
