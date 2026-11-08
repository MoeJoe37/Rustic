use rustic_dom::{Document, DocumentLimits, DomError, DomString, NodeId, NodeKind, QualifiedName};

fn document() -> Document {
    Document::new(DocumentLimits::default()).unwrap()
}

fn html(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

fn children(document: &Document, parent: NodeId) -> Vec<NodeId> {
    document.children(parent).unwrap().collect()
}

#[test]
fn inserting_a_fragment_moves_its_children_in_order_and_empties_it() {
    let mut doc = document();
    let body = html(&mut doc, "body");
    let existing = html(&mut doc, "p");
    doc.append_child(body, existing).unwrap();
    let fragment = doc.create_document_fragment().unwrap();
    assert_eq!(doc.node(fragment).unwrap().template_host(), None);
    let a = doc.create_text("a").unwrap();
    let b = html(&mut doc, "b");
    let c = doc.create_comment("c").unwrap();
    for id in [a, b, c] {
        doc.append_child(fragment, id).unwrap();
    }
    doc.insert_before(body, fragment, Some(existing)).unwrap();
    assert_eq!(children(&doc, body), [a, b, c, existing]);
    assert_eq!(children(&doc, fragment), []);
    assert_eq!(doc.node(fragment).unwrap().parent(), None);
    for id in [a, b, c] {
        assert_eq!(doc.node(id).unwrap().parent(), Some(body));
    }
    // An empty fragment is a no-op, and the fragment remains usable.
    doc.append_child(body, fragment).unwrap();
    assert_eq!(children(&doc, body), [a, b, c, existing]);
    doc.append_child(fragment, existing).unwrap();
    assert_eq!(children(&doc, fragment), [existing]);
    // A free-standing fragment can be detached (no-op) and destroyed.
    let count = doc.node_count();
    doc.detach(fragment).unwrap();
    doc.destroy_subtree(fragment).unwrap();
    assert_eq!(doc.node_count(), count - 2);
    assert_eq!(doc.node(existing).unwrap_err(), DomError::InvalidNode);
}

#[test]
fn rejected_fragment_insertion_moves_nothing() {
    let mut doc = document();
    let html_element = html(&mut doc, "html");
    doc.append_child(doc.root(), html_element).unwrap();

    // Document root: a second element or any text is a hierarchy error.
    let fragment = doc.create_document_fragment().unwrap();
    let comment = doc.create_comment("ok").unwrap();
    let second = html(&mut doc, "second");
    doc.append_child(fragment, comment).unwrap();
    doc.append_child(fragment, second).unwrap();
    assert_eq!(
        doc.append_child(doc.root(), fragment),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(children(&doc, fragment), [comment, second]);
    assert_eq!(children(&doc, doc.root()), [html_element]);

    let text_fragment = doc.create_document_fragment().unwrap();
    let text = doc.create_text("x").unwrap();
    doc.append_child(text_fragment, text).unwrap();
    assert_eq!(
        doc.append_child(doc.root(), text_fragment),
        Err(DomError::InvalidHierarchy)
    );

    // A fragment cannot be inserted into itself or its descendants.
    let inner = html(&mut doc, "div");
    doc.append_child(fragment, inner).unwrap();
    assert_eq!(doc.append_child(inner, fragment), Err(DomError::Cycle));
    assert_eq!(doc.append_child(fragment, fragment), Err(DomError::Cycle));

    // Depth: children land one level below the parent.
    let mut shallow = Document::new(DocumentLimits {
        max_depth: 2,
        ..DocumentLimits::default()
    })
    .unwrap();
    let top = html(&mut shallow, "top");
    shallow.append_child(shallow.root(), top).unwrap();
    let fragment = shallow.create_document_fragment().unwrap();
    let child = html(&mut shallow, "child");
    let grandchild = html(&mut shallow, "grandchild");
    shallow.append_child(fragment, child).unwrap();
    shallow.append_child(child, grandchild).unwrap();
    assert_eq!(
        shallow.append_child(top, fragment),
        Err(DomError::DepthLimit)
    );
    assert_eq!(children(&shallow, fragment), [child]);
    shallow.append_child(shallow.root(), fragment).unwrap_err();
    shallow.destroy_subtree(grandchild).unwrap();
    shallow.append_child(top, fragment).unwrap();
    assert_eq!(children(&shallow, top), [child]);
}

#[test]
fn pre_insert_checks_follow_dom_step_order() {
    let mut doc = document();
    let parent = html(&mut doc, "parent");
    let unrelated = html(&mut doc, "unrelated");
    let child = html(&mut doc, "child");
    doc.append_child(child, parent).unwrap();
    // Fails both the ancestor check and the reference check: ancestor first.
    assert_eq!(
        doc.insert_before(parent, child, Some(unrelated)),
        Err(DomError::Cycle)
    );
    let text = doc.create_text("t").unwrap();
    assert_eq!(
        doc.insert_before(text, unrelated, Some(child)),
        Err(DomError::InvalidHierarchy)
    );
}

#[test]
fn template_contents_insert_like_a_fragment_but_stay_owned() {
    let mut doc = document();
    let template = html(&mut doc, "template");
    let contents = doc.create_template_contents(template).unwrap();
    assert_eq!(doc.node(contents).unwrap().template_host(), Some(template));
    let span = html(&mut doc, "span");
    doc.append_child(contents, span).unwrap();
    let target = html(&mut doc, "div");
    doc.append_child(target, contents).unwrap();
    assert_eq!(children(&doc, target), [span]);
    assert_eq!(children(&doc, contents), []);
    assert_eq!(
        doc.destroy_subtree(contents),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(doc.detach(contents), Err(DomError::InvalidHierarchy));
}

#[test]
fn processing_instructions_validate_creation_and_edit_as_character_data() {
    let mut doc = document();
    assert_eq!(
        doc.create_processing_instruction("1bad", "x"),
        Err(DomError::InvalidName)
    );
    assert_eq!(
        doc.create_processing_instruction("", "x"),
        Err(DomError::InvalidName)
    );
    assert_eq!(
        doc.create_processing_instruction("xml-stylesheet", "a?>b"),
        Err(DomError::InvalidCharacterData)
    );
    let before = doc.data_bytes();
    let pi = doc
        .create_processing_instruction("xml-stylesheet", "href=\"a.css\"")
        .unwrap();
    let target_bytes = "xml-stylesheet".len() * 2;
    let data_bytes = "href=\"a.css\"".len() * 2;
    assert_eq!(doc.data_bytes(), before + target_bytes + data_bytes);
    // Data edits keep the target and its accounting.
    doc.set_character_data(pi, "x").unwrap();
    assert_eq!(doc.data_bytes(), before + target_bytes + 2);
    doc.append_character_data(pi, &[0x3f, 0x3e]).unwrap(); // "?>" is allowed after creation.
    assert_eq!(doc.data_bytes(), before + target_bytes + 6);
    let NodeKind::ProcessingInstruction(value) = doc.node(pi).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(value.target(), &DomString::from("xml-stylesheet"));
    assert_eq!(value.data(), &DomString::from("x?>"));

    // A processing instruction may sit beside the document element.
    let root_element = html(&mut doc, "html");
    doc.append_child(doc.root(), pi).unwrap();
    doc.append_child(doc.root(), root_element).unwrap();
    let late = doc.create_processing_instruction("late", "").unwrap();
    doc.append_child(doc.root(), late).unwrap();
    // But it cannot have children.
    let text = doc.create_text("x").unwrap();
    assert_eq!(doc.append_child(pi, text), Err(DomError::InvalidHierarchy));
    assert_eq!(
        doc.dump_tree(doc.root(), 4096).unwrap(),
        concat!(
            "#document\n",
            "  processing-instruction target=\"xml-stylesheet\" data=\"x?>\"\n",
            "  element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"html\"\n",
            "  processing-instruction target=\"late\" data=\"\"\n",
        )
    );
}

#[test]
fn processing_instruction_data_edits_respect_the_data_limit() {
    let mut doc = Document::new(DocumentLimits {
        max_data_bytes: 10,
        ..DocumentLimits::default()
    })
    .unwrap();
    let pi = doc.create_processing_instruction("ab", "cd").unwrap(); // 8 bytes.
    assert_eq!(doc.append_character_data(pi, &[0x65]), Ok(()));
    assert_eq!(
        doc.append_character_data(pi, &[0x66]),
        Err(DomError::DataLimit)
    );
    assert_eq!(doc.set_character_data(pi, "wxyz"), Err(DomError::DataLimit));
    assert_eq!(doc.data_bytes(), 10);
    doc.set_character_data(pi, "").unwrap();
    assert_eq!(doc.data_bytes(), 4);
}

#[test]
fn cdata_sections_are_text_like_character_data() {
    let mut doc = document();
    assert_eq!(
        doc.create_cdata_section("a]]>b"),
        Err(DomError::InvalidCharacterData)
    );
    let cdata = doc.create_cdata_section("x < y").unwrap();
    doc.replace_character_data(cdata, 0, 1, &[0x7a]).unwrap();
    let NodeKind::CdataSection(value) = doc.node(cdata).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(value, &DomString::from("z < y"));
    // Like text, CDATA cannot be a child of the document.
    assert_eq!(
        doc.append_child(doc.root(), cdata),
        Err(DomError::InvalidHierarchy)
    );
    let element = html(&mut doc, "svg");
    doc.append_child(element, cdata).unwrap();
    let fragment = doc.create_document_fragment().unwrap();
    doc.append_child(fragment, element).unwrap();
    assert_eq!(
        doc.dump_tree(fragment, 4096).unwrap(),
        concat!(
            "#document-fragment\n",
            "  element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"svg\"\n",
            "    cdata \"z < y\"\n",
        )
    );
}
