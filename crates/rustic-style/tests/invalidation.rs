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

#[test]
fn mutations_of_scope_roots_and_limits_restyle_existing_subjects() {
    let mut page = Page::new(
        "<main id=root class=card><div id=limit><p id=subject>text</p></div></main>",
        "@scope (.card) to (.closed) { p { color: red } }",
    );
    assert_eq!(page.color("subject"), "rgb(255, 0, 0)");
    page.set("limit", "class", "closed");
    page.restyle();
    assert_eq!(page.color("subject"), "rgb(0, 0, 0)");
    page.remove("limit", "class");
    page.restyle();
    assert_eq!(page.color("subject"), "rgb(255, 0, 0)");
    page.remove("root", "class");
    page.restyle();
    assert_eq!(page.color("subject"), "rgb(0, 0, 0)");
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

/// A page with `n` filler paragraphs after `html`, so that a full restyle
/// is easy to tell apart from a targeted one.
fn padded(html: &str) -> String {
    format!(
        "<!doctype html><body>{html}{}",
        "<p class=filler>filler</p>".repeat(40)
    )
}

impl Page {
    fn width(&self, id: &str) -> String {
        let node = by_id(&self.document, id);
        self.styled
            .style(node)
            .unwrap()
            .computed
            .get(rustic_css::PropertyId::Width)
            .to_string()
    }

    fn append(&mut self, parent: &str, tag: &str, attributes: &[(&str, &str)]) -> NodeId {
        let parent = by_id(&self.document, parent);
        let child = self
            .document
            .create_element(QualifiedName::html(tag).unwrap())
            .unwrap();
        for (local, value) in attributes {
            self.document
                .set_attribute(child, attribute(local), *value)
                .unwrap();
        }
        self.document.append_child(parent, child).unwrap();
        self.styled
            .note(&self.document, Mutation::Children { parent });
        child
    }

    fn set_text(&mut self, id: &str, text: &str) {
        let element = by_id(&self.document, id);
        let node = self.document.children(element).unwrap().next().unwrap();
        self.document.set_character_data(node, text).unwrap();
        self.styled.note(&self.document, Mutation::Text { node });
    }
}

/// Fewer than the 40 fillers: the restyle was targeted.
fn targeted(stats: RestyleStats) {
    assert!(stats.recomputed < 20, "{stats:?}");
}

#[test]
fn has_restyles_the_anchors_not_the_document() {
    let html = padded("<div id=d><section id=s><span id=t></span></section></div>");
    let mut page = Page::new(&html, "div:has(.on) { color: red }");
    page.set("t", "class", "on");
    targeted(page.restyle());
    assert_eq!(page.color("d"), "rgb(255, 0, 0)");
    page.remove("t", "class");
    targeted(page.restyle());
    assert_eq!(page.color("d"), "rgb(0, 0, 0)");

    // Inserting and removing a matching element.
    let span = page.append("s", "span", &[("class", "on")]);
    targeted(page.restyle());
    assert_eq!(page.color("d"), "rgb(255, 0, 0)");
    let section = by_id(&page.document, "s");
    page.document.detach(span).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: section });
    targeted(page.restyle());
    assert_eq!(page.color("d"), "rgb(0, 0, 0)");

    // `:empty` inside the argument depends on text.
    let html = padded("<div id=d><p id=p>x</p></div>");
    let mut page = Page::new(&html, "div:has(> p:empty) { color: red }");
    page.set_text("p", "");
    targeted(page.restyle());
    assert_eq!(page.color("d"), "rgb(255, 0, 0)");
}

#[test]
fn has_with_siblings_reaches_preceding_siblings() {
    let html = padded("<ul><li id=a></li><li id=b></li><li id=c><b id=x></b></li></ul>");
    let mut page = Page::new(
        &html,
        "li:has(+ .on) { color: red } li:has(~ li .on) { width: 3px }",
    );
    page.set("b", "class", "on");
    targeted(page.restyle());
    assert_eq!(page.color("a"), "rgb(255, 0, 0)");
    assert_eq!(page.color("b"), "rgb(0, 0, 0)");
    page.set("x", "class", "on");
    targeted(page.restyle());
    assert_eq!(page.width("a"), "3px");
    assert_eq!(page.width("b"), "3px");
    assert_eq!(page.width("c"), "auto");

    // Removing a sibling changes what follows the earlier ones.
    let b = by_id(&page.document, "b");
    let list = page.document.node(b).unwrap().parent().unwrap();
    page.document.detach(b).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: list });
    targeted(page.restyle());
    assert_eq!(page.color("a"), "rgb(0, 0, 0)");
}

