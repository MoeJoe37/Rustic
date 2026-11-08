//! `@container` rules in the cascade and their invalidation (CSS-06).

use std::collections::HashMap;

use rustic_css::{CssParser, PropertyId};
use rustic_dom::{Document, NodeId, NodeKind, QualifiedName};
use rustic_html::HtmlDocumentParser;
use rustic_style::{
    Cascade, ComputeContext, ContainerSize, ElementStyle, MatchContext, Mutation, Origin,
    StyleDiagnosticKind, StyleSheet, StyledDocument, style_tree_with_sizes,
};

fn document(html: &str) -> Document {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap().into_document()
}

fn by_id(document: &Document, id: &str) -> NodeId {
    document
        .walk(document.root())
        .unwrap()
        .map(|(node, _)| node)
        .find(|&node| match document.node(node).unwrap().kind() {
            NodeKind::Element(element) => element.attributes().iter().any(|a| {
                a.name().local() == "id" && String::from_utf16_lossy(a.value().as_units()) == id
            }),
            _ => false,
        })
        .unwrap_or_else(|| panic!("no #{id}"))
}

fn cascade(css: &str) -> Cascade {
    let (sheet, diagnostics) =
        StyleSheet::parse(&CssParser::default(), css, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let mut cascade = Cascade::new();
    cascade.push(sheet);
    cascade
}

fn styles(document: &Document, cascade: &Cascade, sizes: &[(&str, f64, f64)]) -> Vec<ElementStyle> {
    let sizes: HashMap<NodeId, ContainerSize> = sizes
        .iter()
        .map(|&(id, width, height)| (by_id(document, id), ContainerSize { width, height }))
        .collect();
    style_tree_with_sizes(
        document,
        document.root(),
        cascade,
        &CssParser::default(),
        &MatchContext::html(),
        &ComputeContext::default(),
        &sizes,
    )
    .unwrap()
}

fn get(document: &Document, styles: &[ElementStyle], id: &str, property: PropertyId) -> String {
    let node = by_id(document, id);
    styles
        .iter()
        .find(|s| s.node == node)
        .unwrap()
        .computed
        .get(property)
        .to_string()
}

const PAGE: &str = "<!doctype html><div id=outer><div id=card><p id=p>text</p></div></div>";

#[test]
fn size_queries_use_the_nearest_eligible_container() {
    let css = "#outer { container: page / size } #card { container-type: inline-size } \
               @container (width > 300px) { p { color: red } } \
               @container (height > 300px) { p { width: 5px } } \
               @container page (width > 900px) { p { height: 7px } } \
               @container (min-width: 10em) { p { margin-top: 1px } }";
    let doc = document(PAGE);
    let cascade = cascade(css);
    // No sizes from layout: every size query is unknown.
    let none = styles(&doc, &cascade, &[]);
    assert_eq!(get(&doc, &none, "p", PropertyId::Color), "rgb(0, 0, 0)");
    let sized = styles(
        &doc,
        &cascade,
        &[("outer", 1000.0, 800.0), ("card", 400.0, 50.0)],
    );
    // Width: the card (inline-size) is nearest.
    assert_eq!(get(&doc, &sized, "p", PropertyId::Color), "rgb(255, 0, 0)");
    // Height: the card has no block size, so the outer size container answers.
    assert_eq!(get(&doc, &sized, "p", PropertyId::Width), "5px");
    // A named query skips to the named container.
    assert_eq!(get(&doc, &sized, "p", PropertyId::Height), "7px");
    // `em` uses the container's font size (16px): 10em = 160px < 400px.
    assert_eq!(get(&doc, &sized, "p", PropertyId::MarginTop), "1px");
    let narrow = styles(
        &doc,
        &cascade,
        &[("outer", 800.0, 200.0), ("card", 100.0, 50.0)],
    );
    assert_eq!(get(&doc, &narrow, "p", PropertyId::Color), "rgb(0, 0, 0)");
    assert_eq!(get(&doc, &narrow, "p", PropertyId::Width), "auto");
    assert_eq!(get(&doc, &narrow, "p", PropertyId::Height), "auto");
}

#[test]
fn an_element_is_never_its_own_container() {
    let css = "#card { container-type: size } @container (width > 1px) { #card { color: red } }";
    let doc = document(PAGE);
    let styled = styles(&doc, &cascade(css), &[("card", 100.0, 100.0)]);
    assert_eq!(
        get(&doc, &styled, "card", PropertyId::Color),
        "rgb(0, 0, 0)"
    );
}

#[test]
fn style_queries_read_the_container_custom_properties() {
    let css = "#outer { --theme: dark; container-name: shell } #card { --theme: light } \
               @container style(--theme: dark) { p { color: red } } \
               @container shell style(--theme: dark) { p { width: 3px } } \
               @container style(--missing) { p { height: 1px } } \
               @container shell { p { margin-left: 2px } } \
               @container nothing { p { margin-right: 2px } }";
    let doc = document(PAGE);
    let styled = styles(&doc, &cascade(css), &[]);
    // The parent (card) is the nearest style container and is light.
    assert_eq!(get(&doc, &styled, "p", PropertyId::Color), "rgb(0, 0, 0)");
    assert_eq!(get(&doc, &styled, "p", PropertyId::Width), "3px");
    assert_eq!(get(&doc, &styled, "p", PropertyId::Height), "auto");
    assert_eq!(get(&doc, &styled, "p", PropertyId::MarginLeft), "2px");
    assert_eq!(get(&doc, &styled, "p", PropertyId::MarginRight), "0px");
}

#[test]
fn container_rules_nest_and_report_errors() {
    let css = "#outer { container-type: size } \
               p { color: blue; @container (width > 1px) { color: red } }";
    let doc = document(PAGE);
    let styled = styles(&doc, &cascade(css), &[("outer", 10.0, 10.0)]);
    assert_eq!(get(&doc, &styled, "p", PropertyId::Color), "rgb(255, 0, 0)");
    let (_, diagnostics) = StyleSheet::parse(
        &CssParser::default(),
        "@container none (width > 1px) { p { color: red } } @container (width > 1px);",
        Origin::Author,
    )
    .unwrap();
    let kinds: Vec<_> = diagnostics.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [
            StyleDiagnosticKind::InvalidContainerRule,
            StyleDiagnosticKind::InvalidContainerRule
        ]
    );
}

