use std::fmt::Write;

use rustic_dom::{DomString, NodeKind};
use rustic_html::HtmlDocumentParser;

mod fixtures {
    include!("fixtures/tree_cases.rs");
}

fn utf8(value: &DomString) -> String {
    String::from_utf16(value.as_units()).expect("Unicode corpus has scalar values")
}

fn parse(input: &[u8], sizes: impl IntoIterator<Item = usize>) -> String {
    let mut parser = HtmlDocumentParser::default();
    // Upstream input is Unicode, separate from the byte decoder's transport BOM.
    parser.feed(b"\xef\xbb\xbf").unwrap();
    let mut offset = 0;
    for size in sizes {
        parser.feed(&input[offset..offset + size]).unwrap();
        offset += size;
    }
    assert_eq!(offset, input.len());
    let result = parser.finish().unwrap();
    let doc = result.document();
    let mut output = String::new();
    for (id, depth) in doc.walk(doc.root()).unwrap().skip(1) {
        let prefix = format!("| {}", "  ".repeat(depth - 1));
        match doc.node(id).unwrap().kind() {
            NodeKind::Element(element) => {
                writeln!(output, "{prefix}<{}>", element.name().local()).unwrap();
                let mut attributes = element.attributes().iter().collect::<Vec<_>>();
                attributes.sort_by_key(|a| a.name().local());
                for attribute in attributes {
                    assert_eq!(attribute.name().namespace(), None);
                    writeln!(
                        output,
                        "{prefix}  {}=\"{}\"",
                        attribute.name().local(),
                        utf8(attribute.value())
                    )
                    .unwrap();
                }
            }
            NodeKind::Text(value) => writeln!(output, "{prefix}\"{}\"", utf8(value)).unwrap(),
            NodeKind::Comment(value) => {
                writeln!(output, "{prefix}<!-- {} -->", utf8(value)).unwrap()
            }
            NodeKind::DocumentType(value) => {
                let (name, public, system) = (
                    utf8(&value.name),
                    utf8(&value.public_id),
                    utf8(&value.system_id),
                );
                if public.is_empty() && system.is_empty() {
                    writeln!(output, "{prefix}<!DOCTYPE {name}>").unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}<!DOCTYPE {name} \"{public}\" \"{system}\">"
                    )
                    .unwrap();
                }
            }
            NodeKind::Document => unreachable!(),
        }
    }
    output
}

#[test]
fn pinned_tree_subset_matches_expected_trees_at_every_byte_split() {
    let mut splits = 0;
    for &(index, source, expected) in fixtures::TREE_CASES {
        let bytes = source.as_bytes();
        assert_eq!(parse(bytes, [bytes.len()]), expected, "record {index}");
        for split in 0..=bytes.len() {
            assert_eq!(
                parse(bytes, [split, bytes.len() - split]),
                expected,
                "record {index} split {split}"
            );
            splits += 1;
        }
        assert_eq!(
            parse(bytes, std::iter::repeat_n(1, bytes.len())),
            expected,
            "record {index} one-byte feeds"
        );
    }
    assert_eq!(fixtures::TREE_CASES.len(), 45);
    assert_eq!(splits, fixtures::TREE_SPLITS);
}