#[test]
fn has_outside_the_subject_restyles_anchor_subtrees() {
    let html = "<!doctype html><div id=d><p id=p></p><i id=i></i></div><p id=q></p>";
    let mut page = Page::new(html, "div:has(.on) p, div:has(.on) + p { color: red }");
    page.set("i", "class", "on");
    page.restyle();
    assert_eq!(page.color("p"), "rgb(255, 0, 0)");
    assert_eq!(page.color("q"), "rgb(255, 0, 0)");
}

#[test]
fn has_with_nested_complex_arguments_looks_above_the_anchor() {
    let html = padded("<section id=sec><div id=d><span class=t></span></div></section>");
    let mut page = Page::new(&html, "div:has(:is(.on .t)) { color: red }");
    page.set("sec", "class", "on");
    targeted(page.restyle());
    assert_eq!(page.color("d"), "rgb(255, 0, 0)");
}

#[test]
fn radio_groups_and_default_buttons_restyle_their_controls() {
    let html = padded(
        "<form id=f><input id=r1 type=radio name=g checked><input id=r2 type=radio name=g>\
         <input id=c type=checkbox><button id=b1>go</button></form>",
    );
    let mut page = Page::new(
        &html,
        "input:checked { width: 7px } :default { color: red } input:checked + input { height: 2px }",
    );
    assert_eq!(page.width("r1"), "7px");
    page.set("r2", "checked", "");
    // Checking r2 unchecks r1.
    targeted(page.restyle());
    assert_eq!(page.width("r1"), "auto");
    assert_eq!(page.width("r2"), "7px");
    page.set("r1", "name", "h");
    targeted(page.restyle());
    assert_eq!(page.width("r1"), "7px");

    // A submit button inserted before the default button takes over.
    let form = by_id(&page.document, "f");
    let first = page
        .document
        .create_element(QualifiedName::html("button").unwrap())
        .unwrap();
    page.document
        .set_attribute(first, attribute("id"), "b0")
        .unwrap();
    let r1 = by_id(&page.document, "r1");
    page.document.insert_before(form, first, Some(r1)).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: form });
    targeted(page.restyle());
    assert_eq!(page.color("b0"), "rgb(255, 0, 0)");
    assert_eq!(page.color("b1"), "rgb(0, 0, 0)");
}

#[test]
fn fieldsets_and_editing_hosts_restyle_their_subtrees() {
    let html = padded(
        "<fieldset id=f><legend><input id=l></legend><div><input id=i></div></fieldset>\
         <div id=e><span id=s></span></div>",
    );
    let mut page = Page::new(
        &html,
        "input:disabled { color: gray } :read-write { width: 1px }",
    );
    page.set("f", "disabled", "");
    targeted(page.restyle());
    assert_eq!(page.color("i"), "rgb(128, 128, 128)");
    assert_eq!(page.color("l"), "rgb(0, 0, 0)");
    page.set("e", "contenteditable", "");
    targeted(page.restyle());
    assert_eq!(page.width("s"), "1px");

    let html = padded("<textarea id=t placeholder=hint>x</textarea>");
    let mut page = Page::new(&html, ":placeholder-shown { color: red }");
    page.set_text("t", "");
    targeted(page.restyle());
    assert_eq!(page.color("t"), "rgb(255, 0, 0)");
}

