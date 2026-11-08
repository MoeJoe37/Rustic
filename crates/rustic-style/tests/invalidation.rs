//! Incremental restyling after DOM mutations always equals a full restyle,
//! and recomputes only what the mutation can affect (CSS-06).

use rustic_css::CssParser;
use rustic_dom::{Document, NodeId, NodeKind, QualifiedName};
use rustic_html::HtmlDocumentParser;
use rustic_style::{
    Cascade, ComputeContext, MatchContext, Mutation, Origin, RestyleStats, StyleSheet,
    StyledDocument, document_cascade,
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

fn attribute(local: &str) -> QualifiedName {
    QualifiedName::from_parsed(None, None, local).unwrap()
}

/// A document, its cascade (UA sheet, `<style>` elements, plus `author`)
/// and incremental styles.
struct Page {
    document: Document,
    cascade: Cascade,
    styled: StyledDocument,
}

impl Page {
    fn new(html: &str, author: &str) -> Self {
        let document = document(html);
        let parser = CssParser::default();
        let (mut cascade, _) = document_cascade(&document, &parser).unwrap();
        let (sheet, diagnostics) = StyleSheet::parse(&parser, author, Origin::Author).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        cascade.push(sheet);
        let styled = StyledDocument::new(
            &document,
            &cascade,
            &parser,
            &MatchContext::html(),
            &ComputeContext::default(),
        )
        .unwrap();
        Self {
            document,
            cascade,
            styled,
        }
    }

    /// Restyle and check against a full restyle.
    fn restyle(&mut self) -> RestyleStats {
        let parser = CssParser::default();
        let cx = MatchContext::html();
        let compute = ComputeContext::default();
        let stats = self
            .styled
            .restyle(&self.document, &self.cascade, &parser, &cx, &compute)
            .unwrap();
        let full = rustic_style::style_tree(
            &self.document,
            self.document.root(),
            &self.cascade,
            &parser,
            &cx,
            &compute,
        )
        .unwrap();
        assert_eq!(
            self.styled.styles().len(),
            full.len(),
            "incremental styles differ from a full restyle"
        );
        for (a, b) in self.styled.styles().iter().zip(&full) {
            assert_eq!(a, b, "incremental styles differ from a full restyle");
        }
        assert!(
            self.styled
                .matches_full_restyle(&self.document, &self.cascade, &parser, &cx, &compute)
                .unwrap()
        );
        stats
    }

    fn set(&mut self, id: &str, local: &str, value: &str) {
        let element = by_id(&self.document, id);
        self.document
            .set_attribute(element, attribute(local), value)
            .unwrap();
        self.styled.note(
            &self.document,
            Mutation::Attribute {
                element,
                local: local.into(),
            },
        );
    }

    fn remove(&mut self, id: &str, local: &str) {
        let element = by_id(&self.document, id);
        self.document
            .remove_attribute(element, None, local)
            .unwrap();
        self.styled.note(
            &self.document,
            Mutation::Attribute {
                element,
                local: local.into(),
            },
        );
    }

    fn color(&self, id: &str) -> String {
        let node = by_id(&self.document, id);
        self.styled
            .style(node)
            .unwrap()
            .computed
            .get(rustic_css::PropertyId::Color)
            .to_string()
    }
}

const LIST: &str = "<!doctype html><body><div id=outer><ul id=list>\
    <li id=a>a</li><li id=b>b</li><li id=c>c</li></ul><p id=p>text</p></div>";

#[test]
fn unrelated_attributes_restyle_nothing_and_style_restyles_one() {
    let mut page = Page::new(LIST, ".hot { color: red }");
    assert!(!page.styled.needs_restyle());
    page.set("b", "data-x", "1");
    assert!(!page.styled.needs_restyle());
    assert_eq!(page.restyle().recomputed, 0);
    page.set("b", "style", "width: 3px");
    let stats = page.restyle();
    // The element only: its computed style changed, but `li` has no element
    // children.
    assert_eq!(stats.recomputed, 1);
}

#[test]
fn subject_classes_restyle_the_element_and_ancestor_classes_the_subtree() {
    let mut page = Page::new(LIST, ".hot { color: red }");
    page.set("b", "class", "hot");
    assert_eq!(page.restyle().recomputed, 1);
    assert_eq!(page.color("b"), "rgb(255, 0, 0)");
    page.remove("b", "class");
    page.restyle();
    assert_eq!(page.color("b"), "rgb(0, 0, 0)");

    let mut page = Page::new(LIST, ".hot li { width: 9px }");
    page.set("list", "class", "hot");
    // The list and its three items.
    assert_eq!(page.restyle().recomputed, 4);
    page.set("outer", "class", "hot");
    page.restyle();
}

#[test]
fn inherited_changes_flow_down_and_stop_where_nothing_changed() {
    let mut page = Page::new(LIST, "#outer.big { font-size: 30px } .quiet { width: 1px }");
    page.set("outer", "class", "big");
    // outer, list, three items, and p all inherit the font size.
    assert_eq!(page.restyle().recomputed, 6);
    page.set("outer", "class", "big quiet");
    // Width is not inherited: the direct children are checked (one of them
    // could say `width: inherit`) but keep equal styles, so the
    // grandchildren are reused.
    assert_eq!(page.restyle().recomputed, 3);
}

#[test]
fn sibling_combinators_restyle_following_siblings() {
    let mut page = Page::new(LIST, ".hot + li { color: blue } .hot ~ p { color: red }");
    page.set("a", "class", "hot");
    page.restyle();
    assert_eq!(page.color("b"), "rgb(0, 0, 255)");
    assert_eq!(page.color("c"), "rgb(0, 0, 0)");
    page.set("list", "class", "hot");
    page.restyle();
    assert_eq!(page.color("p"), "rgb(255, 0, 0)");
}

#[test]
fn inserted_removed_and_moved_elements() {
    let mut page = Page::new(LIST, "ul > li { width: 2px } div li { height: 3px }");
    let list = by_id(&page.document, "list");
    let item = page
        .document
        .create_element(QualifiedName::html("li").unwrap())
        .unwrap();
    page.document.append_child(list, item).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: list });
    // Only the new element: no structural selectors.
    assert_eq!(page.restyle().recomputed, 1);
    assert!(page.styled.style(item).is_some());

    // Move `b` out of the list into `p`; it is restyled with the new parent.
    let b = by_id(&page.document, "b");
    let p = by_id(&page.document, "p");
    page.document.append_child(p, b).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: list });
    page.styled
        .note(&page.document, Mutation::Children { parent: p });
    assert_eq!(page.restyle().recomputed, 1);

    page.document.detach(b).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: p });
    page.restyle();
    assert!(page.styled.style(b).is_none());
}

