use rustic_dom::{
    Document, DocumentLimits, DocumentType, DomError, DomString, NodeId, NodeKind, QualifiedName,
    ReadyState,
};

fn document() -> Document {
    Document::new(DocumentLimits::default()).unwrap()
}

fn html(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

/// A body with attributes, text, a comment, a PI and a template with nested contents.
fn sample(doc: &mut Document) -> NodeId {
    let body = html(doc, "body");
    doc.set_attribute(body, QualifiedName::html("class").unwrap(), "a")
        .unwrap();
    doc.set_attribute(body, QualifiedName::html("id").unwrap(), "b")
        .unwrap();
    let text = doc.create_text("hi").unwrap();
    let comment = doc.create_comment("c").unwrap();
    let pi = doc.create_processing_instruction("pi", "d").unwrap();
    let template = html(doc, "template");
    let contents = doc.create_template_contents(template).unwrap();
    let inner = html(doc, "template");
    doc.append_child(contents, inner).unwrap();
    let nested = doc.create_template_contents(inner).unwrap();
    let deep = doc.create_text("deep").unwrap();
    doc.append_child(nested, deep).unwrap();
    for id in [text, comment, pi, template] {
        doc.append_child(body, id).unwrap();
    }
    body
}

fn dump(doc: &Document, id: NodeId) -> String {
    doc.dump_tree(id, 1 << 16).unwrap()
}

#[test]
fn deep_clone_copies_payloads_and_template_contents_with_new_ids() {
    let mut doc = document();
    let body = sample(&mut doc);
    let before_nodes = doc.node_count();
    let before_bytes = doc.data_bytes();
    let copy = doc.clone_node(body, true).unwrap();
    assert_ne!(copy, body);
    assert_eq!(dump(&doc, copy), dump(&doc, body));
    assert_eq!(doc.node(copy).unwrap().parent(), None);
    assert_eq!(doc.node_count(), before_nodes * 2 - 1);
    assert_eq!(doc.data_bytes(), before_bytes * 2);

    // Copies are independent: editing one leaves the other alone.
    let copied_text = doc.children(copy).unwrap().next().unwrap();
    doc.set_character_data(copied_text, "changed").unwrap();
    assert_ne!(dump(&doc, copy), dump(&doc, body));

    // Template contents are fresh fragments owned by the copied template.
    let template = doc.children(copy).unwrap().last().unwrap();
    let NodeKind::Element(element) = doc.node(template).unwrap().kind() else {
        unreachable!()
    };
    let contents = element.template_contents().unwrap();
    assert_eq!(doc.node(contents).unwrap().template_host(), Some(template));

    // Destroying the copy restores the original accounting exactly.
    doc.destroy_subtree(copy).unwrap();
    assert_eq!(doc.node_count(), before_nodes);
    assert_eq!(doc.data_bytes(), before_bytes);
}

#[test]
fn shallow_clone_copies_attributes_but_not_children() {
    let mut doc = document();
    let body = sample(&mut doc);
    let copy = doc.clone_node(body, false).unwrap();
    assert_eq!(
        dump(&doc, copy),
        concat!(
            "element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"body\"\n",
            "  attribute namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"class\" value=\"a\"\n",
            "  attribute namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"id\" value=\"b\"\n",
        )
    );
    // A shallow template clone still gets its own, empty, contents.
    let template = html(&mut doc, "template");
    let contents = doc.create_template_contents(template).unwrap();
    let child = doc.create_text("x").unwrap();
    doc.append_child(contents, child).unwrap();
    let copy = doc.clone_node(template, false).unwrap();
    let NodeKind::Element(element) = doc.node(copy).unwrap().kind() else {
        unreachable!()
    };
    let copied_contents = element.template_contents().unwrap();
    assert_ne!(copied_contents, contents);
    assert_eq!(doc.children(copied_contents).unwrap().count(), 0);
    // Cloning template contents yields a free-standing fragment.
    let fragment = doc.clone_node(contents, true).unwrap();
    assert_eq!(doc.node(fragment).unwrap().template_host(), None);
    assert_eq!(doc.children(fragment).unwrap().count(), 1);
    doc.destroy_subtree(fragment).unwrap();
}

#[test]
fn clone_failure_at_a_limit_creates_nothing() {
    let mut doc = document();
    let body = sample(&mut doc);
    let limit = doc.node_count() + 3;
    let mut small = Document::new(DocumentLimits {
        max_nodes: limit,
        ..DocumentLimits::default()
    })
    .unwrap();
    let body_small = sample(&mut small);
    let nodes = small.node_count();
    let bytes = small.data_bytes();
    assert_eq!(small.clone_node(body_small, true), Err(DomError::NodeLimit));
    assert_eq!(small.node_count(), nodes);
    assert_eq!(small.data_bytes(), bytes);
    assert_eq!(dump(&small, body_small), dump(&doc, body));
    // A shallow clone still fits.
    small.clone_node(body_small, false).unwrap();
    assert_eq!(
        small.clone_node(small.root(), true),
        Err(DomError::WrongNodeKind)
    );

    let mut tight = Document::new(DocumentLimits {
        max_data_bytes: 20,
        ..DocumentLimits::default()
    })
    .unwrap();
    let text = tight.create_text("12345").unwrap(); // 10 bytes.
    tight.clone_node(text, false).unwrap();
    assert_eq!(tight.clone_node(text, false), Err(DomError::DataLimit));
    assert_eq!(tight.node_count(), 3);
}

#[test]
fn adoption_moves_a_subtree_between_documents_and_invalidates_old_ids() {
    let mut source = document();
    let html_element = html(&mut source, "html");
    source.append_child(source.root(), html_element).unwrap();
    let body = sample(&mut source);
    source.append_child(html_element, body).unwrap();
    let expected = dump(&source, body);
    let old_ids: Vec<NodeId> = source.walk(body).unwrap().map(|(id, _)| id).collect();
    let source_nodes = source.node_count();

    let mut target = document();
    let adopted = target.adopt_node(&mut source, body).unwrap();
    assert_eq!(dump(&target, adopted.root), expected);
    assert_eq!(adopted.ids[0], (body, adopted.root));
    // Every adopted node (including nested template contents) is mapped.
    assert_eq!(adopted.ids.len(), source_nodes - source.node_count());
    for (old, new) in &adopted.ids {
        assert_eq!(source.node(*old).unwrap_err(), DomError::InvalidNode);
        assert_eq!(
            target.node(*new).unwrap().parent().is_some(),
            *new != adopted.root && target.node(*new).unwrap().template_host().is_none()
        );
        assert_eq!(new.document(), target.id());
    }
    for old in old_ids {
        assert!(adopted.ids.iter().any(|(id, _)| *id == old));
    }
    assert_eq!(source.children(html_element).unwrap().count(), 0);
    assert_eq!(target.node(adopted.root).unwrap().parent(), None);
    let target_root = target.root();
    let new_html = html(&mut target, "html");
    target.append_child(target_root, new_html).unwrap();
    target.append_child(new_html, adopted.root).unwrap();
}

#[test]
fn adoption_failures_leave_both_documents_unchanged() {
    let mut source = document();
    let body = sample(&mut source);
    let source_dump = dump(&source, body);
    let source_nodes = source.node_count();

    // Target node limit.
    let mut small = Document::new(DocumentLimits {
        max_nodes: 4,
        ..DocumentLimits::default()
    })
    .unwrap();
    assert_eq!(
        small.adopt_node(&mut source, body),
        Err(DomError::NodeLimit)
    );
    assert_eq!(small.node_count(), 1);
    assert_eq!(small.data_bytes(), 0);
    assert_eq!(source.node_count(), source_nodes);
    assert_eq!(dump(&source, body), source_dump);

    // Target depth limit applies to adopted trees and their template contents.
    let mut shallow = Document::new(DocumentLimits {
        max_depth: 1,
        ..DocumentLimits::default()
    })
    .unwrap();
    // The sample's trees are all one edge deep; a two-deep template contents
    // tree (template > contents > div > span) must still be rejected.
    let template = html(&mut source, "template");
    let contents = source.create_template_contents(template).unwrap();
    let div = html(&mut source, "div");
    let span = html(&mut source, "span");
    source.append_child(contents, div).unwrap();
    source.append_child(div, span).unwrap();
    let source_nodes = source.node_count();
    assert_eq!(
        shallow.adopt_node(&mut source, template),
        Err(DomError::DepthLimit)
    );
    assert_eq!(shallow.node_count(), 1);
    assert_eq!(shallow.data_bytes(), 0);
    assert_eq!(source.node_count(), source_nodes);
    source.destroy_subtree(span).unwrap();
    shallow.adopt_node(&mut source, template).unwrap();
    let source_nodes = source.node_count();
    assert_eq!(source.node_count(), source_nodes);

    // Target data limit.
    let mut tight = Document::new(DocumentLimits {
        max_data_bytes: 8,
        ..DocumentLimits::default()
    })
    .unwrap();
    assert_eq!(
        tight.adopt_node(&mut source, body),
        Err(DomError::DataLimit)
    );
    assert_eq!(tight.data_bytes(), 0);

    // Protected and owned nodes.
    let mut target = document();
    let source_root = source.root();
    assert_eq!(
        target.adopt_node(&mut source, source_root),
        Err(DomError::ProtectedRoot)
    );
    let template = source.children(body).unwrap().last().unwrap();
    let NodeKind::Element(element) = source.node(template).unwrap().kind() else {
        unreachable!()
    };
    let contents = element.template_contents().unwrap();
    assert_eq!(
        target.adopt_node(&mut source, contents),
        Err(DomError::InvalidHierarchy)
    );
    // A node of a third document is rejected without changes.
    let mut other = document();
    let foreign = html(&mut other, "x");
    assert_eq!(
        target.adopt_node(&mut source, foreign),
        Err(DomError::WrongDocument)
    );
    assert_eq!(source.node_count(), source_nodes);
    assert_eq!(target.node_count(), 1);
}

#[test]
fn adopting_a_free_standing_fragment_keeps_it_a_fragment() {
    let mut source = document();
    let fragment = source.create_document_fragment().unwrap();
    let text = source.create_text("t").unwrap();
    source.append_child(fragment, text).unwrap();
    let mut target = document();
    let adopted = target.adopt_node(&mut source, fragment).unwrap();
    assert!(matches!(
        target.node(adopted.root).unwrap().kind(),
        NodeKind::DocumentFragment
    ));
    assert_eq!(adopted.ids.len(), 2);
    assert_eq!(source.node_count(), 1);
}

#[test]
fn clone_document_copies_the_whole_tree_into_a_new_complete_document() {
    let mut doc = Document::new_for_parsing(DocumentLimits::default()).unwrap();
    let doctype = doc
        .create_doctype(DocumentType {
            name: "html".into(),
            public_id: DomString::default(),
            system_id: DomString::default(),
        })
        .unwrap();
    doc.append_child(doc.root(), doctype).unwrap();
    let html_element = html(&mut doc, "html");
    doc.append_child(doc.root(), html_element).unwrap();
    let body = sample(&mut doc);
    doc.append_child(html_element, body).unwrap();
    let copy = doc.clone_document().unwrap();
    assert_ne!(copy.id(), doc.id());
    assert_eq!(dump(&copy, copy.root()), dump(&doc, doc.root()));
    assert_eq!(copy.node_count(), doc.node_count());
    assert_eq!(copy.data_bytes(), doc.data_bytes());
    assert_eq!(copy.ready_state(), ReadyState::Complete);
    assert_eq!(doc.ready_state(), ReadyState::Loading);
    assert_eq!(copy.node(body).unwrap_err(), DomError::WrongDocument);
}

#[test]
fn ready_state_only_moves_forward() {
    let mut doc = Document::new_for_parsing(DocumentLimits::default()).unwrap();
    assert_eq!(doc.ready_state(), ReadyState::Loading);
    assert_eq!(
        doc.advance_ready_state(ReadyState::Loading),
        Err(DomError::InvalidState)
    );
    doc.advance_ready_state(ReadyState::Interactive).unwrap();
    assert_eq!(
        doc.advance_ready_state(ReadyState::Loading),
        Err(DomError::InvalidState)
    );
    doc.advance_ready_state(ReadyState::Complete).unwrap();
    assert_eq!(
        doc.advance_ready_state(ReadyState::Complete),
        Err(DomError::InvalidState)
    );
    assert_eq!(doc.ready_state(), ReadyState::Complete);

    // Aborted parsing can skip Interactive; script documents start Complete.
    let mut aborted = Document::new_for_parsing(DocumentLimits::default()).unwrap();
    aborted.advance_ready_state(ReadyState::Complete).unwrap();
    let created = document();
    assert_eq!(created.ready_state(), ReadyState::Complete);
}

#[test]
fn ids_from_a_dropped_document_never_resolve_in_a_new_one() {
    let mut old = document();
    let node = html(&mut old, "x");
    let old_id = old.id();
    drop(old);
    let fresh = document();
    assert_ne!(fresh.id(), old_id);
    assert_eq!(fresh.node(node).unwrap_err(), DomError::WrongDocument);
}