#[test]
fn language_and_direction_restyle_what_they_reach() {
    let html = padded(
        "<div id=d><p id=p>x</p></div><div id=auto dir=auto><span id=s>abc</span><b id=b></b></div>",
    );
    let mut page = Page::new(&html, ":lang(de) { color: red } :dir(rtl) { width: 5px }");
    page.set("d", "lang", "de");
    targeted(page.restyle());
    assert_eq!(page.color("p"), "rgb(255, 0, 0)");

    // dir=auto follows the first strong character below it.
    page.set_text("s", "\u{5d0}");
    targeted(page.restyle());
    assert_eq!(page.width("b"), "5px");
    page.set("s", "dir", "ltr");
    // The span's text no longer counts for the auto ancestor.
    targeted(page.restyle());
    assert_eq!(page.width("b"), "auto");

    // A pragma-set default language applies everywhere.
    let head = page
        .document
        .walk(page.document.root())
        .unwrap()
        .map(|(node, _)| node)
        .find(|&node| match page.document.node(node).unwrap().kind() {
            NodeKind::Element(e) => e.name().local() == "head",
            _ => false,
        })
        .unwrap();
    let meta = page
        .document
        .create_element(QualifiedName::html("meta").unwrap())
        .unwrap();
    page.document
        .set_attribute(meta, attribute("http-equiv"), "content-language")
        .unwrap();
    page.document
        .set_attribute(meta, attribute("content"), "de")
        .unwrap();
    page.document.append_child(head, meta).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent: head });
    page.restyle();
    assert_eq!(page.color("b"), "rgb(255, 0, 0)");
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

#[test]
fn custom_property_changes_flow_to_var_users() {
    let mut page = Page::new(
        LIST,
        ".dark { --fg: red } .loop { --fg: var(--bg); --bg: var(--fg) } li { color: var(--fg, blue) }",
    );
    assert_eq!(page.color("a"), "rgb(0, 0, 255)");
    page.set("outer", "class", "dark");
    page.restyle();
    assert_eq!(page.color("b"), "rgb(255, 0, 0)");
    page.set("list", "class", "loop");
    page.restyle();
    assert_eq!(page.color("c"), "rgb(0, 0, 255)");
    page.set("b", "style", "--fg: green");
    assert_eq!(page.restyle().recomputed, 1);
    assert_eq!(page.color("b"), "rgb(0, 128, 0)");
}

#[test]
fn structural_ancestor_changes_restyle_descendant_subjects() {
    let mut page = Page::new(
        "<main id=m><section id=a><b id=x></b></section><section id=b><b id=y></b></section></main>",
        "section:first-child b { width: 7px } section:nth-child(2) b { color: red }",
    );
    let parent = by_id(&page.document, "m");
    let first = by_id(&page.document, "a");
    page.document.detach(first).unwrap();
    page.styled
        .note(&page.document, Mutation::Children { parent });
    page.restyle();
}

#[test]
fn nth_filter_attribute_changes_restyle_other_counted_siblings() {
    let mut page = Page::new(
        "<main><p id=a class=hot></p><p id=b class=hot></p><p id=c class=hot></p></main>",
        "p:nth-child(2 of .hot) { width: 7px }",
    );
    page.remove("a", "class");
    page.restyle();
}

#[test]
fn empty_ancestor_sibling_subjects_change_after_text_and_child_list_edits() {
    for child_list in [false, true] {
        let mut page = Page::new(
            "<main><div id=a>x</div><section><b id=b></b></section></main>",
            "div:empty + section b { width: 7px }",
        );
        let parent = by_id(&page.document, "a");
        let text = page.document.children(parent).unwrap().next().unwrap();
        if child_list {
            page.document.detach(text).unwrap();
            page.styled
                .note(&page.document, Mutation::Children { parent });
        } else {
            page.document.set_character_data(text, "").unwrap();
            page.styled
                .note(&page.document, Mutation::Text { node: text });
        }
        page.restyle();
    }
}

#[test]
fn parser_associated_controls_use_the_form_element_pointer() {
    // The button is not the form's descendant: the parser's form element
    // pointer associates it, so it is the form's default button (TD-31).
    let page = Page::new(
        "<table><form id=f><tr><td><button id=b>go</button></td></tr></form></table>\
         <button id=loose>no form</button>",
        ":default { color: red }",
    );
    let form = by_id(&page.document, "f");
    let button = by_id(&page.document, "b");
    assert_ne!(page.document.node(button).unwrap().parent(), Some(form));
    assert_eq!(page.document.parser_form(button), Some(form));
    assert_eq!(page.color("b"), "rgb(255, 0, 0)");
    assert_eq!(page.color("loose"), "rgb(0, 0, 0)");
}
