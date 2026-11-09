//! Font-relative units (`ex`, `ch`, `cap`, `ic` and root forms) computed from
//! loaded font metrics (REN-03a), with the bundled original test fonts.

use std::sync::{Arc, Mutex};

use rustic_css::{CssParser, FamilyName, FontStyle, GenericFamily, PropertyId};
use rustic_dom::{Document, NodeId, NodeKind, QualifiedName};
use rustic_html::HtmlDocumentParser;
use rustic_style::{
    Cascade, ComputeContext, ComputedStyle, EmRatios, FontMetricsSource, FontQuery, MatchContext,
    Mutation, Origin, StyleSheet, StyledDocument, style_tree,
};
use rustic_text::bundled::{TEST_FONTS, test_database};
use rustic_text::{FontDatabase, FontLimits};

fn id_of(document: &Document, id: NodeId) -> Option<String> {
    match document.node(id).ok()?.kind() {
        NodeKind::Element(element) => element
            .attributes()
            .iter()
            .find(|a| a.name().local() == "id")
            .map(|a| String::from_utf16_lossy(a.value().as_units())),
        _ => None,
    }
}

/// Every element's computed style keyed by id, styled with `compute`.
fn styles(html: &str, author: &str, compute: &ComputeContext) -> Vec<(String, ComputedStyle)> {
    let parser = CssParser::default();
    let mut html_parser = HtmlDocumentParser::default();
    html_parser.feed(html.as_bytes()).unwrap();
    let page = html_parser.finish().unwrap();
    let mut cascade = Cascade::new();
    let (sheet, diagnostics) = StyleSheet::parse(&parser, author, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    cascade.push(sheet);
    let doc = page.document();
    style_tree(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        compute,
    )
    .unwrap()
    .into_iter()
    .filter_map(|s| Some((id_of(doc, s.node)?, s.computed)))
    .collect()
}

fn fonts() -> ComputeContext {
    ComputeContext::with_fonts(Arc::new(test_database()))
}

fn px(styles: &[(String, ComputedStyle)], id: &str, property: &str) -> String {
    let style = &styles.iter().find(|(i, _)| i == id).expect("element").1;
    style
        .get(PropertyId::from_name(property).unwrap())
        .to_string()
}

#[test]
fn units_follow_the_selected_face() {
    let html = "<div id=r></div><div id=b></div><div id=i></div><div id=c></div>";
    let sheet = "html { font: 20px 'Rustic Test' } \
                 div { width: 10ex; height: 10cap; min-width: 10ch; min-height: 10ic } \
                 #b { font-weight: bold } #i { font-style: italic; font-size: 2048px } \
                 #c { font-stretch: condensed }";
    let s = styles(html, sheet, &fonts());
    // Regular: x 0.5, cap 0.7, `0` 0.55, 水 1.0 em.
    assert_eq!(px(&s, "r", "width"), "100px");
    assert_eq!(px(&s, "r", "height"), "140px");
    assert_eq!(px(&s, "r", "min-width"), "110px");
    assert_eq!(px(&s, "r", "min-height"), "200px");
    // Bold: x 0.52, cap 0.72, `0` 0.6.
    assert_eq!(px(&s, "b", "width"), "104px");
    assert_eq!(px(&s, "b", "height"), "144px");
    assert_eq!(px(&s, "b", "min-width"), "120px");
    // Italic (2048 units/em, OS/2 v1): x and cap from outlines; no 水, so
    // `ic` is 1em.
    assert_eq!(px(&s, "i", "width"), "9830px");
    assert_eq!(px(&s, "i", "height"), "14130px");
    assert_eq!(px(&s, "i", "min-width"), "11060px");
    assert_eq!(px(&s, "i", "min-height"), "20480px");
    // Condensed: `0` 0.45, 水 0.9.
    assert_eq!(px(&s, "c", "min-width"), "90px");
    assert_eq!(px(&s, "c", "min-height"), "180px");
}

#[test]
fn without_fonts_the_css_fallbacks_apply() {
    let html = "<div id=a></div>";
    let sheet = "html { font: 20px 'Rustic Test' } \
                 #a { font-weight: bold; width: 10ex; height: 10cap; min-width: 10ch; \
                 min-height: 10ic }";
    for compute in [
        ComputeContext::default(),
        ComputeContext::with_fonts(Arc::new(FontDatabase::new())),
    ] {
        let s = styles(html, sheet, &compute);
        assert_eq!(px(&s, "a", "width"), "100px");
        assert_eq!(px(&s, "a", "height"), "140px");
        assert_eq!(px(&s, "a", "min-width"), "100px");
        assert_eq!(px(&s, "a", "min-height"), "200px");
    }
}

#[test]
fn family_lists_skip_unavailable_families() {
    // Only the bold face, with no generic or fallback mapping.
    let mut db = FontDatabase::new();
    db.load(TEST_FONTS[1].1.to_vec(), &FontLimits::default())
        .unwrap();
    let compute = ComputeContext::with_fonts(Arc::new(db));
    let html = "<div id=a></div><div id=b></div><div id=c></div>";
    let sheet = "div { width: 10ch } #a { font-family: Missing, 'rustic test', serif } \
                 #b { font-family: Missing, serif } #c { font-family: monospace }";
    let s = styles(html, sheet, &compute);
    // Any weight reaches the family's only face.
    assert_eq!(px(&s, "a", "width"), "96px");
    assert_eq!(px(&s, "b", "width"), "80px");
    assert_eq!(px(&s, "c", "width"), "80px");
}

#[test]
fn parent_and_root_fonts_feed_font_size_and_root_units() {
    let html = "<div id=p><div id=c></div></div>";
    let sheet = "html { font: bold 16px 'Rustic Test' } \
                 #p { font-size: 20px } \
                 #c { font-weight: normal; font-size: 2ex; width: 1ex; height: 1rch; \
                 min-width: 1ch; min-height: 2rex }";
    let s = styles(html, sheet, &fonts());
    // `ex` in font-size uses the parent's (bold) font: 2 × 0.52 × 20.
    assert_eq!(px(&s, "c", "font-size"), "20.8px");
    // Other `ex`/`ch` use the element's own (regular) font.
    assert_eq!(px(&s, "c", "width"), "10.4px");
    assert_eq!(px(&s, "c", "min-width"), "11.44px");
    // Root units use the root's font (bold, 16px).
    assert_eq!(px(&s, "c", "height"), "9.6px");
    assert_eq!(px(&s, "c", "min-height"), "16.64px");
    // Inherited font properties inherit the metrics.
    let html = "<div id=p><span id=c></span></div>";
    let sheet = "html { font: 16px 'Rustic Test' } #p { font-weight: bold } \
                 #c { margin-left: 1ch }";
    assert_eq!(
        px(&styles(html, sheet, &fonts()), "c", "margin-left"),
        "9.6px"
    );
}

#[test]
fn root_ex_in_font_size_uses_the_initial_font() {
    // The root's parent is the initial font (`serif`, mapped to the test
    // family at weight 400), so `ch` in the root's font-size uses Regular
    // even though the root itself is bold.
    let html = "<p id=a></p>";
    let sheet = "html { font-family: 'Rustic Test'; font-weight: bold; font-size: 3ch }";
    let compute = fonts();
    let parser = CssParser::default();
    let mut html_parser = HtmlDocumentParser::default();
    html_parser.feed(html.as_bytes()).unwrap();
    let page = html_parser.finish().unwrap();
    let mut cascade = Cascade::new();
    cascade.push(StyleSheet::parse(&parser, sheet, Origin::Author).unwrap().0);
    let doc = page.document();
    let styles = style_tree(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &compute,
    )
    .unwrap();
    let root = &styles[0].computed;
    // 3 × 0.55 (Regular `0`) × 16.
    assert_eq!(root.get(PropertyId::FontSize).to_string(), "26.4px");
    assert_eq!(root.font_metrics().zero_advance, 0.6);
}

/// One recorded query: families, weight, style and stretch.
type Query = (Vec<FamilyName>, f64, FontStyle, f64);

/// Records every query and answers with fixed ratios.
#[derive(Debug, Default)]
struct Recorder(Mutex<Vec<Query>>);

impl FontMetricsSource for Recorder {
    fn em_ratios(&self, font: &FontQuery<'_>) -> Option<EmRatios> {
        self.0.lock().unwrap().push((
            font.families.to_vec(),
            font.weight,
            font.style,
            font.stretch,
        ));
        Some(EmRatios {
            x_height: 0.25,
            cap_height: 0.75,
            zero_advance: 0.125,
            ideograph_advance: 0.875,
        })
    }
}

#[test]
fn sources_receive_computed_font_properties() {
    let recorder = Arc::new(Recorder::default());
    let compute = ComputeContext::with_fonts(recorder.clone());
    let html = "<div id=a><div id=b></div></div>";
    let sheet = "html { font-size: 16px } \
                 #a { font-family: Foo, sans-serif; font-weight: 300; font-style: oblique; \
                 font-stretch: expanded; width: 4ex } \
                 #b { font-weight: bolder; width: 8ch; height: 8ic; min-width: 4cap }";
    let s = styles(html, sheet, &compute);
    assert_eq!(px(&s, "a", "width"), "16px");
    assert_eq!(px(&s, "b", "width"), "16px");
    assert_eq!(px(&s, "b", "height"), "112px");
    assert_eq!(px(&s, "b", "min-width"), "48px");
    let queries = recorder.0.lock().unwrap();
    let family = vec![
        FamilyName::Named("Foo".to_owned()),
        FamilyName::Generic(GenericFamily::SansSerif),
    ];
    assert!(queries.contains(&(family.clone(), 300.0, FontStyle::Oblique, 125.0)));
    // `bolder` from 300 is 400, resolved before the query.
    assert!(queries.contains(&(family, 400.0, FontStyle::Oblique, 125.0)));
    // Elements whose font properties equal their parent's reuse its ratios.
    let initial = vec![FamilyName::Generic(GenericFamily::Serif)];
    let initial_queries = queries
        .iter()
        .filter(|q| q.0 == initial && q.1 == 400.0)
        .count();
    assert!(
        initial_queries <= 2,
        "head/body must not re-query: {queries:?}"
    );
}

#[test]
fn contexts_compare_by_source_identity() {
    let shared: Arc<dyn FontMetricsSource> = Arc::new(test_database());
    let a = ComputeContext::with_fonts(shared.clone());
    let b = ComputeContext::with_fonts(shared);
    let c = fonts();
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, ComputeContext::default());
    assert_eq!(ComputeContext::default(), ComputeContext::default());
    assert!(format!("{a:?}").contains("fonts: true"));
}

