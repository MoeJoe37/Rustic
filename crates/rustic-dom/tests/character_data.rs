use std::num::NonZeroU64;

use rustic_dom::{
    Document, DocumentLimits, DocumentType, DomError, DomString, NodeId, NodeKind, QualifiedName,
};

fn document(max_data_bytes: usize) -> Document {
    Document::new(DocumentLimits {
        max_data_bytes,
        ..DocumentLimits::default()
    })
    .unwrap()
}

fn value(doc: &Document, id: NodeId) -> &DomString {
    match doc.node(id).unwrap().kind() {
        NodeKind::Text(value) | NodeKind::Comment(value) => value,
        _ => panic!("expected character data"),
    }
}

#[test]
fn replacement_uses_code_units_and_preserves_lone_surrogates() {
    let mut doc = document(64);
    for comment in [false, true] {
        let initial = DomString::from_units(vec![0x61, 0xd83d, 0xde80, 0x62]);
        let id = if comment {
            doc.create_comment(initial).unwrap()
        } else {
            doc.create_text(initial).unwrap()
        };
        // Split a surrogate pair, insert without removal, then clamp a huge count.
        doc.replace_character_data(id, 1, 1, &[0xdc00]).unwrap();
        assert_eq!(value(&doc, id).as_units(), &[0x61, 0xdc00, 0xde80, 0x62]);
        doc.replace_character_data(id, 2, 0, &[0xd800, 0]).unwrap();
        assert_eq!(
            value(&doc, id).as_units(),
            &[0x61, 0xdc00, 0xd800, 0, 0xde80, 0x62]
        );
        doc.replace_character_data(id, 3, usize::MAX, &[0x63])
            .unwrap();
        assert_eq!(value(&doc, id).as_units(), &[0x61, 0xdc00, 0xd800, 0x63]);
        doc.replace_character_data(id, 0, usize::MAX, &[]).unwrap();
        assert!(value(&doc, id).as_units().is_empty());
    }
    assert_eq!(doc.data_bytes(), 0);
}

#[test]
fn growing_and_shrinking_middle_ranges_preserve_the_suffix() {
    let mut doc = document(64);
    let id = doc.create_text("abcdef").unwrap();
    doc.replace_character_data(id, 1, 2, &[0x31, 0x32, 0x33, 0x34])
        .unwrap();
    assert_eq!(value(&doc, id), &DomString::from("a1234def"));
    doc.replace_character_data(id, 2, 4, &[0x58]).unwrap();
    assert_eq!(value(&doc, id), &DomString::from("a1Xef"));
    doc.replace_character_data(id, 1, 3, &[]).unwrap();
    assert_eq!(value(&doc, id), &DomString::from("af"));
    assert_eq!(doc.data_bytes(), 4);
}

#[test]
fn appending_utf8_and_units_preserves_identity_links_and_snapshots() {
    let mut doc = document(128);
    let parent = doc
        .create_element(QualifiedName::html("p").unwrap())
        .unwrap();
    doc.append_child(doc.root(), parent).unwrap();
    let text = doc
        .create_text(DomString::from_units(vec![0xd800]))
        .unwrap();
    let comment = doc.create_comment("after").unwrap();
    doc.append_child(parent, text).unwrap();
    doc.append_child(parent, comment).unwrap();
    let snapshot = value(&doc, text).clone();
    doc.append_utf8_character_data(text, "🚀ع").unwrap();
    doc.append_character_data(text, &[0xdc00]).unwrap();
    assert_eq!(snapshot.as_units(), &[0xd800]);
    assert_eq!(
        value(&doc, text).as_units(),
        &[0xd800, 0xd83d, 0xde80, 0x0639, 0xdc00]
    );
    let prior = value(&doc, text).clone();
    assert_eq!(doc.set_character_data(text, "new").unwrap(), prior);
    doc.append_utf8_character_data(text, "!").unwrap();
    assert_eq!(prior.as_units().len(), 5);
    assert_eq!(value(&doc, text), &DomString::from("new!"));
    assert_eq!(
        doc.children(parent).unwrap().collect::<Vec<_>>(),
        [text, comment]
    );
    assert_eq!(doc.node(text).unwrap().parent(), Some(parent));
    assert_eq!(doc.node(text).unwrap().next_sibling(), Some(comment));
    assert_eq!(doc.node(comment).unwrap().previous_sibling(), Some(text));
    assert_eq!(doc.node_count(), 4);
}

