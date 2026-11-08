use std::num::NonZeroU64;

use rustic_dom::{
    Document, DocumentLimits, DocumentType, DomError, DomString, NodeId, NodeKind, QualifiedName,
};

fn document() -> Document {
    Document::new(DocumentLimits::default()).unwrap()
}

fn element(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::from_parsed(None, None, name).unwrap())
        .unwrap()
}

fn doctype(document: &mut Document) -> NodeId {
    document
        .create_doctype(DocumentType {
            name: "html".into(),
            public_id: DomString::default(),
            system_id: DomString::default(),
        })
        .unwrap()
}

fn children(document: &Document, parent: NodeId) -> Vec<NodeId> {
    document.children(parent).unwrap().collect()
}

#[test]
fn stale_forged_and_foreign_handles_cannot_access_a_live_node() {
    let mut doc = document();
    let other = document();
    let first = element(&mut doc, "first");
    doc.destroy_subtree(first).unwrap();
    let replacement = element(&mut doc, "replacement");
    assert_eq!(first.slot(), replacement.slot());
    assert_ne!(first.generation(), replacement.generation());
    assert_eq!(doc.node(first).unwrap_err(), DomError::InvalidNode);
    assert_eq!(doc.node(other.root()).unwrap_err(), DomError::WrongDocument);
    let out_of_range = NodeId::from_parts(doc.id(), u32::MAX, NonZeroU64::MIN);
    assert_eq!(doc.node(out_of_range).unwrap_err(), DomError::InvalidNode);
    let forged = NodeId::from_parts(doc.id(), replacement.slot(), NonZeroU64::new(99).unwrap());
    assert_eq!(doc.node(forged).unwrap_err(), DomError::InvalidNode);
    assert_eq!(
        doc.append_child(doc.root(), other.root()),
        Err(DomError::WrongDocument)
    );
    assert_eq!(doc.detach(doc.root()), Err(DomError::ProtectedRoot));
    assert_eq!(
        doc.destroy_subtree(doc.root()),
        Err(DomError::ProtectedRoot)
    );
    assert_eq!(
        doc.append_child(replacement, doc.root()),
        Err(DomError::ProtectedRoot)
    );
}