#[test]
fn sizes_and_container_styles_invalidate_descendants() {
    let html =
        "<!doctype html><div id=outer><div id=mid><div id=card><p id=p>x</p></div></div></div>";
    let css = "#outer { container: shell / size } #mid { --theme: light } \
               .dark { --theme: dark } #card { --theme: light } \
               @container shell style(--theme: dark) { p { height: 4px } } \
               @container (width > 500px) { p { color: red } } \
               @container style(--theme: dark) { p { width: 9px } } \
               #card.flip { --theme: dark }";
    let mut doc = document(html);
    let cascade = cascade(css);
    let parser = CssParser::default();
    let cx = MatchContext::html();
    let compute = ComputeContext::default();
    let mut styled = StyledDocument::new(&doc, &cascade, &parser, &cx, &compute).unwrap();
    let p = by_id(&doc, "p");
    let color = |styled: &StyledDocument| {
        styled
            .style(p)
            .unwrap()
            .computed
            .get(PropertyId::Color)
            .to_string()
    };
    let check = |styled: &mut StyledDocument, doc: &Document| {
        styled
            .restyle(doc, &cascade, &parser, &cx, &compute)
            .unwrap();
        assert!(
            styled
                .matches_full_restyle(doc, &cascade, &parser, &cx, &compute)
                .unwrap()
        );
    };
    let outer = by_id(&doc, "outer");
    styled.set_container_size(
        outer,
        Some(ContainerSize {
            width: 600.0,
            height: 10.0,
        }),
    );
    check(&mut styled, &doc);
    assert_eq!(color(&styled), "rgb(255, 0, 0)");
    styled.set_container_size(
        outer,
        Some(ContainerSize {
            width: 400.0,
            height: 10.0,
        }),
    );
    check(&mut styled, &doc);
    assert_eq!(color(&styled), "rgb(0, 0, 0)");
    styled.set_container_size(outer, None);
    check(&mut styled, &doc);
    // The card's own --theme decides; flipping it restyles p.
    let card = by_id(&doc, "card");
    doc.set_attribute(
        card,
        QualifiedName::from_parsed(None, None, "class").unwrap(),
        "flip",
    )
    .unwrap();
    styled.note(
        &doc,
        Mutation::Attribute {
            element: card,
            local: "class".into(),
        },
    );
    check(&mut styled, &doc);
    assert_eq!(
        styled
            .style(p)
            .unwrap()
            .computed
            .get(PropertyId::Width)
            .to_string(),
        "9px"
    );
    // The outer container changes while #mid hides its --theme from the
    // elements in between: p still restyles.
    doc.set_attribute(
        outer,
        QualifiedName::from_parsed(None, None, "class").unwrap(),
        "dark",
    )
    .unwrap();
    styled.note(
        &doc,
        Mutation::Attribute {
            element: outer,
            local: "class".into(),
        },
    );
    check(&mut styled, &doc);
    assert_eq!(
        styled
            .style(p)
            .unwrap()
            .computed
            .get(PropertyId::Height)
            .to_string(),
        "4px"
    );
}