#[test]
fn structural_selectors_restyle_siblings_and_parent() {
    let mut page = Page::new(
        LIST,
        "li:nth-child(odd) { color: red } li:last-child { width: 1px } ul:empty { height: 5px }",
    );
    let list = by_id(&page.document, "list");
    let a = by_id(&page.document, "a");
    page.document.detach(a).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: list });
    page.restyle();
    assert_eq!(page.color("b"), "rgb(255, 0, 0)");
    for id in ["b", "c"] {
        let node = by_id(&page.document, id);
        page.document.detach(node).unwrap();
        page.styled
            .note(&page.document, Mutation::Children { parent: list });
        page.restyle();
    }
    let ul = page.styled.style(list).unwrap();
    assert_eq!(
        ul.computed.get(rustic_css::PropertyId::Height).to_string(),
        "5px"
    );
}

#[test]
fn text_changes_matter_for_empty() {
    let mut page = Page::new(
        "<!doctype html><p id=p>x</p><p id=q></p>",
        "p:empty { color: red }",
    );
    let p = by_id(&page.document, "p");
    let text = page.document.children(p).unwrap().next().unwrap();
    page.document.set_character_data(text, "").unwrap();
    page.styled
        .note(&page.document, Mutation::Text { node: text });
    page.restyle();
    assert_eq!(page.color("p"), "rgb(255, 0, 0)");
}

#[test]
fn element_states_and_has_restyle_conservatively() {
    let mut page = Page::new(
        "<!doctype html><form><input id=i type=checkbox><input id=r1 type=radio name=g checked>\
         <input id=r2 type=radio name=g></form><div id=d><span id=s></span></div>",
        "input:checked { width: 7px } input:disabled { color: gray } div:has(.on) { color: red }",
    );
    page.set("i", "checked", "");
    page.restyle();
    page.set("r2", "checked", "");
    // Checking r2 unchecks r1: a whole-tree fact.
    page.restyle();
    page.set("i", "disabled", "");
    page.restyle();
    assert_eq!(page.color("i"), "rgb(128, 128, 128)");
    page.set("s", "class", "on");
    page.restyle();
    assert_eq!(page.color("d"), "rgb(255, 0, 0)");
    page.remove("s", "class");
    page.restyle();
    assert_eq!(page.color("d"), "rgb(0, 0, 0)");
}

#[test]
fn root_changes_restyle_everything_for_rem() {
    let mut page = Page::new(
        "<!doctype html><html id=root><body><p id=p>",
        ".big { font-size: 20px } p { width: 2rem }",
    );
    page.set("root", "class", "big");
    let stats = page.restyle();
    assert_eq!(stats.reused, 0);
    let p = by_id(&page.document, "p");
    assert_eq!(
        page.styled
            .style(p)
            .unwrap()
            .computed
            .get(rustic_css::PropertyId::Width)
            .to_string(),
        "40px"
    );
}

#[test]
fn environment_changes_restyle_through_the_cascade() {
    let mut page = Page::new(LIST, "@media (max-width: 600px) { li { color: red } }");
    let changed = page
        .cascade
        .set_environment(rustic_css::MediaEnvironment::screen(400.0, 800.0));
    assert!(changed);
    page.styled.cascade_changed(&page.cascade);
    page.restyle();
    assert_eq!(page.color("a"), "rgb(255, 0, 0)");
}
