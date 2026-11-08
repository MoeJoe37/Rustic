//! DOM conformance gate for node insertion (DOC-09).
//!
//! Every (parent, node, reference child) combination over a fixture holding each
//! node kind, connected and detached trees, and template contents is checked
//! against an independent oracle transcribed from the DOM Standard's "ensure
//! pre-insert validity" and "pre-insert" algorithms
//! (<https://dom.spec.whatwg.org/#concept-node-ensure-pre-insertion-validity>,
//! reviewed 2026-11-08 against the living standard). The gate checks the error
//! class and step precedence, the resulting child order, that rejected calls
//! leave every tree unchanged, and the structural invariants after each call.
//!
//! Engine limits (depth, nodes) are not part of the spec and are tested
//! elsewhere; the fixture stays far below them.

use rustic_dom::{
    Document, DocumentLimits, DocumentType, DomError, DomString, NodeId, NodeKind, QualifiedName,
};

/// The DOMException a binding must throw for a failed pre-insert.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Inserted,
    HierarchyRequest,
    NotFound,
}

fn class(result: Result<(), DomError>) -> Outcome {
    match result {
        Ok(()) => Outcome::Inserted,
        Err(DomError::InvalidHierarchy | DomError::Cycle | DomError::ProtectedRoot) => {
            Outcome::HierarchyRequest
        }
        Err(DomError::InvalidReference) => Outcome::NotFound,
        Err(other) => panic!("unexpected non-spec error {other:?}"),
    }
}

