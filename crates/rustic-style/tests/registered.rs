//! Registered custom properties (`@property`) in the cascade, including
//! cycles through font-relative units.

use rustic_css::{CssParser, PropertyId};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument};
use rustic_style::{
    Cascade, ComputeContext, ElementStyle, MatchContext, Origin, StyleDiagnosticKind, StyleSheet,
    style_tree,
};

fn document(html: &str) -> ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
}

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

struct Styled {
    page: ParsedDocument,
    styles: Vec<ElementStyle>,
}

impl Styled {
    fn new(html: &str, author: &str) -> Self {
        let parser = CssParser::default();
        let page = document(html);
        let mut cascade = Cascade::new();
        let (sheet, diagnostics) = StyleSheet::parse(&parser, author, Origin::Author).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        cascade.push(sheet);
        let doc = page.document();
        let styles = style_tree(
            doc,
            doc.root(),
            &cascade,
            &parser,
            &MatchContext::html(),
            &ComputeContext::default(),
        )
        .unwrap();
        Self { page, styles }
    }

    fn style(&self, id: &str) -> &ElementStyle {
        let doc = self.page.document();
        self.styles
            .iter()
            .find(|s| id_of(doc, s.node).as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no element #{id}"))
    }

    fn get(&self, id: &str, property: PropertyId) -> String {
        self.style(id).computed.get(property).to_string()
    }

    fn custom(&self, id: &str, name: &str) -> Option<String> {
        self.style(id)
            .computed
            .custom_properties()
            .get(name)
            .map(ToString::to_string)
    }
}

const PAGE: &str = "<!doctype html><html id=root><body id=body><div id=outer><div id=inner></div></div></body></html>";

const LENGTH: &str = "@property --x { syntax: '<length>'; inherits: false; initial-value: 5px }";

#[test]
fn non_inherited_properties_take_their_initial_value() {
    let page = Styled::new(PAGE, &format!("{LENGTH} #outer {{ --x: 10px }}"));
    assert_eq!(page.custom("root", "--x").as_deref(), Some("5px"));
    assert_eq!(page.custom("outer", "--x").as_deref(), Some("10px"));
    assert_eq!(page.custom("inner", "--x").as_deref(), Some("5px"));
}

#[test]
fn inherited_properties_start_from_their_initial_value() {
    let page = Styled::new(
        PAGE,
        "@property --c { syntax: '<color>'; inherits: true; initial-value: blue }
         #outer { --c: lime } #inner { color: var(--c) } #body { color: var(--c) }",
    );
    assert_eq!(
        page.custom("root", "--c").as_deref(),
        Some("rgb(0, 0, 255)")
    );
    assert_eq!(
        page.custom("inner", "--c").as_deref(),
        Some("rgb(0, 255, 0)")
    );
    assert_eq!(page.get("body", PropertyId::Color), "rgb(0, 0, 255)");
    assert_eq!(page.get("inner", PropertyId::Color), "rgb(0, 255, 0)");
}

#[test]
fn values_are_checked_and_canonical() {
    let page = Styled::new(
        PAGE,
        &format!(
            "{LENGTH}
             @property --y {{ syntax: '<length>'; inherits: true; initial-value: 1px }}
             #outer {{ --x: 0; --y: 7px }}
             #inner {{ --x: red; --y: red }}"
        ),
    );
    assert_eq!(page.custom("outer", "--x").as_deref(), Some("0px"));
    // Invalid at computed-value time: `unset`, so initial or inherited.
    assert_eq!(page.custom("inner", "--x").as_deref(), Some("5px"));
    assert_eq!(page.custom("inner", "--y").as_deref(), Some("7px"));
}

#[test]
fn substituted_values_are_checked() {
    let page = Styled::new(
        PAGE,
        &format!(
            "{LENGTH}
             #outer {{ --a: 3px; --b: blue; --x: var(--a) }}
             #inner {{ --x: var(--b) }}"
        ),
    );
    assert_eq!(page.custom("outer", "--x").as_deref(), Some("3px"));
    assert_eq!(page.custom("inner", "--x").as_deref(), Some("5px"));
}

#[test]
fn css_wide_keywords_follow_the_registration() {
    let page = Styled::new(
        PAGE,
        &format!(
            "{LENGTH}
             #outer {{ --x: 10px }}
             #inner {{ --x: inherit }}
             #body {{ --x: 9px }} #body {{ --x: initial }}"
        ),
    );
    assert_eq!(page.custom("inner", "--x").as_deref(), Some("10px"));
    assert_eq!(page.custom("body", "--x").as_deref(), Some("5px"));
}

#[test]
fn relative_lengths_compute_to_pixels_and_inherit_as_pixels() {
    let page = Styled::new(
        PAGE,
        "@property --w { syntax: '<length>'; inherits: true; initial-value: 0px }
         #outer { font-size: 20px; --w: 2em }
         #inner { font-size: 10px; width: var(--w) }
         #body { --u: 2em }",
    );
    assert_eq!(page.custom("outer", "--w").as_deref(), Some("40px"));
    assert_eq!(page.custom("inner", "--w").as_deref(), Some("40px"));
    assert_eq!(page.get("inner", PropertyId::Width), "40px");
    // Unregistered values stay tokens.
    assert_eq!(page.custom("body", "--u").as_deref(), Some("2em"));
}

#[test]
fn font_size_cycles_through_em_make_both_unset() {
    let page = Styled::new(
        PAGE,
        "@property --x { syntax: '<length>'; inherits: false; initial-value: 0px }
         #outer { font-size: 20px }
         #inner { --x: 1em; font-size: var(--x) }
         #body { --y: 2em; font-size: var(--y) }",
    );
    assert_eq!(page.get("inner", PropertyId::FontSize), "20px");
    assert_eq!(page.custom("inner", "--x").as_deref(), Some("0px"));
    // An unregistered property is not resolved, so no cycle: 2em of 16px.
    assert_eq!(page.get("body", PropertyId::FontSize), "32px");
    assert_eq!(page.custom("body", "--y").as_deref(), Some("2em"));
}

#[test]
fn indirect_cycles_include_every_member() {
    let page = Styled::new(
        PAGE,
        "@property --x { syntax: '<length>'; inherits: false; initial-value: 0px }
         @property --y { syntax: '<length>'; inherits: false; initial-value: 1px }
         #outer { font-size: 20px }
         #inner { --x: 1em; --y: var(--x); font-size: var(--y); --z: var(--y) }",
    );
    assert_eq!(page.get("inner", PropertyId::FontSize), "20px");
    assert_eq!(page.custom("inner", "--x").as_deref(), Some("0px"));
    assert_eq!(page.custom("inner", "--y").as_deref(), Some("1px"));
    assert_eq!(page.custom("inner", "--z").as_deref(), Some("1px"));
}

#[test]
fn line_height_cycles_through_lh() {
    let page = Styled::new(
        PAGE,
        "@property --x { syntax: '<length>'; inherits: false; initial-value: 0px }
         #outer { line-height: 30px }
         #inner { --x: 2lh; line-height: var(--x); font-size: 12px }",
    );
    assert_eq!(page.get("inner", PropertyId::LineHeight), "30px");
    assert_eq!(page.get("inner", PropertyId::FontSize), "12px");
    assert_eq!(page.custom("inner", "--x").as_deref(), Some("0px"));
}

#[test]
fn rem_cycles_only_on_the_root() {
    let page = Styled::new(
        PAGE,
        "@property --x { syntax: '<length>'; inherits: false; initial-value: 0px }
         #root { --x: 2rem; font-size: var(--x) }
         #outer { --x: 2rem; font-size: var(--x) }",
    );
    assert_eq!(page.get("root", PropertyId::FontSize), "16px");
    assert_eq!(page.custom("root", "--x").as_deref(), Some("0px"));
    assert_eq!(page.get("outer", PropertyId::FontSize), "32px");
    assert_eq!(page.custom("outer", "--x").as_deref(), Some("32px"));
}

#[test]
fn invalid_property_rules_are_dropped() {
    let parser = CssParser::default();
    let source = "
        @property --a { inherits: true; initial-value: 1px }
        @property --b { syntax: '<length>'; initial-value: 1px }
        @property --c { syntax: '<length>'; inherits: false }
        @property --d { syntax: '<length>'; inherits: false; initial-value: 1em }
        @property --e { syntax: '<angle>'; inherits: false; initial-value: 1deg }
        @property --f { syntax: '<length>'; inherits: maybe; initial-value: 1px }
        @property --g { syntax: '<length>'; inherits: false; initial-value: red }
        @property --h { syntax: '*'; inherits: false; initial-value: var(--a) }
        @property h { syntax: '*'; inherits: false }
        @property --i --j { syntax: '*'; inherits: false }
        @property --k;
        @media screen { @property --l { syntax: '*'; inherits: false } }
        @property --m { syntax: <length>; inherits: false; initial-value: 1px }
        @property --ok { syntax: '*'; inherits: false }
        @layer base { @property --also { syntax: '<number>'; inherits: true; initial-value: 2 } }
    ";
    let (sheet, diagnostics) = StyleSheet::parse(&parser, source, Origin::Author).unwrap();
    let dropped = diagnostics
        .iter()
        .filter(|d| d.kind == StyleDiagnosticKind::InvalidPropertyRule)
        .count();
    assert_eq!(dropped, 13, "{diagnostics:?}");
    let names: Vec<&str> = sheet.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["--ok", "--also"]);
    assert!(sheet.properties[0].initial.is_none());
}

#[test]
fn later_registrations_win() {
    let parser = CssParser::default();
    let mut cascade = Cascade::new();
    for source in [
        "@property --x { syntax: '<length>'; inherits: false; initial-value: 1px }",
        "@property --x { syntax: '<number>'; inherits: true; initial-value: 2 }",
    ] {
        cascade.push(
            StyleSheet::parse(&parser, source, Origin::Author)
                .unwrap()
                .0,
        );
    }
    let registration = cascade.registration("--x").unwrap();
    assert!(registration.inherits);
    assert_eq!(registration.syntax.to_string(), "<number>");
    assert!(cascade.registration("--y").is_none());
}
