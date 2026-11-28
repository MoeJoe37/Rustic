//! Shadow roots: attachment, ownership, traversal, dumps, destruction,
//! cloning, adoption and tracing.

use rustic_dom::{
    Document, DocumentLimits, DomError, NodeId, NodeKind, QualifiedName, ShadowRootMode,
};

fn document() -> Document {
    Document::new(DocumentLimits::default()).unwrap()
}

fn html(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

fn shadow_root_of(document: &Document, host: NodeId) -> Option<NodeId> {
    match document.node(host).unwrap().kind() {
        NodeKind::Element(element) => element.shadow_root(),
        _ => None,
    }
}

#[test]
fn only_permitted_html_elements_host_one_shadow_root() {
    let mut doc = document();
    for name in [
        "div",
        "span",
        "p",
        "body",
        "h6",
        "section",
        "my-element",
        "x-é",
        "a-b.c_d",
    ] {
        let host = html(&mut doc, name);
        let root = doc.attach_shadow(host, ShadowRootMode::Open).unwrap();
        assert_eq!(shadow_root_of(&doc, host), Some(root), "{name}");
        assert_eq!(doc.node(root).unwrap().shadow_host(), Some(host));
        assert_eq!(
            doc.attach_shadow(host, ShadowRootMode::Closed),
            Err(DomError::InvalidHierarchy)
        );
    }
    for name in [
        "a",
        "template",
        "input",
        "img",
        "font-face",
        "annotation-xml",
        "nohyphen",
        "1-x",
        "-x",
        "my-Element",
        "a-b c",
        "a-b!",
    ] {
        let element = html(&mut doc, name);
        assert_eq!(
            doc.attach_shadow(element, ShadowRootMode::Open),
            Err(DomError::WrongNodeKind),
            "{name}"
        );
    }
    let svg = doc
        .create_element(
            QualifiedName::from_parsed(Some(rustic_dom::SVG_NAMESPACE), None, "div").unwrap(),
        )
        .unwrap();
    assert_eq!(
        doc.attach_shadow(svg, ShadowRootMode::Open),
        Err(DomError::WrongNodeKind)
    );
    let text = doc.create_text("x").unwrap();
    assert_eq!(
        doc.attach_shadow(text, ShadowRootMode::Open),
        Err(DomError::WrongNodeKind)
    );
    let closed = html(&mut doc, "div");
    doc.attach_shadow(closed, ShadowRootMode::Closed).unwrap();
    let NodeKind::Element(element) = doc.node(closed).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(element.shadow_root_mode(), Some(ShadowRootMode::Closed));
    doc.check_invariants().unwrap();
}

#[test]
fn a_shadow_root_is_a_separate_owned_tree() {
    let mut doc = document();
    let baseline = doc.node_count();
    let outer = html(&mut doc, "div");
    let host = html(&mut doc, "div");
    doc.append_child(outer, host).unwrap();
    let light = html(&mut doc, "span");
    doc.append_child(host, light).unwrap();
    let root = doc.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let inner = html(&mut doc, "p");
    doc.append_child(root, inner).unwrap();
    let text = doc.create_text("in shadow").unwrap();
    doc.append_child(inner, text).unwrap();

    // Not a child, not walked, never inserted, detached or destroyed alone.
    assert_eq!(doc.node(root).unwrap().parent(), None);
    assert_eq!(doc.walk(outer).unwrap().count(), 3);
    let other = html(&mut doc, "div");
    assert_eq!(
        doc.append_child(other, root),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(doc.detach(root), Err(DomError::InvalidHierarchy));
    assert_eq!(doc.destroy_subtree(root), Err(DomError::InvalidHierarchy));
    // Shadow-including ancestors: a host cannot enter its own shadow tree.
    assert_eq!(doc.append_child(inner, host), Err(DomError::Cycle));
    assert_eq!(doc.append_child(inner, outer), Err(DomError::Cycle));
    doc.check_invariants().unwrap();

    assert_eq!(
        doc.dump_tree(outer, 4096).unwrap(),
        concat!(
            "element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"div\"\n",
            "  element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"div\"\n",
            "    #shadow-root\n",
            "      element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"p\"\n",
            "        text \"in shadow\"\n",
            "    element namespace=\"http://www.w3.org/1999/xhtml\" prefix=null local=\"span\"\n",
        )
    );

    doc.destroy_subtree(outer).unwrap();
    doc.destroy_subtree(other).unwrap();
    assert_eq!(doc.node_count(), baseline);
    for id in [outer, host, light, root, inner, text] {
        assert_eq!(doc.node(id).unwrap_err(), DomError::InvalidNode);
    }
    doc.check_invariants().unwrap();
}

#[test]
fn deeply_nested_shadow_trees_destroy_without_recursion() {
    let mut doc = document();
    let baseline = doc.node_count();
    let top = html(&mut doc, "div");
    let mut host = top;
    for _ in 0..20_000 {
        let root = doc.attach_shadow(host, ShadowRootMode::Open).unwrap();
        let next = html(&mut doc, "div");
        doc.append_child(root, next).unwrap();
        host = next;
    }
    doc.check_invariants().unwrap();
    assert!(doc.dump_tree(top, usize::MAX).unwrap().lines().count() > 40_000);
    doc.destroy_subtree(top).unwrap();
    assert_eq!(doc.node_count(), baseline);
}

#[test]
fn clones_drop_shadow_roots_and_adoption_moves_them() {
    let mut doc = document();
    let host = html(&mut doc, "div");
    let light = doc.create_text("light").unwrap();
    doc.append_child(host, light).unwrap();
    let root = doc.attach_shadow(host, ShadowRootMode::Closed).unwrap();
    let inner = html(&mut doc, "b");
    doc.append_child(root, inner).unwrap();

    let copy = doc.clone_node(host, true).unwrap();
    assert_eq!(shadow_root_of(&doc, copy), None);
    assert_eq!(doc.walk(copy).unwrap().count(), 2);
    // A cloned shadow root is a plain fragment.
    let fragment = doc.clone_node(root, true).unwrap();
    assert_eq!(doc.node(fragment).unwrap().shadow_host(), None);
    assert_eq!(doc.walk(fragment).unwrap().count(), 2);
    let document_copy = doc.clone_document().unwrap();
    document_copy.check_invariants().unwrap();

    let mut target = document();
    assert_eq!(
        target.adopt_node(&mut doc, root),
        Err(DomError::InvalidHierarchy)
    );
    let adopted = target.adopt_node(&mut doc, host).unwrap();
    let new_root = shadow_root_of(&target, adopted.root).unwrap();
    assert_eq!(
        target.node(new_root).unwrap().shadow_host(),
        Some(adopted.root)
    );
    let NodeKind::Element(element) = target.node(adopted.root).unwrap().kind() else {
        unreachable!()
    };
    assert_eq!(element.shadow_root_mode(), Some(ShadowRootMode::Closed));
    // The map covers the shadow tree too.
    assert_eq!(adopted.ids.len(), 4);
    assert!(adopted.ids.contains(&(root, new_root)));
    assert_eq!(doc.node(root).unwrap_err(), DomError::InvalidNode);
    doc.check_invariants().unwrap();
    target.check_invariants().unwrap();
}

/// Collect with only `held` (and the document tree) live.
fn collect(doc: &mut Document, held: &[NodeId]) -> usize {
    let mut marks = doc.tree_marks().unwrap();
    doc.mark_document_tree(&mut marks, &mut |_| {}).unwrap();
    for &node in held {
        doc.mark_tree(&mut marks, node, &mut |_| {}).unwrap();
    }
    doc.sweep_unmarked_trees(&marks).unwrap()
}

#[test]
fn hosts_and_shadow_roots_keep_each_other_alive() {
    let mut doc = document();
    let host = html(&mut doc, "div");
    let root = doc.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let inner = html(&mut doc, "span");
    doc.append_child(root, inner).unwrap();

    assert_eq!(collect(&mut doc, &[inner]), 0);
    assert!(doc.node(host).is_ok());
    assert_eq!(collect(&mut doc, &[host]), 0);
    assert!(doc.node(inner).is_ok());
    assert_eq!(collect(&mut doc, &[]), 3);
    assert_eq!(doc.node_count(), 1);
    doc.check_invariants().unwrap();
}

#[test]
fn declarative_root_reclaim_preserves_its_identity_and_detaches_children() {
    let mut doc = document();
    let host = html(&mut doc, "div");
    let options = rustic_dom::ShadowRootOptions {
        declarative: true,
        clonable: true,
        ..rustic_dom::ShadowRootOptions::new(ShadowRootMode::Open)
    };
    let root = doc.attach_shadow_with_options(host, options).unwrap();
    let child = html(&mut doc, "span");
    doc.append_child(root, child).unwrap();
    assert_eq!(
        doc.attach_shadow(host, ShadowRootMode::Closed),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(doc.node(child).unwrap().parent(), Some(root));
    assert_eq!(doc.attach_shadow(host, ShadowRootMode::Open), Ok(root));
    assert_eq!(doc.node(child).unwrap().parent(), None);
    assert_eq!(
        doc.attach_shadow(host, ShadowRootMode::Open),
        Err(DomError::InvalidHierarchy)
    );
    doc.check_invariants().unwrap();
}

#[test]
fn adoption_preserves_manual_assignments_with_both_endpoints_in_the_subtree() {
    let mut source = document();
    let host = html(&mut source, "div");
    let root = source
        .attach_shadow_with_options(
            host,
            rustic_dom::ShadowRootOptions {
                slot_assignment: rustic_dom::SlotAssignment::Manual,
                ..rustic_dom::ShadowRootOptions::new(ShadowRootMode::Open)
            },
        )
        .unwrap();
    let slot = html(&mut source, "slot");
    let child = html(&mut source, "span");
    source.append_child(root, slot).unwrap();
    source.append_child(host, child).unwrap();
    source.assign_slot(slot, &[child]).unwrap();
    let mut dest = document();
    let adopted = dest.adopt_node(&mut source, host).unwrap();
    let rename = |old| adopted.ids.iter().find(|item| item.0 == old).unwrap().1;
    assert_eq!(
        dest.slot_map().unwrap().assigned_nodes(rename(slot)),
        &[rename(child)]
    );
    assert!(!source.has_shadow_roots());
    dest.check_invariants().unwrap();
    source.check_invariants().unwrap();
}
