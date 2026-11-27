//! Live ranges and node iterators follow mutations (DOM §5.3, §6.1).

use rustic_dom::{BoundaryPoint, Document, DocumentLimits, NodeId, QualifiedName};

fn point(node: NodeId, offset: u32) -> BoundaryPoint {
    BoundaryPoint { node, offset }
}

fn element(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

#[test]
fn ranges_follow_insertion_removal_and_data_edits() {
    let mut d = Document::new(DocumentLimits::default()).unwrap();
    let root = d.root();
    let div = element(&mut d, "div");
    d.append_child(root, div).unwrap();
    let a = d.create_text("hello").unwrap();
    let b = element(&mut d, "b");
    d.append_child(div, a).unwrap();
    d.append_child(div, b).unwrap();
    let range = d.create_live_range(point(a, 2), point(div, 2)).unwrap();

    // Insertion before the end point's offset shifts it.
    let c = element(&mut d, "i");
    d.insert_before(div, c, Some(a)).unwrap();
    assert_eq!(d.live_range(range), Some((point(a, 2), point(div, 3))));

    // Replacing data around the start clamps it; after it, shifts it.
    d.replace_character_data(a, 1, 3, &[0x78]).unwrap();
    assert_eq!(d.live_range(range).unwrap().0, point(a, 1));
    d.replace_character_data(a, 0, 0, &[0x79, 0x79]).unwrap();
    assert_eq!(d.live_range(range).unwrap().0, point(a, 3));

    // Removing the start node moves the point to its old parent and index;
    // the end point after it moves down by one.
    d.detach(a).unwrap();
    assert_eq!(d.live_range(range), Some((point(div, 1), point(div, 2))));

    // Moving a node to where it already is still removes and inserts it.
    let inner = d.create_text("x").unwrap();
    d.append_child(b, inner).unwrap();
    d.set_live_range(range, point(inner, 1), point(inner, 1))
        .unwrap();
    d.append_child(div, b).unwrap();
    assert_eq!(d.live_range(range), Some((point(div, 1), point(div, 1))));

    d.drop_live(range);
    assert_eq!(d.live_range(range), None);
}

#[test]
fn split_text_and_merge_move_points_between_text_nodes() {
    let mut d = Document::new(DocumentLimits::default()).unwrap();
    let root = d.root();
    let div = element(&mut d, "div");
    d.append_child(root, div).unwrap();
    let text = d.create_text("abcdef").unwrap();
    d.append_child(div, text).unwrap();
    let inside = d.create_live_range(point(text, 4), point(text, 6)).unwrap();
    let after = d.create_live_range(point(div, 1), point(div, 1)).unwrap();
    let tail = d
        .split_text_with_preflight(text, 3, |_, new| Ok(new))
        .unwrap();
    assert_eq!(d.live_range(inside), Some((point(tail, 1), point(tail, 3))));
    // A point just after the split node moves past the new node too.
    assert_eq!(d.live_range(after), Some((point(div, 2), point(div, 2))));

    // normalize(): merging moves points into the surviving node.
    d.merge_text(text, &[tail], &[0x64, 0x65, 0x66]).unwrap();
    assert_eq!(d.live_range(inside), Some((point(text, 4), point(text, 6))));
    assert_eq!(d.live_range(after), Some((point(div, 1), point(div, 1))));
}

#[test]
fn node_iterators_step_back_when_their_reference_is_removed() {
    let mut d = Document::new(DocumentLimits::default()).unwrap();
    let root = d.root();
    let div = element(&mut d, "div");
    d.append_child(root, div).unwrap();
    let [a, b, c] = [
        element(&mut d, "a"),
        element(&mut d, "b"),
        element(&mut d, "c"),
    ];
    for child in [a, b, c] {
        d.append_child(div, child).unwrap();
    }
    let before = d.create_node_iterator(div).unwrap();
    d.set_node_iterator(before, b, true).unwrap();
    let after = d.create_node_iterator(div).unwrap();
    d.set_node_iterator(after, b, false).unwrap();
    d.detach(b).unwrap();
    // Before the reference: the next node outside the removed subtree.
    let state = d.node_iterator(before).unwrap();
    assert_eq!((state.reference, state.pointer_before_reference), (c, true));
    // After it: the node that preceded the removed one.
    let state = d.node_iterator(after).unwrap();
    assert_eq!(
        (state.reference, state.pointer_before_reference),
        (a, false)
    );
    assert!(d.live_nodes().any(|n| n == div));
}

#[test]
fn ranges_in_a_detached_subtree_follow_it_to_the_adopting_document() {
    let mut source = Document::new(DocumentLimits::default()).unwrap();
    let mut target = Document::new(DocumentLimits::default()).unwrap();
    let text = source.create_text("detached").unwrap();
    let range = source
        .create_live_range(point(text, 1), point(text, 4))
        .unwrap();
    let parent = element(&mut target, "p");
    let root = target.root();
    target.append_child(root, parent).unwrap();
    let adopted = target
        .insert_from_document(&mut source, text, parent, None, |_| Ok(()))
        .unwrap();
    assert_eq!(source.live_range(range), None);
    assert_eq!(
        target.live_range(range),
        Some((point(adopted.root, 1), point(adopted.root, 4)))
    );
}
