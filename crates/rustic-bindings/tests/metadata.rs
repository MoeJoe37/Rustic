use rustic_bindings::metadata::{self, ConvertedValue, Member, ReturnType, ValueType};
use rustic_bindings::webidl::from_dom_string;
use rustic_dom::{Document, DocumentLimits, DomString, NodeKind};
use rustic_js::runtime::{Abrupt, Agent, Value};

#[test]
fn generated_metadata_keeps_inheritance_and_member_flags() {
    let names: Vec<_> = metadata::interfaces()
        .iter()
        .map(|entry| entry.name())
        .collect();
    assert_eq!(names, ["CharacterData", "Text", "CDATASection", "Comment"]);
    let character_data = metadata::interface("CharacterData").unwrap();
    assert_eq!(character_data.parent(), Some("Node"));
    assert!(metadata::interface("Node").is_none());
    assert_eq!(
        character_data.members()[1],
        Member::Attribute {
            name: "length",
            value_type: ValueType::UnsignedLong,
            readonly: true,
        }
    );

    let text = metadata::interface("Text").unwrap();
    assert_eq!(text.parent(), Some("CharacterData"));
    assert!(matches!(
        text.members()[1],
        Member::Operation {
            name: "splitText",
            returns: ReturnType::Interface("Text"),
            new_object: true,
            arguments: [_],
        }
    ));
    let cdata = metadata::interface("CDATASection").unwrap();
    assert_eq!(cdata.parent(), Some("Text"));
    assert!(cdata.members().is_empty());
}

#[test]
fn generated_argument_metadata_drives_character_data_conversion() {
    let mut agent = Agent::new().unwrap();
    let mut document = Document::new(DocumentLimits::default()).unwrap();
    let node = document.create_text("abc").unwrap();
    let character_data = metadata::interface("CharacterData").unwrap();
    let Member::Operation {
        name: "replaceData",
        returns: ReturnType::Undefined,
        arguments: [offset, count, data],
        ..
    } = character_data.members().last().unwrap()
    else {
        panic!("expected the pinned replaceData signature");
    };
    assert_eq!(offset.name(), "offset");
    assert!(!data.is_optional());
    assert!(matches!(
        data.convert(&mut agent, None),
        Err(Abrupt::Throw(_))
    ));

    let dom_value = DomString::from_units(vec![0xd800]);
    let js_value = from_dom_string(&mut agent, &dom_value).unwrap();
    let ConvertedValue::UnsignedLong(offset) = offset
        .convert(&mut agent, Some(Value::number(-4_294_967_295.0)))
        .unwrap()
    else {
        panic!("expected wrapping offset");
    };
    let ConvertedValue::UnsignedLong(count) =
        count.convert(&mut agent, Some(Value::number(1.9))).unwrap()
    else {
        panic!("expected truncated count");
    };
    let ConvertedValue::DomString(data) = data.convert(&mut agent, Some(js_value)).unwrap() else {
        panic!("expected DOMString data");
    };
    document
        .replace_character_data(
            node,
            usize::try_from(offset).unwrap(),
            usize::try_from(count).unwrap(),
            data.as_units(),
        )
        .unwrap();
    let NodeKind::Text(data) = document.node(node).unwrap().kind() else {
        panic!("expected a text node");
    };
    assert_eq!(data.as_units(), &[0x61, 0xd800, 0x63]);

    let Member::Attribute {
        name: "data",
        value_type,
        readonly: false,
    } = character_data.members()[0]
    else {
        panic!("expected the pinned data attribute");
    };
    assert_eq!(
        value_type.convert(&mut agent, Value::NULL).unwrap(),
        ConvertedValue::DomString(DomString::default())
    );
}

#[test]
fn generated_text_constructor_distinguishes_defaults_from_null() {
    let mut agent = Agent::new().unwrap();
    for name in ["Text", "Comment"] {
        let Member::Constructor([data]) = metadata::interface(name).unwrap().members()[0] else {
            panic!("expected one constructor argument");
        };
        assert_eq!(data.name(), "data");
        assert!(data.is_optional());
        for defaulted in [None, Some(Value::UNDEFINED)] {
            assert_eq!(
                data.convert(&mut agent, defaulted).unwrap(),
                ConvertedValue::DomString(DomString::default())
            );
        }
        assert_eq!(
            data.convert(&mut agent, Some(Value::NULL)).unwrap(),
            ConvertedValue::DomString(DomString::from("null"))
        );
    }
}
