//! Custom properties, `var()` substitution and dependency cycles (CSS-06).

use rustic_css::{CssParser, PropertyId};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument};
use rustic_style::{
    Cascade, ComputeContext, ElementStyle, MatchContext, Origin, StyleSheet, dump_computed_styles,
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

const NESTED: &str = "<!doctype html><div id=outer><div id=inner><p id=p>text</p></div></div>";

#[test]
fn custom_properties_inherit_and_substitute() {
    let page = Styled::new(
        NESTED,
        "#outer { --gap: 4px; --c: red } #inner { --gap: 8px } \
         p { margin: var(--gap) calc(var(--gap) * 2); color: var(--c) }",
    );
    assert_eq!(page.custom("p", "--gap").as_deref(), Some("8px"));
    assert_eq!(page.custom("outer", "--gap").as_deref(), Some("4px"));
    assert_eq!(page.get("p", PropertyId::MarginTop), "8px");
    assert_eq!(page.get("p", PropertyId::MarginRight), "16px");
    assert_eq!(page.get("p", PropertyId::MarginLeft), "16px");
    assert_eq!(page.get("p", PropertyId::Color), "rgb(255, 0, 0)");
}

#[test]
fn custom_property_names_are_case_sensitive() {
    let page = Styled::new(NESTED, "#p { --A: 3px; --a: 5px; width: var(--A) }");
    assert_eq!(page.get("p", PropertyId::Width), "3px");
}

#[test]
fn fallbacks_and_invalid_at_computed_value_time() {
    let page = Styled::new(
        NESTED,
        "#outer { color: blue; width: 7px } \
         #inner { color: var(--missing); width: var(--missing) } \
         #p { margin-top: var(--missing, 3px); margin-left: var(--missing, var(--also, 4px)); \
              --word: banana; padding-top: var(--word) }",
    );
    // An invalid reference makes the property `unset`: color inherits,
    // width takes its initial value.
    assert_eq!(page.get("inner", PropertyId::Color), "rgb(0, 0, 255)");
    assert_eq!(page.get("inner", PropertyId::Width), "auto");
    assert_eq!(page.get("p", PropertyId::MarginTop), "3px");
    assert_eq!(page.get("p", PropertyId::MarginLeft), "4px");
    // Substituted but not a valid length.
    assert_eq!(page.get("p", PropertyId::PaddingTop), "0px");
}

#[test]
fn css_wide_keywords_on_custom_properties() {
    let page = Styled::new(
        NESTED,
        "#outer { --x: 2px } #inner { --x: initial } #p { --x: inherit; width: var(--x, 9px) }",
    );
    assert_eq!(page.custom("inner", "--x"), None);
    assert_eq!(page.custom("p", "--x"), None);
    assert_eq!(page.get("p", PropertyId::Width), "9px");
    let page = Styled::new(
        NESTED,
        "#outer { --x: 2px } #p { --x: unset; width: var(--x) }",
    );
    assert_eq!(page.get("p", PropertyId::Width), "2px");
}

#[test]
fn cycles_make_every_member_invalid() {
    let page = Styled::new(
        NESTED,
        "#outer { --a: 1px } \
         #p { --a: var(--b); --b: var(--c, 1px); --c: var(--a); \
              --self: var(--self, 2px); --after: var(--a, 5px); --ok: var(--d); --d: 6px; \
              width: var(--a, 3px); height: var(--after); margin-top: var(--ok) }",
    );
    for name in ["--a", "--b", "--c", "--self"] {
        assert_eq!(page.custom("p", name), None, "{name}");
    }
    // Referring to a cycle member is not being in the cycle: the fallback
    // applies.
    assert_eq!(page.custom("p", "--after").as_deref(), Some("5px"));
    assert_eq!(page.get("p", PropertyId::Width), "3px");
    assert_eq!(page.get("p", PropertyId::Height), "5px");
    assert_eq!(page.get("p", PropertyId::MarginTop), "6px");
    // The parent's value is not visible through a cycle.
    assert_eq!(page.custom("outer", "--a").as_deref(), Some("1px"));
}

#[test]
fn var_in_shorthands_and_style_attributes() {
    let page = Styled::new(
        "<!doctype html><div id=d style=\"--w: 2px; border: var(--w) solid var(--col, green)\"></div>",
        "",
    );
    assert_eq!(page.get("d", PropertyId::BorderLeftWidth), "2px");
    assert_eq!(page.get("d", PropertyId::BorderTopStyle), "solid");
    assert_eq!(
        page.get("d", PropertyId::BorderBottomColor),
        "rgb(0, 128, 0)"
    );
    // A later longhand overrides one longhand of the var() shorthand.
    let page = Styled::new(
        NESTED,
        "#p { --m: 1px 2px; margin: var(--m); margin-left: 9px }",
    );
    assert_eq!(page.get("p", PropertyId::MarginTop), "1px");
    assert_eq!(page.get("p", PropertyId::MarginRight), "2px");
    assert_eq!(page.get("p", PropertyId::MarginLeft), "9px");
}

#[test]
fn relative_units_resolve_after_substitution() {
    let page = Styled::new(
        NESTED,
        "#outer { --size: 2em; font-size: 10px } #p { font-size: 20px; width: var(--size) }",
    );
    // The token `2em` is inherited, then resolved against p's font size.
    assert_eq!(page.custom("p", "--size").as_deref(), Some("2em"));
    assert_eq!(page.get("p", PropertyId::Width), "40px");
}

#[test]
fn exponential_expansion_is_cut_off() {
    let mut css = String::from("#p { --v0: x x x x x x x x;");
    for i in 1..40 {
        css.push_str(&format!(" --v{i}: var(--v{0}) var(--v{0});", i - 1));
    }
    css.push_str(" width: var(--v39, 1px) }");
    let page = Styled::new(NESTED, &css);
    assert_eq!(page.custom("p", "--v39"), None);
    assert!(page.custom("p", "--v5").is_some());
    assert_eq!(page.get("p", PropertyId::Width), "1px");
}

#[test]
fn dumps_list_custom_properties_where_they_change() {
    let page = Styled::new(NESTED, "#outer { --x: 1 } #p { --x: 2; --y: a  b }");
    let dump = dump_computed_styles(page.page.document(), &page.styles, 1 << 20).unwrap();
    assert!(dump.contains("div#outer\n      --x: 1\n"), "{dump}");
    assert!(
        dump.contains("p#p\n          --x: 2\n          --y: a b\n"),
        "{dump}"
    );
    assert!(!dump.contains("div#inner\n        --x"), "{dump}");
}

#[test]
fn chained_variables_are_computed_once_and_shared() {
    // TD-33: a doubling chain on many siblings is computed once per
    // distinct input and shared, instead of once per element.
    let mut css = String::from("p { --v0: aaaaaaaaaa;");
    for i in 1..40 {
        css.push_str(&format!(" --v{i}: var(--v{0}) var(--v{0});", i - 1));
    }
    css.push_str(" width: var(--v39) }");
    let html = format!(
        "<!doctype html><div>{}</div><section><p id=far></p></section>",
        "<p></p>".repeat(2000)
    );
    let started = std::time::Instant::now();
    let page = Styled::new(&html, &css);
    let elapsed = started.elapsed();
    let doc = page.page.document();
    let paragraphs: Vec<&ElementStyle> = page
        .styles
        .iter()
        .filter(|s| match doc.node(s.node).unwrap().kind() {
            NodeKind::Element(e) => e.name().local() == "p",
            _ => false,
        })
        .collect();
    assert_eq!(paragraphs.len(), 2001);
    let first = paragraphs[0].computed.custom_properties();
    assert!(first.get("--v12").is_some());
    assert!(first.get("--v39").is_none(), "past the budget");
    for p in &paragraphs[1..2000] {
        assert!(p.computed.custom_properties().shares_storage_with(first));
    }
    // Same values under another parent (not shared storage, equal content).
    assert_eq!(paragraphs[2000].computed.custom_properties(), first);
    assert_eq!(page.get("far", PropertyId::Width), "auto");
    // Generous: about 0.1 s in release builds; per-element expansion took
    // seconds even in release.
    assert!(elapsed.as_secs() < 30, "{elapsed:?}");
}
