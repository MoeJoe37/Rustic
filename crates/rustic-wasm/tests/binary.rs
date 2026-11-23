use rustic_base::CancellationToken;
use rustic_wasm::{
    BinaryError, BinaryErrorKind, BinaryLimits, SectionKind, ValueType, scan_module,
};

const HEADER: [u8; 8] = *b"\0asm\x01\0\0\0";

fn module(payload: &[u8]) -> Vec<u8> {
    [HEADER.as_slice(), payload].concat()
}

fn decode(bytes: &[u8]) -> Result<rustic_wasm::BinaryModule<'_>, BinaryError> {
    scan_module(
        bytes,
        BinaryLimits::default(),
        &CancellationToken::default(),
    )
}

#[test]
fn borrowed_custom_and_function_spans_point_into_the_original_module() {
    let bytes = module(&[
        0, 5, 3, b'n', b'a', b'm', 42, 1, 4, 1, 0x60, 0, 0, 3, 3, 1, 0x80,
        0, // A padded zero type index.
        10, 9, 1, 7, 2, 3, 0x7f, 1, 0x63, 0x70, 0x0b, 0, 2, 0, 7,
    ]);
    let parsed = decode(&bytes).unwrap();
    assert_eq!(parsed.sections.len(), 5);
    assert_eq!(parsed.sections[0].kind, SectionKind::Custom);
    assert_eq!(parsed.sections[0].name, Some("nam"));
    assert_eq!(parsed.sections[0].offset, 14);
    assert_eq!(parsed.sections[0].bytes, &[42]);
    assert_eq!(parsed.sections[4].name, Some(""));
    let body = &parsed.functions[0];
    assert_eq!(body.type_index, 0);
    assert_eq!(body.locals[0].count, 3);
    assert_eq!(body.locals[0].value_type, ValueType::I32);
    assert_eq!(
        body.locals[1].value_type,
        ValueType::Ref {
            nullable: true,
            heap_type: -16
        }
    );
    assert_eq!(body.instructions, &[0x0b]);
    assert!(std::ptr::eq(
        body.instructions,
        &bytes[body.offset..body.offset + 1]
    ));
}

#[test]
fn leb_padding_is_accepted_but_invalid_upper_bits_and_continuations_fail() {
    let padded_length = module(&[0, 0x81, 0x80, 0x80, 0x80, 0, 0]);
    assert_eq!(decode(&padded_length).unwrap().sections[0].name, Some(""));
    for size in [[0x80, 0x80, 0x80, 0x80, 0x10], [0x80; 5]] {
        let bytes = module(&[&[0], size.as_slice()].concat());
        let err = decode(&bytes).unwrap_err();
        assert_eq!(err.kind, BinaryErrorKind::InvalidLeb);
        assert_eq!(err.offset, 13);
    }
    let reference = |value: &[u8]| {
        let body_size = (value.len() + 4) as u8;
        module(
            &[
                &[3, 2, 1, 0, 10, body_size + 2, 1, body_size, 1, 1, 0x64],
                value,
                &[0x0b],
            ]
            .concat(),
        )
    };
    let max_index = reference(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
    assert_eq!(
        decode(&max_index).unwrap().functions[0].locals[0].value_type,
        ValueType::Ref {
            nullable: false,
            heap_type: i64::from(u32::MAX)
        }
    );
    let invalid_padding = reference(&[0xff, 0xff, 0xff, 0xff, 0x30]);
    assert_eq!(
        decode(&invalid_padding).unwrap_err().kind,
        BinaryErrorKind::InvalidLeb
    );
    let padded_abstract = reference(&[0xf0, 0x7f]);
    assert_eq!(
        decode(&padded_abstract).unwrap_err().kind,
        BinaryErrorKind::InvalidType
    );
}

#[test]
fn framing_reports_bad_lengths_ordering_and_mismatched_counts() {
    for (chunk, kind) in [
        (&[0, 8, 0][..], BinaryErrorKind::UnexpectedEnd),
        (&[0, 3, 2, b'a', 0xff], BinaryErrorKind::InvalidName),
        (
            &[1, 1, 0, 0, 1, 0, 1, 1, 0],
            BinaryErrorKind::InvalidSection,
        ),
        (&[6, 1, 0, 13, 1, 0], BinaryErrorKind::InvalidSection),
        (&[255, 0], BinaryErrorKind::InvalidSection),
        (&[3, 3, 1, 0, 0], BinaryErrorKind::TrailingBytes),
        (&[3, 2, 1, 0], BinaryErrorKind::FunctionCountMismatch),
        (&[10, 1, 1], BinaryErrorKind::FunctionCountMismatch),
        (&[12, 1, 1, 11, 1, 0], BinaryErrorKind::DataCountMismatch),
        (
            &[3, 2, 1, 0, 10, 4, 1, 3, 0, 0x0b],
            BinaryErrorKind::UnexpectedEnd,
        ),
        (
            &[3, 2, 1, 0, 10, 4, 1, 2, 0, 1],
            BinaryErrorKind::MissingEnd,
        ),
        (
            &[3, 2, 1, 0, 10, 6, 1, 4, 1, 1, 0x68, 0x0b],
            BinaryErrorKind::InvalidType,
        ),
    ] {
        assert_eq!(decode(&module(chunk)).unwrap_err().kind, kind, "{chunk:?}");
    }
    let bytes = module(&[5, 1, 0, 13, 1, 0, 6, 1, 0, 12, 1, 0, 10, 1, 0, 11, 1, 0]);
    assert_eq!(decode(&bytes).unwrap().data_count, Some(0));
    assert_eq!(
        decode(b"\0asm\x02\0\0\0").unwrap_err().kind,
        BinaryErrorKind::UnsupportedVersion
    );
    assert_eq!(
        decode(b"\0bad\x01\0\0\0").unwrap_err().kind,
        BinaryErrorKind::BadMagic
    );
}

#[test]
fn enforces_admission_limits_and_polls_cancellation_without_executing() {
    let source = module(&[3, 2, 1, 0, 10, 6, 1, 4, 1, 2, 0x7f, 0x0b]);
    let defaults = BinaryLimits::default();
    let token = CancellationToken::default();
    for limits in [
        BinaryLimits {
            module_bytes: source.len() - 1,
            ..defaults
        },
        BinaryLimits {
            sections: 1,
            ..defaults
        },
        BinaryLimits {
            functions: 0,
            ..defaults
        },
        BinaryLimits {
            local_groups: 0,
            ..defaults
        },
        BinaryLimits {
            locals_per_function: 1,
            ..defaults
        },
    ] {
        assert_eq!(
            scan_module(&source, limits, &token).unwrap_err().kind,
            BinaryErrorKind::LimitExceeded
        );
    }
    token.cancel();
    assert_eq!(
        scan_module(&source, defaults, &token).unwrap_err(),
        BinaryError {
            offset: 0,
            kind: BinaryErrorKind::Cancelled
        }
    );
}