#[test]
fn document_hierarchy_and_rejected_moves_preserve_the_original_tree() {
    let mut doc = document();
    let root = doc.root();
    let html = element(&mut doc, "html");
    let body = element(&mut doc, "body");
    let declaration = doctype(&mut doc);
    let duplicate = doctype(&mut doc);
    let comment = doc.create_comment("after root is allowed").unwrap();
    let text = doc.create_text("not a document child").unwrap();
    doc.append_child(root, declaration).unwrap();
    doc.append_child(root, html).unwrap();
    doc.append_child(html, body).unwrap();
    doc.append_child(root, comment).unwrap();
    let before = children(&doc, root);
    assert_eq!(
        doc.insert_before(root, html, Some(declaration)),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(
        doc.append_child(root, declaration),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(
        doc.append_child(root, duplicate),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(
        doc.append_child(root, body),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(
        doc.append_child(root, text),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(
        doc.append_child(body, declaration),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(
        doc.append_child(text, body),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(
        doc.insert_before(html, text, Some(comment)),
        Err(DomError::InvalidReference)
    );
    assert_eq!(doc.append_child(body, html), Err(DomError::Cycle));
    assert_eq!(children(&doc, root), before);
    assert_eq!(children(&doc, html), vec![body]);
    assert_eq!(doc.node(declaration).unwrap().parent(), Some(root));
    assert_eq!(doc.node(text).unwrap().parent(), None);
}

#[test]
fn reordering_self_insertion_and_detachment_preserve_links_and_identity() {
    let mut doc = document();
    let parent = element(&mut doc, "parent");
    let a = element(&mut doc, "a");
    let b = element(&mut doc, "b");
    let c = element(&mut doc, "c");
    for child in [a, b, c] {
        doc.append_child(parent, child).unwrap();
    }
    doc.insert_before(parent, c, Some(a)).unwrap();
    assert_eq!(children(&doc, parent), vec![c, a, b]);
    doc.insert_before(parent, a, Some(a)).unwrap();
    assert_eq!(children(&doc, parent), vec![c, a, b]);
    doc.append_child(parent, c).unwrap();
    assert_eq!(children(&doc, parent), vec![a, b, c]);
    doc.detach(b).unwrap();
    doc.detach(b).unwrap();
    assert_eq!(children(&doc, parent), vec![a, c]);
    let node = doc.node(b).unwrap();
    assert_eq!(
        (node.parent(), node.previous_sibling(), node.next_sibling()),
        (None, None, None)
    );
    assert_eq!(doc.node(a).unwrap().next_sibling(), Some(c));
    assert_eq!(doc.node(c).unwrap().previous_sibling(), Some(a));
    assert_eq!(doc.node_count(), 5);
    assert_eq!(
        doc.walk(parent).unwrap().collect::<Vec<_>>(),
        vec![(parent, 0), (a, 1), (c, 1)]
    );
    assert_eq!(doc.walk(b).unwrap().collect::<Vec<_>>(), vec![(b, 0)]);
}

#[test]
fn node_data_and_depth_limits_fail_atomically_and_recover_after_destruction() {
    let mut doc = Document::new(DocumentLimits {
        max_nodes: 2,
        max_depth: 1,
        max_data_bytes: 0,
    })
    .unwrap();
    assert_eq!(doc.create_text("x"), Err(DomError::DataLimit));
    assert_eq!(doc.node_count(), 1);
    let comment = doc.create_comment("").unwrap();
    doc.append_child(doc.root(), comment).unwrap();
    assert_eq!(doc.create_comment(""), Err(DomError::NodeLimit));
    doc.destroy_subtree(comment).unwrap();
    assert!(doc.create_comment("").is_ok());
    assert_eq!(doc.data_bytes(), 0);
    assert_eq!(
        Document::new(DocumentLimits {
            max_nodes: 0,
            ..DocumentLimits::default()
        })
        .unwrap_err(),
        DomError::NodeLimit
    );

    let mut doc = Document::new(DocumentLimits {
        max_depth: 2,
        ..DocumentLimits::default()
    })
    .unwrap();
    let a = element(&mut doc, "a");
    let b = element(&mut doc, "b");
    let c = element(&mut doc, "c");
    let d = element(&mut doc, "d");
    doc.append_child(doc.root(), a).unwrap();
    doc.append_child(a, b).unwrap();
    doc.append_child(c, d).unwrap();
    assert_eq!(doc.append_child(b, c), Err(DomError::DepthLimit));
    assert_eq!(doc.append_child(a, c), Err(DomError::DepthLimit));
    assert_eq!(doc.node(c).unwrap().parent(), None);
    assert_eq!(doc.node(d).unwrap().parent(), Some(c));
}

#[test]
fn attributes_preserve_expanded_name_identity_order_and_utf16() {
    let mut doc = document();
    let element = element(&mut doc, "e");
    let name = |prefix| QualifiedName::from_parsed(Some("urn:test"), Some(prefix), "key").unwrap();
    let plain = QualifiedName::from_parsed(None, None, "key").unwrap();
    doc.set_attribute(element, name("a"), "first").unwrap();
    doc.set_attribute(element, plain, "unqualified").unwrap();
    let units = DomString::from_units(vec![0xd800, 0, 0xdc00, 0xd83d, 0xde00]);
    assert_eq!(
        doc.set_attribute(element, name("b"), units.clone())
            .unwrap(),
        Some("first".into())
    );
    let NodeKind::Element(data) = doc.node(element).unwrap().kind() else {
        panic!("expected element")
    };
    assert_eq!(data.attributes().len(), 2);
    assert_eq!(data.attributes()[0].name().prefix(), Some("b"));
    assert_eq!(data.attributes()[0].value(), &units);
    assert_eq!(data.attributes()[1].name().namespace(), None);
    assert_eq!(
        doc.remove_attribute(element, Some("urn:test"), "key")
            .unwrap(),
        Some(units)
    );
    assert_eq!(
        doc.remove_attribute(element, Some("urn:test"), "key")
            .unwrap(),
        None
    );
    assert_eq!(
        doc.remove_attribute(element, Some(""), "key").unwrap(),
        Some("unqualified".into())
    );
    assert_eq!(doc.data_bytes(), 1); // Only the element local name remains.
}

#[test]
fn payload_replacement_rejects_excess_without_losing_prior_data() {
    let mut doc = Document::new(DocumentLimits {
        max_data_bytes: 6,
        ..DocumentLimits::default()
    })
    .unwrap();
    let node = element(&mut doc, "e"); // One UTF-8 name byte.
    let text = doc.create_text("ab").unwrap(); // Four UTF-16 bytes.
    let key = || QualifiedName::from_parsed(None, None, "x").unwrap();
    doc.set_attribute(node, key(), "").unwrap(); // One more name byte: limit exactly.
    assert_eq!(doc.data_bytes(), 6);
    assert_eq!(
        doc.set_attribute(node, key(), "z"),
        Err(DomError::DataLimit)
    );
    assert_eq!(
        doc.set_character_data(text, "abcd"),
        Err(DomError::DataLimit)
    );
    assert_eq!(doc.data_bytes(), 6);
    let NodeKind::Text(value) = doc.node(text).unwrap().kind() else {
        panic!("expected text")
    };
    assert_eq!(value, &DomString::from("ab"));
    assert_eq!(doc.set_character_data(text, "z").unwrap(), "ab".into());
    doc.destroy_subtree(node).unwrap();
    assert_eq!(doc.data_bytes(), 2);
    assert_eq!(
        doc.set_character_data(doc.root(), ""),
        Err(DomError::WrongNodeKind)
    );
}

#[test]
fn traversal_and_destruction_do_not_recurse_on_deep_trees() {
    let depth = 1500;
    let mut doc = Document::new(DocumentLimits {
        max_depth: depth,
        ..DocumentLimits::default()
    })
    .unwrap();
    let first = element(&mut doc, "n");
    doc.append_child(doc.root(), first).unwrap();
    let mut parent = first;
    for _ in 1..depth {
        let node = element(&mut doc, "n");
        doc.append_child(parent, node).unwrap();
        parent = node;
    }
    let extra = element(&mut doc, "n");
    assert_eq!(doc.append_child(parent, extra), Err(DomError::DepthLimit));
    assert_eq!(doc.walk(doc.root()).unwrap().last(), Some((parent, depth)));
    doc.destroy_subtree(first).unwrap();
    assert_eq!(doc.node_count(), 2); // Root and detached extra node.
    assert_eq!(doc.data_bytes(), 1);
    assert_eq!(doc.node(parent).unwrap_err(), DomError::InvalidNode);
    doc.destroy_subtree(extra).unwrap();
    assert_eq!(doc.node_count(), 1);
    assert_eq!(doc.data_bytes(), 0);
}

#[test]
fn seeded_mutation_sequences_match_an_independent_ordered_tree_model() {
    const COUNT: usize = 32;
    let mut doc = document();
    let mut nodes = vec![element(&mut doc, "container")];
    doc.append_child(doc.root(), nodes[0]).unwrap();
    for _ in 1..COUNT {
        nodes.push(element(&mut doc, "n"));
    }
    let mut parents: Vec<Option<usize>> = vec![None; COUNT];
    let mut ordered: Vec<Vec<usize>> = vec![Vec::new(); COUNT];
    let mut seed = 0x6a09e667f3bcc909u64;
    let mut random = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as usize
    };
    for _ in 0..1500 {
        let child = 1 + random() % (COUNT - 1);
        let parent = random() % COUNT;
        if random() % 5 == 0 {
            doc.detach(nodes[child]).unwrap();
            if let Some(old) = parents[child].take() {
                ordered[old].retain(|id| *id != child);
            }
        } else {
            let reference = if ordered[parent].is_empty() || random() % 2 == 0 {
                None
            } else {
                Some(ordered[parent][random() % ordered[parent].len()])
            };
            let mut ancestor = Some(parent);
            let mut cycle = false;
            while let Some(id) = ancestor {
                if id == child {
                    cycle = true;
                    break;
                }
                ancestor = parents[id];
            }
            let result =
                doc.insert_before(nodes[parent], nodes[child], reference.map(|id| nodes[id]));
            if cycle {
                assert_eq!(result, Err(DomError::Cycle));
            } else {
                result.unwrap();
                if reference != Some(child) {
                    if let Some(old) = parents[child] {
                        ordered[old].retain(|id| *id != child);
                    }
                    let index = reference
                        .map(|id| ordered[parent].iter().position(|item| *item == id).unwrap())
                        .unwrap_or(ordered[parent].len());
                    ordered[parent].insert(index, child);
                    parents[child] = Some(parent);
                }
            }
        }
        for id in 0..COUNT {
            assert_eq!(
                children(&doc, nodes[id]),
                ordered[id].iter().map(|id| nodes[*id]).collect::<Vec<_>>()
            );
            let node = doc.node(nodes[id]).unwrap();
            assert_eq!(
                node.parent(),
                if id == 0 {
                    Some(doc.root())
                } else {
                    parents[id].map(|id| nodes[id])
                }
            );
            assert_eq!(node.first_child(), ordered[id].first().map(|id| nodes[*id]));
            assert_eq!(node.last_child(), ordered[id].last().map(|id| nodes[*id]));
            for (index, child) in ordered[id].iter().enumerate() {
                let node = doc.node(nodes[*child]).unwrap();
                assert_eq!(
                    node.previous_sibling(),
                    index.checked_sub(1).map(|index| nodes[ordered[id][index]])
                );
                assert_eq!(
                    node.next_sibling(),
                    ordered[id].get(index + 1).map(|id| nodes[*id])
                );
            }
        }
    }
}

fn html(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

#[test]
fn template_contents_are_a_separate_owned_root_destroyed_with_their_template() {
    let mut doc = document();
    let baseline = doc.node_count();
    let div = html(&mut doc, "div");
    assert_eq!(
        doc.create_template_contents(div),
        Err(DomError::WrongNodeKind)
    );
    let svg_template = doc
        .create_element(
            QualifiedName::from_parsed(Some(rustic_dom::SVG_NAMESPACE), None, "template").unwrap(),
        )
        .unwrap();
    assert_eq!(
        doc.create_template_contents(svg_template),
        Err(DomError::WrongNodeKind)
    );
    doc.destroy_subtree(svg_template).unwrap();
    let template = html(&mut doc, "template");
    doc.append_child(div, template).unwrap();
    let contents = doc.create_template_contents(template).unwrap();
    assert_eq!(
        doc.create_template_contents(template),
        Err(DomError::InvalidHierarchy)
    );
    let NodeKind::Element(value) = doc.node(template).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(value.template_contents(), Some(contents));
    assert!(matches!(
        doc.node(contents).unwrap().kind(),
        NodeKind::DocumentFragment
    ));
    assert_eq!(doc.node(contents).unwrap().parent(), None);

    // Contents accept ordinary children; inserting them moves those children.
    let inner = html(&mut doc, "template");
    doc.append_child(contents, inner).unwrap();
    let nested = doc.create_template_contents(inner).unwrap();
    let text = doc.create_text("x").unwrap();
    doc.append_child(nested, text).unwrap();
    // Host-including ancestors: a template cannot enter its own contents.
    assert_eq!(doc.append_child(nested, template), Err(DomError::Cycle));
    assert_eq!(doc.append_child(nested, div), Err(DomError::Cycle));
    assert_eq!(doc.detach(contents), Err(DomError::InvalidHierarchy));
    assert_eq!(
        doc.destroy_subtree(contents),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(doc.walk(div).unwrap().count(), 2);
    assert_eq!(
        doc.dump_tree(div, 4096).unwrap(),
        concat!(
            "element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"div\"\n",
            "  element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"template\"\n",
            "    #template-contents\n",
            "      element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"template\"\n",
            "        #template-contents\n",
            "          text \"x\"\n",
        )
    );

    doc.destroy_subtree(div).unwrap();
    assert_eq!(doc.node_count(), baseline);
    for id in [div, template, contents, inner, nested, text] {
        assert_eq!(doc.node(id).unwrap_err(), DomError::InvalidNode);
    }
}

#[test]
fn deeply_nested_template_contents_dump_and_destroy_without_recursion() {
    let mut doc = document();
    let baseline = doc.node_count();
    let root = html(&mut doc, "template");
    let mut parent = doc.create_template_contents(root).unwrap();
    for _ in 0..20_000 {
        let template = html(&mut doc, "template");
        doc.append_child(parent, template).unwrap();
        parent = doc.create_template_contents(template).unwrap();
    }
    assert!(doc.dump_tree(root, usize::MAX).unwrap().lines().count() > 40_000);
    doc.destroy_subtree(root).unwrap();
    assert_eq!(doc.node_count(), baseline);
}
