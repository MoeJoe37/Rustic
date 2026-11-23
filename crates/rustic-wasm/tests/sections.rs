mod common;

use common::{decode, module};
use rustic_base::CancellationToken;
use rustic_wasm::{
    AddressType, BinaryErrorKind as Fault, DecodeLimits, ElementItems, ElementMode, ExportIndex,
    ImportType, InstructionLimits, ValueType, decode_module,
};

#[test]
fn declarations_keep_module_order_and_borrow_names_and_data() {
    #[rustfmt::skip]
    let bytes = module(&[
        (0, b"\x03abc\xfe"),
        (1, &[2, 0x60, 1, 0x7f, 1, 0x7e, 0x60, 0, 0]),
        (2, b"\x05\x01m\x02fn\0\0\x01m\x02tb\x01\x70\0\x01\x01m\x02me\x02\x01\x01\x02\x01m\x02gl\x03\x7f\0\x01m\x02tg\x04\0\x01"),
        (3, &[1, 1]),
        (4, &[1, 0x40, 0, 0x70, 0, 1, 0xd0, 0x70, 0x0b]),
        (5, &[1, 0, 1]),
        (13, &[1, 0, 1]),
        (6, &[1, 0x7f, 0, 0x41, 3, 0x0b]),
        (7, b"\x05\x01f\0\x01\x01t\x01\x01\x01m\x02\x01\x01g\x03\x01\x01e\x04\x01"),
        (8, &[1]),
        (9, &[1, 2, 1, 0x41, 0, 0x0b, 0, 1, 1]),
        (12, &[3]),
        (10, &[1, 2, 0, 0x0b]),
        (11, &[3, 0, 0x41, 0, 0x0b, 0, 1, 1, 42, 2, 1, 0x41, 0, 0x0b, 2, 7, 8]),
    ]);
    let parsed = decode(&bytes).unwrap();
    let declarations = parsed.declarations();
    assert_eq!(parsed.custom_sections()[0].name, Some("abc"));
    assert_eq!(parsed.custom_sections()[0].bytes, &[0xfe]);
    assert_eq!(declarations.types[0].value.params, [ValueType::I32]);
    assert_eq!(declarations.types[0].value.results, [ValueType::I64]);
    let import = &declarations.imports[0];
    assert_eq!(import.value.ty, ImportType::Function(0));
    assert!(std::ptr::eq(
        import.value.name.as_bytes(),
        &bytes[import.offset + 3..import.offset + 5],
    ));
    assert!(matches!(
        declarations.imports[1].value.ty,
        ImportType::Table(_)
    ));
    assert!(matches!(
        declarations.imports[2].value.ty,
        ImportType::Memory(_)
    ));
    assert!(matches!(
        declarations.imports[3].value.ty,
        ImportType::Global(_)
    ));
    assert_eq!(declarations.imports[4].value.ty, ImportType::Tag(1));
    assert_eq!(
        declarations
            .exports
            .iter()
            .map(|e| e.value.target)
            .collect::<Vec<_>>(),
        [
            ExportIndex::Function(1),
            ExportIndex::Table(1),
            ExportIndex::Memory(1),
            ExportIndex::Global(1),
            ExportIndex::Tag(1),
        ]
    );
    assert_eq!(
        declarations.tables[0]
            .value
            .initializer
            .as_ref()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        declarations.memories[0].value.address_type,
        AddressType::I32
    );
    assert_eq!(declarations.tags[0].value, 1);
    assert_eq!(declarations.globals[0].value.initializer.len(), 2);
    assert_eq!(declarations.start.as_ref().unwrap().value, 1);
    assert_eq!(parsed.functions()[0].value.type_index, 1);
    assert_eq!(declarations.data_count, Some(3));
    assert!(declarations.data[0].value.bytes.is_empty());
    assert!(declarations.data[1].value.active.is_none());
    assert_eq!(declarations.data[1].value.bytes, &[42]);
    let explicit = &declarations.data[2].value;
    assert_eq!(explicit.active.as_ref().unwrap().target, 1);
    assert!(std::ptr::eq(explicit.bytes, &bytes[bytes.len() - 2..]));
    rustic_wasm::validate_module(parsed, Default::default(), &CancellationToken::default())
        .unwrap();
}