struct Fixture {
    doc: Document,
    /// Named handles in a fixed order, used for enumeration and messages.
    nodes: Vec<(&'static str, NodeId)>,
    /// (template contents fragment, template host).
    hosts: Vec<(NodeId, NodeId)>,
}

fn html(doc: &mut Document, name: &str) -> NodeId {
    doc.create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

fn doctype(doc: &mut Document) -> NodeId {
    doc.create_doctype(DocumentType {
        name: "html".into(),
        public_id: DomString::default(),
        system_id: DomString::default(),
    })
    .unwrap()
}

/// `#document > [pi, doctype, html > [head, text, template(#contents > em), comment], comment]`
/// plus detached `div > [text, span]`, a detached text, comment, doctype,
/// processing instruction, CDATA section and empty element, and four
/// free-standing fragments: empty, one element, text + element, two elements.
fn fixture() -> Fixture {
    let mut doc = Document::new(DocumentLimits::default()).unwrap();
    let root = doc.root();
    let dt = doctype(&mut doc);
    let html_el = html(&mut doc, "html");
    let head = html(&mut doc, "head");
    let text = doc.create_text("t").unwrap();
    let template = html(&mut doc, "template");
    let contents = doc.create_template_contents(template).unwrap();
    let em = html(&mut doc, "em");
    let inner_comment = doc.create_comment("c").unwrap();
    let outer_comment = doc.create_comment("tail").unwrap();
    let div = html(&mut doc, "div");
    let div_text = doc.create_text("d").unwrap();
    let span = html(&mut doc, "span");
    let loose_text = doc.create_text("x").unwrap();
    let loose_comment = doc.create_comment("y").unwrap();
    let loose_doctype = doctype(&mut doc);
    let empty = html(&mut doc, "p");
    let doc_pi = doc.create_processing_instruction("pi", "a").unwrap();
    let loose_pi = doc.create_processing_instruction("pi", "b").unwrap();
    let cdata = doc.create_cdata_section("c").unwrap();
    let frag_empty = doc.create_document_fragment().unwrap();
    let frag_one = doc.create_document_fragment().unwrap();
    let frag_one_el = html(&mut doc, "i");
    let frag_text = doc.create_document_fragment().unwrap();
    let frag_text_t = doc.create_text("ft").unwrap();
    let frag_text_el = html(&mut doc, "b");
    let frag_two = doc.create_document_fragment().unwrap();
    let frag_two_a = html(&mut doc, "u");
    let frag_two_b = html(&mut doc, "s");
    for (parent, child) in [
        (root, doc_pi),
        (root, dt),
        (root, html_el),
        (root, outer_comment),
        (html_el, head),
        (html_el, text),
        (html_el, template),
        (html_el, inner_comment),
        (contents, em),
        (div, div_text),
        (div, span),
        (frag_one, frag_one_el),
        (frag_text, frag_text_t),
        (frag_text, frag_text_el),
        (frag_two, frag_two_a),
        (frag_two, frag_two_b),
    ] {
        doc.append_child(parent, child).unwrap();
    }
    Fixture {
        nodes: vec![
            ("#document", root),
            ("doctype", dt),
            ("html", html_el),
            ("head", head),
            ("text", text),
            ("template", template),
            ("#contents", contents),
            ("em", em),
            ("inner-comment", inner_comment),
            ("outer-comment", outer_comment),
            ("div", div),
            ("div-text", div_text),
            ("span", span),
            ("loose-text", loose_text),
            ("loose-comment", loose_comment),
            ("loose-doctype", loose_doctype),
            ("p", empty),
            ("doc-pi", doc_pi),
            ("loose-pi", loose_pi),
            ("cdata", cdata),
            ("frag-empty", frag_empty),
            ("frag-one", frag_one),
            ("frag-one-i", frag_one_el),
            ("frag-text", frag_text),
            ("frag-text-t", frag_text_t),
            ("frag-text-b", frag_text_el),
            ("frag-two", frag_two),
            ("frag-two-u", frag_two_a),
            ("frag-two-s", frag_two_b),
        ],
        hosts: vec![(contents, template)],
        doc,
    }
}

/// Every named node's parent and children: the observable tree state.
fn snapshot(fixture: &Fixture) -> Vec<(Option<NodeId>, Vec<NodeId>)> {
    fixture
        .nodes
        .iter()
        .map(|(_, id)| {
            let node = fixture.doc.node(*id).unwrap();
            (node.parent(), fixture.doc.children(*id).unwrap().collect())
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Document,
    Fragment,
    DocumentType,
    Element,
    /// Text and CDATA sections (CDATASection inherits from Text).
    Text,
    /// Comments and processing instructions: other character data.
    OtherData,
}

fn kind(doc: &Document, id: NodeId) -> Kind {
    match doc.node(id).unwrap().kind() {
        NodeKind::Document => Kind::Document,
        NodeKind::DocumentFragment => Kind::Fragment,
        NodeKind::DocumentType(_) => Kind::DocumentType,
        NodeKind::Element(_) => Kind::Element,
        NodeKind::Text(_) | NodeKind::CdataSection(_) => Kind::Text,
        NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_) => Kind::OtherData,
    }
}

/// The spec's verdict, transcribed step by step from "ensure pre-insert validity".
fn oracle(fixture: &Fixture, parent: NodeId, node: NodeId, child: Option<NodeId>) -> Outcome {
    let doc = &fixture.doc;
    let children = |id| doc.children(id).unwrap().collect::<Vec<_>>();
    // 1. Parent must be a Document, DocumentFragment or Element.
    if !matches!(
        kind(doc, parent),
        Kind::Document | Kind::Fragment | Kind::Element
    ) {
        return Outcome::HierarchyRequest;
    }
    // 2. Node must not be a host-including inclusive ancestor of parent.
    let mut ancestor = Some(parent);
    while let Some(id) = ancestor {
        if id == node {
            return Outcome::HierarchyRequest;
        }
        ancestor = doc.node(id).unwrap().parent().or_else(|| {
            fixture
                .hosts
                .iter()
                .find(|(contents, _)| *contents == id)
                .map(|(_, host)| *host)
        });
    }
    // 3. A reference child must be a child of parent.
    if let Some(child) = child {
        if doc.node(child).unwrap().parent() != Some(parent) {
            return Outcome::NotFound;
        }
    }
    // 4. Node must be a DocumentFragment, DocumentType, Element or CharacterData.
    let node_kind = kind(doc, node);
    if node_kind == Kind::Document {
        return Outcome::HierarchyRequest;
    }
    // 5. Text cannot go in a document; a doctype only goes in a document.
    let parent_is_document = kind(doc, parent) == Kind::Document;
    if (node_kind == Kind::Text && parent_is_document)
        || (node_kind == Kind::DocumentType && !parent_is_document)
    {
        return Outcome::HierarchyRequest;
    }
    // 6. Document child constraints.
    if parent_is_document {
        let siblings = children(parent);
        let has = |wanted: Kind| siblings.iter().any(|id| kind(doc, *id) == wanted);
        let position = child.map(|child| siblings.iter().position(|id| *id == child).unwrap());
        let doctype_following = position.is_some_and(|index| {
            siblings[index..]
                .iter()
                .any(|id| kind(doc, *id) == Kind::DocumentType)
        });
        let element_preceding = position.is_some_and(|index| {
            siblings[..index]
                .iter()
                .any(|id| kind(doc, *id) == Kind::Element)
        });
        let child_is_doctype = child.is_some_and(|child| kind(doc, child) == Kind::DocumentType);
        let invalid = match node_kind {
            Kind::Fragment => {
                let inner = children(node);
                let elements = inner
                    .iter()
                    .filter(|id| kind(doc, **id) == Kind::Element)
                    .count();
                let text = inner.iter().any(|id| kind(doc, *id) == Kind::Text);
                elements > 1
                    || text
                    || (elements == 1
                        && (has(Kind::Element) || child_is_doctype || doctype_following))
            }
            Kind::Element => has(Kind::Element) || child_is_doctype || doctype_following,
            Kind::DocumentType => {
                has(Kind::DocumentType)
                    || element_preceding
                    || (child.is_none() && has(Kind::Element))
            }
            _ => false,
        };
        if invalid {
            return Outcome::HierarchyRequest;
        }
    }
    Outcome::Inserted
}

/// The parent's children after a successful pre-insert, per the spec.
fn expected_children(
    fixture: &Fixture,
    parent: NodeId,
    node: NodeId,
    child: Option<NodeId>,
) -> Vec<NodeId> {
    let doc = &fixture.doc;
    let reference = if child == Some(node) {
        doc.node(node).unwrap().next_sibling()
    } else {
        child
    };
    let inserted: Vec<NodeId> = if kind(doc, node) == Kind::Fragment {
        doc.children(node).unwrap().collect()
    } else {
        vec![node]
    };
    let mut result: Vec<NodeId> = doc
        .children(parent)
        .unwrap()
        .filter(|id| !inserted.contains(id))
        .collect();
    let at = reference.map_or(result.len(), |reference| {
        result.iter().position(|id| *id == reference).unwrap()
    });
    result.splice(at..at, inserted);
    result
}

#[test]
fn every_pre_insert_combination_matches_the_dom_standard() {
    let names = fixture().nodes;
    let mut checked = 0usize;
    let mut mismatches = Vec::new();
    let mut outcomes = [0usize; 3];
    for parent_index in 0..names.len() {
        for node_index in 0..names.len() {
            for child_index in 0..=names.len() {
                let mut fixture = fixture();
                let parent = fixture.nodes[parent_index].1;
                let node = fixture.nodes[node_index].1;
                let child = fixture.nodes.get(child_index).map(|(_, id)| *id);
                let expected = oracle(&fixture, parent, node, child);
                let expected_order = (expected == Outcome::Inserted)
                    .then(|| expected_children(&fixture, parent, node, child));
                let before = snapshot(&fixture);
                let actual = class(fixture.doc.insert_before(parent, node, child));
                checked += 1;
                outcomes[expected as usize] += 1;
                let case = format!(
                    "insert {} into {} before {}",
                    names[node_index].0,
                    names[parent_index].0,
                    names.get(child_index).map_or("null", |(name, _)| name),
                );
                if actual != expected {
                    mismatches.push(format!("{case}: expected {expected:?}, got {actual:?}"));
                    continue;
                }
                fixture
                    .doc
                    .check_invariants()
                    .unwrap_or_else(|error| panic!("{case}: {error}"));
                match expected_order {
                    Some(order) => assert_eq!(
                        fixture.doc.children(parent).unwrap().collect::<Vec<_>>(),
                        order,
                        "{case}"
                    ),
                    None => assert_eq!(snapshot(&fixture), before, "{case}: rejected but changed"),
                }
            }
        }
    }
    assert_eq!(checked, 29 * 29 * 30);
    // Each outcome is well represented, so the gate is not vacuous.
    assert!(outcomes.iter().all(|count| *count > 150), "{outcomes:?}");
    assert!(
        mismatches.is_empty(),
        "{} of {checked} cases differ from the DOM Standard:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn named_spec_examples_have_the_expected_exception() {
    let mut f = fixture();
    let get = |f: &Fixture, name: &str| f.nodes.iter().find(|(n, _)| *n == name).unwrap().1;
    let (root, html_el, template, contents, div, span, loose_text, loose_doctype) = (
        get(&f, "#document"),
        get(&f, "html"),
        get(&f, "template"),
        get(&f, "#contents"),
        get(&f, "div"),
        get(&f, "span"),
        get(&f, "loose-text"),
        get(&f, "loose-doctype"),
    );
    let doc = &mut f.doc;
    // A second document element, text in a document, a doctype in an element.
    assert_eq!(
        class(doc.append_child(root, div)),
        Outcome::HierarchyRequest
    );
    assert_eq!(
        class(doc.append_child(root, loose_text)),
        Outcome::HierarchyRequest
    );
    assert_eq!(
        class(doc.append_child(div, loose_doctype)),
        Outcome::HierarchyRequest
    );
    // A template cannot go inside its own contents (host-including ancestor).
    assert_eq!(
        class(doc.append_child(contents, template)),
        Outcome::HierarchyRequest
    );
    assert_eq!(
        class(doc.append_child(contents, html_el)),
        Outcome::HierarchyRequest
    );
    // Step 2 (ancestor) is checked before step 3 (reference child).
    assert_eq!(
        class(doc.insert_before(span, div, Some(html_el))),
        Outcome::HierarchyRequest
    );
    // A reference child of another parent is NotFound.
    assert_eq!(
        class(doc.insert_before(div, loose_text, Some(html_el))),
        Outcome::NotFound
    );
    // Moving the existing document element within the document is rejected.
    assert_eq!(
        class(doc.append_child(root, html_el)),
        Outcome::HierarchyRequest
    );
    doc.check_invariants().unwrap();
}