#[test]
fn incremental_restyle_matches_a_full_restyle_with_fonts() {
    let html = "<div id=a><span id=b></span></div>";
    let sheet = "html { font: 16px 'Rustic Test' } span { width: 3ch } \
                 .bold { font-weight: bold }";
    let compute = fonts();
    let parser = CssParser::default();
    let cx = MatchContext::html();
    let mut html_parser = HtmlDocumentParser::default();
    html_parser.feed(html.as_bytes()).unwrap();
    let mut document = html_parser.finish().unwrap().into_document();
    let mut cascade = Cascade::new();
    cascade.push(StyleSheet::parse(&parser, sheet, Origin::Author).unwrap().0);
    let mut styled = StyledDocument::new(&document, &cascade, &parser, &cx, &compute).unwrap();
    let a = document
        .walk(document.root())
        .unwrap()
        .map(|(node, _)| node)
        .find(|&n| id_of(&document, n).as_deref() == Some("a"))
        .unwrap();
    document
        .set_attribute(
            a,
            QualifiedName::from_parsed(None, None, "class").unwrap(),
            "bold",
        )
        .unwrap();
    styled.note(
        &document,
        Mutation::Attribute {
            element: a,
            local: "class".into(),
        },
    );
    styled
        .restyle(&document, &cascade, &parser, &cx, &compute)
        .unwrap();
    let full = style_tree(&document, document.root(), &cascade, &parser, &cx, &compute).unwrap();
    assert_eq!(styled.styles(), full.as_slice());
    let b = full
        .iter()
        .find(|s| id_of(&document, s.node).as_deref() == Some("b"))
        .unwrap();
    // Bold `0` advance: 3 × 0.6 × 16.
    assert_eq!(b.computed.get(PropertyId::Width).to_string(), "28.8px");
}
