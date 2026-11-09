use rustic_css::CssParser;
use rustic_dom::Document;
use rustic_html::HtmlDocumentParser;
use rustic_style::{
    ComputeContext, ElementStyle, MatchContext, Origin, StyleSheet, document_cascade, style_tree,
};

pub fn page(html: &str, css: &str) -> (Document, Vec<ElementStyle>) {
    page_with_context(html, css, ComputeContext::default())
}

pub fn page_with_context(
    html: &str,
    css: &str,
    context: ComputeContext,
) -> (Document, Vec<ElementStyle>) {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    let doc = parser.finish().unwrap().into_document();
    let parser = CssParser::default();
    let (mut cascade, _) = document_cascade(&doc, &parser).unwrap();
    let (sheet, diagnostics) = StyleSheet::parse(&parser, css, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    cascade.push(sheet);
    let styles = style_tree(
        &doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &context,
    )
    .unwrap();
    (doc, styles)
}
