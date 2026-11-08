//! Focused tree-construction behavior beyond the html5lib document corpus.

use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParserLimits, TreeDiagnosticKind as D};

fn parse(source: &str) -> rustic_html::ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(source.as_bytes()).unwrap();
    parser.finish().unwrap()
}

fn find(doc: &Document, root: NodeId, local: &str) -> NodeId {
    doc.walk(root)
        .unwrap()
        .find_map(|(id, _)| {
            matches!(doc.node(id).unwrap().kind(), NodeKind::Element(e) if e.name().local() == local)
                .then_some(id)
        })
        .unwrap_or_else(|| panic!("missing {local}"))
}

fn dump(source: &str) -> String {
    let result = parse(source);
    let doc = result.document();
    let body = find(doc, doc.root(), "body");
    doc.dump_tree(body, 1 << 20)
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| {
            line.trim_start_matches("  ").replace(
                "namespace=\"http://www.w3.org/1999/xhtml\" prefix=null ",
                "",
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn template_contents_are_not_children_and_nest_without_depth_growth() {
    let result = parse("<template><p>a<template><b>b</template></template>");
    let doc = result.document();
    let head = find(doc, doc.root(), "head");
    let template = find(doc, head, "template");
    assert_eq!(doc.node(template).unwrap().first_child(), None);
    let NodeKind::Element(element) = doc.node(template).unwrap().kind() else {
        unreachable!()
    };
    let contents = element.template_contents().unwrap();
    // fragment, p, "a" and the inner template, whose contents are separate.
    assert_eq!(doc.walk(contents).unwrap().count(), 4);

    // Each template's contents are a separate root, so DOM edge depth does
    // not bound nesting; construction, completion and drop stay iterative.
    let mut limits = ParserLimits::default();
    limits.document.max_depth = 8;
    limits.tokenizer.max_tokens = 1_000_000;
    limits.max_tree_diagnostics = 100_000; // One unclosed-template recovery each.
    let source = "<template>".repeat(20_000);
    let mut parser = HtmlDocumentParser::new(limits, Default::default()).unwrap();
    parser.feed(source.as_bytes()).unwrap();
    let result = parser.finish().unwrap();
    assert!(result.document().node_count() > 40_000);
    drop(result);
}

#[test]
fn cdata_is_text_only_in_foreign_content() {
    assert_eq!(
        dump("<svg><![CDATA[a<b>]]></svg><div><![CDATA[x]]></div>"),
        concat!(
            "element namespace=\"http://www.w3.org/2000/svg\" prefix=null local=\"svg\"\n",
            "text \"a<b>\"\n",
            "element local=\"div\"\n",
            "comment \"[CDATA[x]]\"",
        )
    );
}

#[test]
fn foreign_names_and_attributes_are_adjusted_with_namespaces() {
    assert_eq!(
        dump(
            "<svg viewbox='0 0 1 1'><lineargradient xlink:href='#a' xml:lang='en'/></svg><math definitionurl=u><mi>x</mi></math>"
        ),
        concat!(
            "element namespace=\"http://www.w3.org/2000/svg\" prefix=null local=\"svg\"\n",
            "attribute namespace=null prefix=null local=\"viewBox\" value=\"0 0 1 1\"\n",
            "element namespace=\"http://www.w3.org/2000/svg\" prefix=null local=\"linearGradient\"\n",
            "attribute namespace=\"http://www.w3.org/1999/xlink\" prefix=\"xlink\" local=\"href\" value=\"#a\"\n",
            "attribute namespace=\"http://www.w3.org/XML/1998/namespace\" prefix=\"xml\" local=\"lang\" value=\"en\"\n",
            "element namespace=\"http://www.w3.org/1998/Math/MathML\" prefix=null local=\"math\"\n",
            "attribute namespace=null prefix=null local=\"definitionURL\" value=\"u\"\n",
            "element namespace=\"http://www.w3.org/1998/Math/MathML\" prefix=null local=\"mi\"\n",
            "text \"x\"",
        )
    );
    // A self-closing foreign element is acknowledged; HTML breakout pops it.
    let result = parse("<!doctype html><svg/><svg><p>");
    assert!(
        result
            .tree_diagnostics()
            .iter()
            .all(|d| d.kind != D::UnacknowledgedSelfClosing)
    );
}

#[test]
fn table_text_is_foster_parented_and_formatting_is_adopted() {
    assert_eq!(
        dump("<table>x<tr><td>c</table><b>1<p>2</b>3"),
        concat!(
            "text \"x\"\n",
            "element local=\"table\"\n",
            "element local=\"tbody\"\n",
            "element local=\"tr\"\n",
            "element local=\"td\"\n",
            "text \"c\"\n",
            "element local=\"b\"\n",
            "text \"1\"\n",
            "element local=\"p\"\n",
            "element local=\"b\"\n",
            "text \"2\"\n",
            "text \"3\"",
        )
    );
}

#[test]
fn frameset_replaces_an_unused_body_and_pre_skips_one_newline() {
    let result = parse("<frameset><frame></frameset><noframes>n</noframes>");
    let doc = result.document();
    assert!(doc.walk(doc.root()).unwrap().all(|(id, _)| !matches!(
        doc.node(id).unwrap().kind(),
        NodeKind::Element(e) if e.name().local() == "body"
    )));
    find(doc, doc.root(), "frame");
    assert_eq!(
        dump("<pre>\n\nx</pre><textarea>\ny</textarea>"),
        "element local=\"pre\"\ntext \"\\nx\"\nelement local=\"textarea\"\ntext \"y\""
    );
}

#[test]
fn selectedcontent_mirrors_the_selected_option_when_it_is_popped() {
    let mirror = |source: &str| {
        let result = parse(source);
        let doc = result.document();
        let target = find(doc, doc.root(), "selectedcontent");
        doc.dump_tree(target, 1 << 16)
            .unwrap()
            .lines()
            .skip(1)
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("|")
    };
    let button = "<select><button><selectedcontent></button>";
    assert_eq!(mirror(&format!("{button}<option>A<option>B")), "text \"A\"");
    assert_eq!(
        mirror(&format!("{button}<option>A<option selected>B</option>")),
        "text \"B\""
    );
    // The first non-disabled option is selected by default.
    assert_eq!(
        mirror(&format!("{button}<option disabled>A<option>B")),
        "text \"B\""
    );
    // A multiple select has no enabled selectedcontent.
    assert_eq!(
        mirror("<select multiple><button><selectedcontent></button><option>A"),
        ""
    );
    // Clones are deep and independent of the option's own nodes.
    let result = parse(&format!("{button}<option><b>x</b>y</option>"));
    let doc = result.document();
    let target = find(doc, doc.root(), "selectedcontent");
    let option = find(doc, doc.root(), "option");
    let bold_in_target = find(doc, target, "b");
    assert_ne!(bold_in_target, find(doc, option, "b"));
    assert_eq!(doc.walk(target).unwrap().count(), 4);
}

#[test]
fn selectedcontent_clones_consume_the_document_budget() {
    let source = "<select><button><selectedcontent></button><option>A</option></select>";
    // html, head, body, select, button, selectedcontent, option, two texts and the root.
    let mut limits = ParserLimits::default();
    limits.document.max_nodes = 9;
    let mut parser = HtmlDocumentParser::new(limits, Default::default()).unwrap();
    let result = parser
        .feed(source.as_bytes())
        .and_then(|()| parser.finish().map(drop));
    assert_eq!(
        result,
        Err(rustic_html::ParseError::Document(
            rustic_dom::DomError::NodeLimit
        ))
    );
    limits.document.max_nodes = 10;
    let mut parser = HtmlDocumentParser::new(limits, Default::default()).unwrap();
    parser.feed(source.as_bytes()).unwrap();
    assert!(parser.finish().is_ok());
}
