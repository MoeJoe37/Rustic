//! The opt-in mutation journal records what DOM mutation records need.

use rustic_dom::{
    Document, DocumentLimits, DomError, DomString, MutationEntry, NodeId, QualifiedName,
};

fn document() -> Document {
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    document.set_mutation_journal(true);
    document
}

fn element(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

fn child_list(
    target: NodeId,
    added: &[NodeId],
    removed: &[NodeId],
    previous: Option<NodeId>,
    next: Option<NodeId>,
    connected: bool,
) -> MutationEntry {
    MutationEntry::ChildList {
        target,
        connected,
        added: added.to_vec(),
        removed: removed.to_vec(),
        previous,
        next,
    }
}

#[test]
fn disabled_journal_records_nothing() {
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let html = element(&mut document, "html");
    document.append_child(document.root(), html).unwrap();
    assert!(!document.mutation_journal_enabled());
    assert!(!document.has_mutation_entries());
    assert!(document.take_mutation_entries().is_empty());
}

#[test]
fn insertion_and_moves_record_removal_then_insertion() {
    let mut document = document();
    let root = document.root();
    let html = element(&mut document, "html");
    let a = element(&mut document, "a");
    let b = element(&mut document, "b");
    document.append_child(root, html).unwrap();
    document.append_child(html, a).unwrap();
    document.append_child(html, b).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(root, &[html], &[], None, None, true),
            child_list(html, &[a], &[], None, None, true),
            child_list(html, &[b], &[], Some(a), None, true),
        ]
    );
    // Moving `a` after `b` removes it first, so `b` is the previous sibling.
    document.append_child(html, a).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(html, &[], &[a], None, Some(b), true),
            child_list(html, &[a], &[], Some(b), None, true),
        ]
    );
    // Appending the last child again still queues both records.
    document.append_child(html, a).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(html, &[], &[a], Some(b), None, true),
            child_list(html, &[a], &[], Some(b), None, true),
        ]
    );
    // Inserting a node before itself uses its next sibling as the reference.
    document.insert_before(html, b, Some(b)).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(html, &[], &[b], None, Some(a), true),
            child_list(html, &[b], &[], None, Some(a), true),
        ]
    );
}

#[test]
fn fragments_record_their_emptying_and_one_insertion() {
    let mut document = document();
    let parent = element(&mut document, "div");
    let existing = element(&mut document, "p");
    document.append_child(parent, existing).unwrap();
    let fragment = document.create_document_fragment().unwrap();
    let a = element(&mut document, "a");
    let b = element(&mut document, "b");
    document.append_child(fragment, a).unwrap();
    document.append_child(fragment, b).unwrap();
    document.take_mutation_entries();
    document
        .insert_before(parent, fragment, Some(existing))
        .unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(fragment, &[], &[a, b], None, None, false),
            child_list(parent, &[a, b], &[], None, Some(existing), false),
        ]
    );
    // An empty fragment changes nothing and records nothing.
    document.insert_before(parent, fragment, None).unwrap();
    assert!(!document.has_mutation_entries());
}

#[test]
fn failed_insertion_records_nothing() {
    let mut document = document();
    let parent = element(&mut document, "div");
    let child = element(&mut document, "span");
    document.append_child(parent, child).unwrap();
    document.take_mutation_entries();
    assert_eq!(document.append_child(child, parent), Err(DomError::Cycle));
    let text = document.create_text("x").unwrap();
    assert_eq!(
        document.append_child(text, child),
        Err(DomError::InvalidHierarchy)
    );
    assert!(!document.has_mutation_entries());
}

#[test]
fn detach_and_replace_all_record_removed_children() {
    let mut document = document();
    let parent = element(&mut document, "div");
    let a = element(&mut document, "a");
    let b = element(&mut document, "b");
    let c = element(&mut document, "c");
    for child in [a, b, c] {
        document.append_child(parent, child).unwrap();
    }
    document.take_mutation_entries();
    document.detach(b).unwrap();
    // A detached node has no parent to record.
    document.detach(b).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [child_list(parent, &[], &[b], Some(a), Some(c), false)]
    );
    document
        .replace_children_with_text(parent, DomString::from("hi"))
        .unwrap();
    let [MutationEntry::ChildList { added, removed, .. }] = &document.take_mutation_entries()[..]
    else {
        panic!("one replace-all record");
    };
    assert_eq!(removed, &[a, c]);
    assert_eq!(added.len(), 1);
    assert_eq!(document.node(parent).unwrap().first_child(), Some(added[0]));
    // Replacing nothing with nothing queues no record.
    let empty = element(&mut document, "i");
    document.take_mutation_entries();
    document
        .replace_children_with_text(empty, DomString::default())
        .unwrap();
    assert!(!document.has_mutation_entries());
}