#[test]
fn exact_shared_budget_rejects_growth_and_releases_deleted_payload() {
    let mut doc = document(16);
    let parent = doc
        .create_element(QualifiedName::from_parsed(None, None, "p").unwrap())
        .unwrap(); // One UTF-8 name byte.
    doc.set_attribute(
        parent,
        QualifiedName::from_parsed(None, None, "x").unwrap(),
        "v",
    )
    .unwrap(); // Three bytes: name plus UTF-16 value.
    doc.append_child(doc.root(), parent).unwrap();
    let text = doc.create_text("ab").unwrap();
    let comment = doc.create_comment("z").unwrap(); // Detached nodes are charged.
    doc.append_child(parent, text).unwrap();
    doc.append_utf8_character_data(text, "🚀x").unwrap();
    assert_eq!(doc.data_bytes(), 16);
    let before = doc.dump_tree(parent, 1024).unwrap();
    assert_eq!(
        doc.append_character_data(text, &[0]),
        Err(DomError::DataLimit)
    );
    assert_eq!(
        doc.append_utf8_character_data(text, "🚀"),
        Err(DomError::DataLimit)
    );
    assert_eq!(
        doc.replace_character_data(text, 1, 1, &[1, 2]),
        Err(DomError::DataLimit)
    );
    assert_eq!(doc.dump_tree(parent, 1024).unwrap(), before);
    assert_eq!(value(&doc, comment), &DomString::from("z"));
    assert_eq!(doc.data_bytes(), 16);
    doc.replace_character_data(text, 1, usize::MAX, &[])
        .unwrap();
    assert_eq!(doc.data_bytes(), 8);
    doc.append_utf8_character_data(comment, "🚀🚀").unwrap();
    assert_eq!(doc.data_bytes(), 16);
    doc.destroy_subtree(comment).unwrap();
    assert_eq!(doc.data_bytes(), 6);
    doc.destroy_subtree(parent).unwrap();
    assert_eq!(doc.data_bytes(), 0);
    assert_eq!(doc.node_count(), 1);
}

#[test]
fn empty_edits_validate_offsets_and_work_at_zero_or_full_budget() {
    let mut doc = document(0);
    let id = doc.create_text("").unwrap();
    doc.replace_character_data(id, 0, usize::MAX, &[]).unwrap();
    doc.append_character_data(id, &[]).unwrap();
    doc.append_utf8_character_data(id, "").unwrap();
    for offset in [1, usize::MAX] {
        assert_eq!(
            doc.replace_character_data(id, offset, 0, &[]),
            Err(DomError::InvalidOffset)
        );
    }
    assert_eq!(
        doc.append_character_data(id, &[0]),
        Err(DomError::DataLimit)
    );
    assert_eq!(doc.data_bytes(), 0);
    let mut doc = document(2);
    let id = doc.create_comment("x").unwrap();
    doc.replace_character_data(id, 1, usize::MAX, &[]).unwrap();
    doc.append_utf8_character_data(id, "").unwrap();
    assert_eq!(value(&doc, id), &DomString::from("x"));
    assert_eq!(doc.data_bytes(), 2);
}

#[test]
fn all_entry_points_reject_wrong_kinds_and_invalid_handles_even_for_empty_edits() {
    let mut doc = document(128);
    let other = document(0);
    let element = doc
        .create_element(QualifiedName::html("p").unwrap())
        .unwrap();
    let doctype = doc
        .create_doctype(DocumentType {
            name: "html".into(),
            public_id: DomString::default(),
            system_id: DomString::default(),
        })
        .unwrap();
    let stale = doc.create_text("old").unwrap();
    doc.destroy_subtree(stale).unwrap();
    let replacement = doc.create_text("live").unwrap();
    assert_eq!(stale.slot(), replacement.slot());
    let forged = NodeId::from_parts(doc.id(), replacement.slot(), NonZeroU64::new(99).unwrap());
    let absent = NodeId::from_parts(doc.id(), u32::MAX, NonZeroU64::MIN);
    let cases = [
        (doc.root(), DomError::WrongNodeKind),
        (element, DomError::WrongNodeKind),
        (doctype, DomError::WrongNodeKind),
        (other.root(), DomError::WrongDocument),
        (stale, DomError::InvalidNode),
        (forged, DomError::InvalidNode),
        (absent, DomError::InvalidNode),
    ];
    let bytes = doc.data_bytes();
    for (id, error) in cases {
        assert_eq!(doc.replace_character_data(id, 0, 0, &[]), Err(error));
        assert_eq!(doc.append_character_data(id, &[]), Err(error));
        assert_eq!(doc.append_utf8_character_data(id, ""), Err(error));
        assert_eq!(doc.set_character_data(id, ""), Err(error));
        assert_eq!(doc.data_bytes(), bytes);
    }
    assert_eq!(value(&doc, replacement), &DomString::from("live"));
}