#[test]
fn all_eight_element_flag_encodings_preserve_mode_and_initializer_form() {
    #[rustfmt::skip]
    let elements = [
        8,
        0, 0x41, 0, 0x0b, 1, 0,
        1, 0, 1, 0,
        2, 1, 0x41, 0, 0x0b, 0, 1, 0,
        3, 0, 1, 0,
        4, 0x41, 0, 0x0b, 1, 0xd2, 0, 0x0b,
        5, 0x70, 1, 0xd2, 0, 0x0b,
        6, 1, 0x41, 0, 0x0b, 0x70, 1, 0xd2, 0, 0x0b,
        7, 0x70, 1, 0xd2, 0, 0x0b,
    ];
    let bytes = module(&[(9, &elements)]);
    let parsed = decode(&bytes).unwrap();
    for (flag, segment) in parsed.declarations().elements.iter().enumerate() {
        match (&segment.value.mode, flag % 4) {
            (ElementMode::Active(active), mode @ (0 | 2)) => {
                assert_eq!(active.target, u32::from(mode == 2));
                assert_eq!(active.offset.len(), 2);
            }
            (ElementMode::Passive, 1) | (ElementMode::Declarative, 3) => {}
            _ => panic!("unexpected element segment mode for flag {flag}"),
        }
        match &segment.value.items {
            ElementItems::Functions(indexes) if flag < 4 => assert_eq!(indexes, &[0]),
            ElementItems::Expressions(expressions) if flag >= 4 => {
                assert_eq!(expressions.len(), 1);
                assert_eq!(expressions[0].len(), 2);
            }
            _ => panic!("unexpected initializer form for flag {flag}"),
        }
    }
}

#[test]
fn malformed_sections_are_located_separately_from_unsupported_types() {
    for (id, payload, kind) in [
        (1, &[1, 0x4e][..], Fault::UnsupportedType),
        (2, &[1, 1, 0xff, 0, 0, 0], Fault::InvalidName),
        (2, &[1, 0, 0, 5], Fault::InvalidDescriptor),
        (5, &[1, 3, 0, 0], Fault::UnsupportedType),
        (6, &[1, 0x7f, 2], Fault::InvalidType),
        (7, &[1, 0, 5, 0], Fault::InvalidDescriptor),
        (8, &[0, 1], Fault::TrailingBytes),
        (9, &[1, 8], Fault::InvalidSegment),
        (11, &[1, 3], Fault::InvalidSegment),
    ] {
        let bytes = module(&[(id, payload)]);
        let error = decode(&bytes).unwrap_err();
        assert_eq!(error.kind, kind, "{bytes:02x?}");
        assert!((10..=bytes.len()).contains(&error.offset));
    }
}

#[test]
fn instruction_and_declaration_budgets_accumulate_across_expressions() {
    let bytes = module(&[
        (1, &[1, 0x60, 1, 0x7f, 1, 0x7e]),
        (6, &[2, 0x7f, 0, 0x41, 0, 0x0b, 0x7f, 0, 0x41, 0, 0x0b]),
    ]);
    let token = CancellationToken::default();
    for limits in [
        DecodeLimits {
            declaration_entries: 4,
            ..DecodeLimits::default()
        },
        DecodeLimits {
            code: InstructionLimits {
                instructions: 3,
                ..InstructionLimits::default()
            },
            ..DecodeLimits::default()
        },
    ] {
        assert_eq!(
            decode_module(&bytes, limits, &token).unwrap_err().kind,
            Fault::LimitExceeded
        );
    }
    token.cancel();
    assert_eq!(
        decode_module(&bytes, DecodeLimits::default(), &token)
            .unwrap_err()
            .kind,
        Fault::Cancelled,
    );
}
