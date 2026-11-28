//! Shadow trees in the cascade: per-tree author sheets, `:host`, context
//! precedence and inheritance into shadow trees (CSS-07b).

use rustic_css::{CssParser, NamespaceMap, SelectorErrorKind, Specificity};
use rustic_dom::{Document, NodeId, NodeKind, ShadowRootMode};
use rustic_html::HtmlDocumentParser;
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree};

fn parse(html: &str) -> Document {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap().into_document()
}

fn attribute(document: &Document, id: NodeId, name: &str) -> Option<String> {
    match document.node(id).ok()?.kind() {
        NodeKind::Element(element) => element
            .attributes()
            .iter()
            .find(|a| a.name().local() == name)
            .map(|a| String::from_utf16_lossy(a.value().as_units())),
        _ => None,
    }
}

/// Every element with an `id`, in this tree and nested shadow trees.
fn all_with_id(document: &Document, id: &str) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut roots = vec![document.root()];
    while let Some(root) = roots.pop() {
        for (node, _) in document.walk(root).unwrap() {
            if let NodeKind::Element(element) = document.node(node).unwrap().kind() {
                roots.extend(element.shadow_root());
            }
            if attribute(document, node, "id").as_deref() == Some(id) {
                found.push(node);
            }
        }
    }
    found
}

fn by_id(document: &Document, id: &str) -> NodeId {
    let found = all_with_id(document, id);
    assert_eq!(found.len(), 1, "#{id}");
    found[0]
}

/// Attach a shadow root to `#host` holding the `<head>` then `<body>`
/// children of `html` (a leading `<style>` lands in the head).
fn attach(document: &mut Document, host: &str, html: &str) {
    let host = by_id(document, host);
    let root = document.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let mut source = parse(html);
    let sections: Vec<NodeId> = source
        .walk(source.root())
        .unwrap()
        .map(|(id, _)| id)
        .filter(|&id| {
            matches!(source.node(id).unwrap().kind(),
                NodeKind::Element(e) if matches!(e.name().local(), "head" | "body"))
        })
        .collect();
    let mut children = Vec::new();
    for section in sections {
        children.extend(source.children(section).unwrap());
    }
    for child in children {
        let adopted = document.adopt_node(&mut source, child).unwrap();
        document.append_child(root, adopted.root).unwrap();
    }
}

/// `id -> non-initial specified values` for every element with an `id`.
fn styles(document: &Document) -> Vec<(String, String)> {
    let parser = CssParser::default();
    let (cascade, _) = document_cascade(document, &parser).unwrap();
    style_tree(
        document,
        document.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &ComputeContext::default(),
    )
    .unwrap()
    .into_iter()
    .filter_map(|s| {
        let id = attribute(document, s.node, "id")?;
        let mut dump = s.specified.dump_non_initial();
        // The UA sheet's block display and paragraph margins are noise here.
        for noise in [
            "display: block\n",
            "margin-top: 1em\n",
            "margin-bottom: 1em\n",
        ] {
            dump = dump.replace(noise, "");
        }
        Some((id, dump))
    })
    .collect()
}

fn style(styles: &[(String, String)], id: &str) -> String {
    styles
        .iter()
        .find(|(i, _)| i == id)
        .map(|(_, s)| s.clone())
        .unwrap_or_else(|| panic!("no #{id}"))
}

const RED: &str = "color: rgb(255, 0, 0)\n";
const GREEN: &str = "color: rgb(0, 128, 0)\n";
const BLUE: &str = "color: rgb(0, 0, 255)\n";

#[test]
fn shadow_sheets_and_document_sheets_style_only_their_own_tree() {
    let mut doc = parse(
        "<style>p { color: red } #host { color: blue }</style>\
         <div id=host><p id=light></p></div>",
    );
    attach(
        &mut doc,
        "host",
        "<style>p { color: green } div { width: 1px }</style><p id=inner></p><span id=plain></span><slot></slot>",
    );
    let got = styles(&doc);
    assert_eq!(style(&got, "light"), RED);
    assert_eq!(style(&got, "inner"), GREEN);
    // Top-level shadow elements inherit from the host; the shadow sheet's
    // `div` does not reach the host.
    assert_eq!(style(&got, "plain"), BLUE);
    assert_eq!(style(&got, "host"), BLUE);
}

#[test]
fn host_rules_lose_to_outer_normal_rules_and_win_when_important() {
    let mut doc = parse(
        "<style>#host { color: red; height: 2px !important }</style>\
         <div id=host style='margin-top: 3px'></div>",
    );
    attach(
        &mut doc,
        "host",
        "<style>:host { color: green; width: 5px; height: 1px !important; margin-top: 9px }</style>",
    );
    assert_eq!(
        style(&styles(&doc), "host"),
        "color: rgb(255, 0, 0)\nmargin-top: 3px\nwidth: 5px\nheight: 1px\n"
    );
}