#[test]
fn chunked_utf8_append_matches_whole_run_with_one_text_node() {
    let mut doc = document(65_536);
    let chunked = doc.create_text("").unwrap();
    let whole = doc.create_text("").unwrap();
    let chunks = ["a", "🚀", "ع", "\0", "\r\n", "", "\u{feff}"];
    let mut expected = String::new();
    for i in 0..4096 {
        let chunk = chunks[i % chunks.len()];
        doc.append_utf8_character_data(chunked, chunk).unwrap();
        expected.push_str(chunk);
    }
    doc.append_utf8_character_data(whole, &expected).unwrap();
    assert_eq!(value(&doc, chunked), &DomString::from(expected.as_str()));
    assert_eq!(value(&doc, chunked), value(&doc, whole));
    assert_eq!(doc.data_bytes(), expected.encode_utf16().count() * 4);
    assert_eq!(doc.node_count(), 3);
}

#[test]
fn seeded_edits_match_an_independent_utf16_model_under_a_shared_budget() {
    const LIMIT: usize = 96;
    let mut doc = document(LIMIT);
    let ids = [
        doc.create_text("").unwrap(),
        doc.create_comment("").unwrap(),
    ];
    let mut model = [Vec::<u16>::new(), Vec::<u16>::new()];
    let mut seed = 0xe0d9_4a1c_6b38_275fu64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut accepted = 0;
    let mut limited = 0;
    let mut invalid = 0;
    for _ in 0..2500 {
        let index = (random() % 2) as usize;
        let op = random() % 3;
        let offset = if op == 0 {
            (random() % (model[index].len() as u64 + 3)) as usize
        } else {
            model[index].len()
        };
        let count = if random() % 5 == 0 {
            usize::MAX
        } else {
            (random() % 12) as usize
        };
        let utf8 = ["", "a", "🚀", "ع\0", "many units"][random() as usize % 5];
        let inserted = if op == 2 {
            utf8.encode_utf16().collect::<Vec<_>>()
        } else {
            (0..random() % 8).map(|_| random() as u16).collect()
        };
        // Build a separate prefix/insert/suffix vector, with no in-place moves.
        let expected = if offset > model[index].len() {
            Err(DomError::InvalidOffset)
        } else {
            let end = offset
                + if op == 0 {
                    count.min(model[index].len() - offset)
                } else {
                    0
                };
            let candidate: Vec<u16> = model[index][..offset]
                .iter()
                .chain(&inserted)
                .chain(&model[index][end..])
                .copied()
                .collect();
            if (candidate.len() + model[1 - index].len()) * 2 > LIMIT {
                Err(DomError::DataLimit)
            } else {
                Ok(candidate)
            }
        };
        let actual = match op {
            0 => doc.replace_character_data(ids[index], offset, count, &inserted),
            1 => doc.append_character_data(ids[index], &inserted),
            _ => doc.append_utf8_character_data(ids[index], utf8),
        };
        match expected {
            Ok(candidate) => {
                assert_eq!(actual, Ok(()));
                model[index] = candidate;
                accepted += 1;
            }
            Err(error) => {
                assert_eq!(actual, Err(error));
                if error == DomError::DataLimit {
                    limited += 1;
                } else {
                    invalid += 1;
                }
            }
        }
        for i in 0..2 {
            assert_eq!(value(&doc, ids[i]).as_units(), model[i]);
        }
        assert_eq!(doc.data_bytes(), (model[0].len() + model[1].len()) * 2);
        assert_eq!(doc.node_count(), 3);
    }
    assert!(accepted > 500 && limited > 100 && invalid > 10);
}
