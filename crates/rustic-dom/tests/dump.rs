use rustic_dom::{
    Document, DocumentLimits, DocumentType, DomError, DomString, DumpError, QualifiedName,
};

fn fixture(with_allocation_history: bool) -> Document {
    let mut doc = Document::new(DocumentLimits::default()).unwrap();
    if with_allocation_history {
        let scratch = doc.create_text("not in the final tree").unwrap();
        doc.destroy_subtree(scratch).unwrap();
    }
    let doctype = doc
        .create_doctype(DocumentType {
            name: "html".into(),
            public_id: DomString::default(),
            system_id: DomString::default(),
        })
        .unwrap();
    let html = doc
        .create_element(QualifiedName::html("html").unwrap())
        .unwrap();
    let body = doc
        .create_element(QualifiedName::html("body").unwrap())
        .unwrap();
    let text = doc.create_text("Hello, Rustic").unwrap();
    let comment = doc.create_comment("inspect me").unwrap();
    doc.append_child(doc.root(), doctype).unwrap();
    doc.append_child(doc.root(), html).unwrap();
    doc.append_child(html, body).unwrap();
    doc.append_child(body, text).unwrap();
    doc.append_child(body, comment).unwrap();
    doc.set_attribute(
        body,
        QualifiedName::from_parsed(None, None, "id").unwrap(),
        "example",
    )
    .unwrap();
    doc
}

#[test]
fn dump_matches_fixture_and_is_independent_of_ids_and_allocation_history() {
    let doc = fixture(false);
    let other = fixture(true);
    let expected = include_str!("fixtures/tree.txt");
    assert_ne!(doc.id(), other.id());
    assert_eq!(doc.dump_tree(doc.root(), expected.len()).unwrap(), expected);
    assert_eq!(
        other.dump_tree(other.root(), expected.len()).unwrap(),
        expected
    );
    assert_eq!(
        doc.dump_tree(doc.root(), expected.len() - 1),
        Err(DumpError::OutputLimit)
    );
    assert_eq!(doc.dump_tree(doc.root(), 0), Err(DumpError::OutputLimit));
}

#[test]
fn quotes_controls_and_lone_surrogates_without_data_loss() {
    let mut doc = Document::new(DocumentLimits::default()).unwrap();
    let text = doc
        .create_text(DomString::from_units(vec![
            0x41, 0x0a, 0x22, 0x5c, 0, 0xd800, 0xd83d, 0xde00, 0xdc00,
        ]))
        .unwrap();
    let expected = concat!("text \"", r#"A\n\"\\\u0000\ud800😀\udc00"#, "\"\n");
    assert_eq!(doc.dump_tree(text, expected.len()).unwrap(), expected);
    assert_eq!(
        doc.dump_tree(text, expected.len() - 1),
        Err(DumpError::OutputLimit)
    );
}

#[test]
fn selected_subtrees_exclude_siblings_and_unrelated_detached_nodes() {
    let mut doc = fixture(false);
    let detached = doc.create_comment("standalone").unwrap();
    let html = doc.children(doc.root()).unwrap().nth(1).unwrap();
    let body = doc.node(html).unwrap().first_child().unwrap();
    let text = doc.node(body).unwrap().first_child().unwrap();
    assert_eq!(
        doc.dump_tree(text, 100).unwrap(),
        "text \"Hello, Rustic\"\n"
    );
    assert_eq!(
        doc.dump_tree(detached, 100).unwrap(),
        "comment \"standalone\"\n"
    );
    doc.destroy_subtree(detached).unwrap();
    assert_eq!(
        doc.dump_tree(detached, 100),
        Err(DumpError::Node(DomError::InvalidNode))
    );
    let other = fixture(false);
    assert_eq!(
        doc.dump_tree(other.root(), 100),
        Err(DumpError::Node(DomError::WrongDocument))
    );
}

#[test]
fn diagnostic_names_cannot_inject_new_lines_or_unquoted_fields() {
    let mut doc = Document::new(DocumentLimits::default()).unwrap();
    let name =
        QualifiedName::from_parsed(Some("urn:\n\""), Some("p\n"), "n\n\u{2028}\u{2029}").unwrap();
    let element = doc.create_element(name).unwrap();
    let expected =
        "element namespace=\"urn:\\n\\\"\" prefix=\"p\\n\" local=\"n\\n\\u2028\\u2029\"\n";
    assert_eq!(doc.dump_tree(element, expected.len()).unwrap(), expected);
    assert_eq!(expected.lines().count(), 1);
}