#[test]
fn attribute_changes_keep_previous_values() {
    let mut document = document();
    let div = element(&mut document, "div");
    let name = QualifiedName::from_parsed(None, None, "id").unwrap();
    document.set_attribute(div, name.clone(), "one").unwrap();
    document.set_attribute(div, name.clone(), "two").unwrap();
    // Setting the same value is still a change record.
    document.set_attribute(div, name.clone(), "two").unwrap();
    document.remove_attribute(div, None, "id").unwrap();
    // Removing a missing attribute is not.
    document.remove_attribute(div, None, "id").unwrap();
    let attribute = |old: Option<&str>, new: Option<&str>| MutationEntry::Attribute {
        target: div,
        name: name.clone(),
        old_value: old.map(DomString::from),
        new_value: new.map(DomString::from),
    };
    assert_eq!(
        document.take_mutation_entries(),
        [
            attribute(None, Some("one")),
            attribute(Some("one"), Some("two")),
            attribute(Some("two"), Some("two")),
            attribute(Some("two"), None),
        ]
    );
}

#[test]
fn character_data_edits_keep_previous_values() {
    let mut document = document();
    let text = document.create_text("abc").unwrap();
    document.set_character_data(text, "abc").unwrap();
    assert_eq!(
        document.set_character_data(text, "xyz").unwrap(),
        DomString::from("abc")
    );
    document.replace_character_data(text, 1, 1, &[]).unwrap();
    document.append_utf8_character_data(text, "!").unwrap();
    document.append_utf8_character_data(text, "").unwrap();
    let data = |old: &str| MutationEntry::CharacterData {
        target: text,
        old_value: DomString::from(old),
    };
    assert_eq!(
        document.take_mutation_entries(),
        [data("abc"), data("abc"), data("xyz"), data("xz")]
    );
    // An invalid offset fails before recording.
    assert_eq!(
        document.replace_character_data(text, 99, 0, &[]),
        Err(DomError::InvalidOffset)
    );
    assert!(!document.has_mutation_entries());
}

#[test]
fn split_text_inserts_before_replacing_data() {
    let mut document = document();
    let parent = element(&mut document, "p");
    let text = document.create_text("hello").unwrap();
    let after = element(&mut document, "b");
    document.append_child(parent, text).unwrap();
    document.append_child(parent, after).unwrap();
    document.take_mutation_entries();
    let new = document
        .split_text_with_preflight(text, 2, |_, id| Ok(id))
        .unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(parent, &[new], &[], Some(text), Some(after), false),
            MutationEntry::CharacterData {
                target: text,
                old_value: DomString::from("hello"),
            },
        ]
    );
    // A detached split only replaces data.
    let lone = document.create_text("ab").unwrap();
    document
        .split_text_with_preflight(lone, 1, |_, id| Ok(id))
        .unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [MutationEntry::CharacterData {
            target: lone,
            old_value: DomString::from("ab"),
        }]
    );
}

#[test]
fn cross_document_moves_record_new_identities_in_the_destination() {
    let mut source = document();
    let mut target = document();
    let old_parent = element(&mut source, "div");
    let moved = element(&mut source, "span");
    let sibling = element(&mut source, "i");
    source.append_child(old_parent, moved).unwrap();
    source.append_child(old_parent, sibling).unwrap();
    let new_parent = element(&mut target, "section");
    source.take_mutation_entries();
    target.take_mutation_entries();
    let adopted = target
        .insert_from_document(&mut source, moved, new_parent, None, |_| Ok(()))
        .unwrap();
    assert!(!source.has_mutation_entries());
    assert_eq!(
        target.take_mutation_entries(),
        [
            child_list(old_parent, &[], &[adopted.root], None, Some(sibling), false),
            child_list(new_parent, &[adopted.root], &[], None, None, false),
        ]
    );
    // Without a destination journal the source records the removal.
    let mut quiet = Document::new(DocumentLimits::default()).unwrap();
    let adopted = quiet.adopt_node(&mut source, sibling).unwrap();
    assert_eq!(
        source.take_mutation_entries(),
        [child_list(
            old_parent,
            &[],
            &[adopted.root],
            None,
            None,
            false
        )]
    );
    // A fragment from another document records its emptying first.
    let fragment = source.create_document_fragment().unwrap();
    let a = element(&mut source, "a");
    source.append_child(fragment, a).unwrap();
    source.take_mutation_entries();
    let adopted = target
        .insert_from_document(&mut source, fragment, new_parent, None, |_| Ok(()))
        .unwrap();
    let new_a = adopted.ids[0].1;
    assert_eq!(
        target.take_mutation_entries(),
        [
            child_list(fragment, &[], &[new_a], None, None, false),
            child_list(
                new_parent,
                &[new_a],
                &[],
                Some(target_last(&target, new_parent, new_a)),
                None,
                false
            ),
        ]
    );
}