#[test]
fn the_host_is_featureless_inside_its_shadow_tree() {
    let mut doc = parse(
        "<style>:host { color: red } div :host { color: red }</style>\
         <section><div id=host class=on></div><div id=off></div></section>",
    );
    let shadow = "<style>\
         * { width: 1px } div { width: 2px } .on { width: 3px } :not(.x) { width: 4px } \
         section div, section p { color: red } \
         :host(.on) { color: green } :host(.missing) { color: red } \
         :is(:host) { margin-top: 1px } \
         :host > p { height: 7px } :host(.on) p { margin-left: 2px } \
         div p { color: red } \
         </style><p id=inner></p>";
    attach(&mut doc, "host", shadow);
    attach(&mut doc, "off", &shadow.replace("id=inner", "id=inner-off"));
    let got = styles(&doc);
    assert_eq!(
        style(&got, "host"),
        "color: rgb(0, 128, 0)\nmargin-top: 1px\n"
    );
    assert_eq!(style(&got, "off"), "margin-top: 1px\n");
    // `*`, `:not(.x)` and `:host > p` all match inside; `div p` does not
    // see past the host, and neither does `section p`.
    assert_eq!(
        style(&got, "inner"),
        "color: rgb(0, 128, 0)\nmargin-left: 2px\nwidth: 4px\nheight: 7px\n"
    );
    assert_eq!(style(&got, "inner-off"), "width: 4px\nheight: 7px\n");
}

#[test]
fn each_shadow_tree_has_its_own_layer_order_and_context_depth() {
    let mut doc = parse(
        "<style>@layer b, a; #outer { color: red }</style>\
         <div id=outer></div>",
    );
    attach(
        &mut doc,
        "outer",
        "<style>@layer a { p { color: red } } @layer b { p { color: green } } \
         :host { color: green !important } #mid { color: green }</style>\
         <p id=layered></p><div id=mid></div>",
    );
    attach(
        &mut doc,
        "mid",
        "<style>:host { color: blue !important }</style><span id=deep></span>",
    );
    let got = styles(&doc);
    assert_eq!(style(&got, "layered"), GREEN);
    // Inner important beats outer important, at every depth.
    assert_eq!(style(&got, "outer"), GREEN);
    assert_eq!(style(&got, "mid"), BLUE);
    assert_eq!(style(&got, "deep"), BLUE);
}

#[test]
fn host_selectors_parse_and_serialize() {
    let parser = CssParser::default();
    let parse = |s: &str| parser.selector_list(s, &NamespaceMap::new()).unwrap().value;
    let list = parse(":host, :HOST(div.a), :host(:not(.b)) > p").unwrap();
    assert_eq!(list.to_string(), ":host, :host(div.a), :host(:not(.b)) > p");
    let specificities: Vec<(u32, u32, u32)> = list
        .selectors()
        .iter()
        .map(|s| s.specificity())
        .map(
            |Specificity {
                 ids,
                 classes,
                 types,
             }| (ids, classes, types),
        )
        .collect();
    assert_eq!(specificities, [(0, 1, 0), (0, 2, 1), (0, 2, 1)]);
    for bad in [
        ":host()",
        ":host(a b)",
        ":host(a, b)",
        ":host(a > b)",
        ":host(::before)",
    ] {
        assert_eq!(
            parse(bad).unwrap_err().kind,
            SelectorErrorKind::InvalidHostArgument,
            "{bad}"
        );
    }
}

#[test]
fn shadow_trees_without_style_inherit_from_their_host() {
    let mut doc = parse("<style>#host { color: blue; font-size: 20px }</style><div id=host></div>");
    attach(&mut doc, "host", "<p><span id=deep></span></p>");
    let got = styles(&doc);
    assert_eq!(
        style(&got, "deep"),
        "color: rgb(0, 0, 255)\nfont-size: 20px\n"
    );
    let _ = (RED, GREEN);
}

#[test]
fn slotted_elements_inherit_from_the_slot_and_match_their_own_tree() {
    let doc = parse(
        "<style>div > span { width: 5px }</style>\
        <div style='color: red'>\
        <template shadowrootmode=open><style>slot { color: blue } span { width: 9px }</style><slot name=x></slot></template>\
        <span id=light slot=x></span><span id=hidden></span></div>",
    );
    let got = styles(&doc);
    assert_eq!(style(&got, "light"), "color: rgb(0, 0, 255)\nwidth: 5px\n");
    assert!(!got.iter().any(|(id, _)| id == "hidden"));
}