/// The sibling before `child` under `parent`.
fn target_last(document: &Document, _parent: NodeId, child: NodeId) -> NodeId {
    document.node(child).unwrap().previous_sibling().unwrap()
}

#[test]
fn disabling_drops_pending_entries() {
    let mut document = document();
    let div = element(&mut document, "div");
    document.append_child(document.root(), div).unwrap();
    assert!(document.has_mutation_entries());
    document.set_mutation_journal(false);
    document.set_mutation_journal(true);
    assert!(document.take_mutation_entries().is_empty());
}

#[test]
fn replace_child_records_adoption_then_one_combined_entry() {
    let mut document = document();
    let parent = element(&mut document, "div");
    let [a, b, c] = ["a", "b", "c"].map(|name| element(&mut document, name));
    for child in [a, b, c] {
        document.append_child(parent, child).unwrap();
    }
    document.take_mutation_entries();
    // Replacing `a` with its sibling `c`: `c` leaves first, then one entry.
    document.replace_child(parent, c, a).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(parent, &[], &[c], Some(b), None, false),
            child_list(parent, &[c], &[a], None, Some(b), false),
        ]
    );
    assert_eq!(document.node(parent).unwrap().first_child(), Some(c));
    // Self replacement changes nothing but still records both entries.
    document.replace_child(parent, b, b).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(parent, &[], &[b], Some(c), None, false),
            child_list(parent, &[b], &[], Some(c), None, false),
        ]
    );
    // A fragment records its emptying, and a detached node only the
    // combined entry.
    let fragment = document.create_document_fragment().unwrap();
    let [x, y] = ["x", "y"].map(|name| element(&mut document, name));
    document.append_child(fragment, x).unwrap();
    document.append_child(fragment, y).unwrap();
    document.take_mutation_entries();
    document.replace_child(parent, fragment, b).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(fragment, &[], &[x, y], None, None, false),
            child_list(parent, &[x, y], &[b], Some(c), None, false),
        ]
    );
    let lone = element(&mut document, "z");
    document.replace_child(parent, lone, c).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [child_list(parent, &[lone], &[c], None, Some(x), false)]
    );
}

#[test]
fn rejected_replacements_leave_the_tree_and_journal_unchanged() {
    let mut document = document();
    let root = document.root();
    let html = element(&mut document, "html");
    document.append_child(root, html).unwrap();
    let child = element(&mut document, "p");
    document.append_child(html, child).unwrap();
    let stranger = element(&mut document, "q");
    let text = document.create_text("t").unwrap();
    document.take_mutation_entries();
    // `child` is not a child of the root, and an ancestor cannot replace its descendant.
    assert_eq!(
        document.replace_child(root, stranger, child),
        Err(DomError::InvalidReference)
    );
    assert_eq!(
        document.replace_child(html, html, child),
        Err(DomError::Cycle)
    );
    // Text cannot become a document child; the document element returns.
    assert_eq!(
        document.replace_child(root, text, html),
        Err(DomError::InvalidHierarchy)
    );
    assert_eq!(document.node(root).unwrap().first_child(), Some(html));
    // Replacing the document element with another element is allowed.
    document.replace_child(root, stranger, html).unwrap();
    assert_eq!(document.node(root).unwrap().first_child(), Some(stranger));
    let entries = document.take_mutation_entries();
    assert_eq!(
        entries,
        [child_list(root, &[stranger], &[html], None, None, true)]
    );
    document.check_invariants().unwrap();
}

#[test]
fn child_list_connectivity_is_recorded_before_later_tree_edits() {
    let mut document = document();
    let html = element(&mut document, "html");
    let host = element(&mut document, "div");
    document.append_child(document.root(), html).unwrap();
    document.append_child(html, host).unwrap();
    let root = document
        .attach_shadow(host, rustic_dom::ShadowRootMode::Closed)
        .unwrap();
    let node = element(&mut document, "x-widget");
    document.take_mutation_entries();
    document.append_child(root, node).unwrap();
    document.detach(host).unwrap();
    document.detach(node).unwrap();
    assert_eq!(
        document.take_mutation_entries(),
        [
            child_list(root, &[node], &[], None, None, true),
            child_list(html, &[], &[host], None, None, true),
            child_list(root, &[], &[node], None, None, false),
        ]
    );
}
