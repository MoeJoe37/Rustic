mod common;

use common::{decode, module, short_len};
use rustic_base::CancellationToken;
use rustic_wasm::{
    IndexSpace, ValidatedModule, ValidationError, ValidationErrorKind as Fault, ValidationLimits,
    validate_module,
};

const EMPTY: &[u8] = &[1, 0x60, 0, 0];
const I32_RESULT: &[u8] = &[1, 0x60, 0, 1, 0x7f];
const END: u8 = 0x0b;

fn function(types: &[u8], declarations: &[(u8, &[u8])], locals: &[u8], code: &[u8]) -> Vec<u8> {
    let body = [locals, code, &[END]].concat();
    let section = [&[1, short_len(&body)], body.as_slice()].concat();
    let mut sections = vec![(1, types), (3, &[1, 0][..])];
    sections.extend_from_slice(declarations);
    sections.push((10, &section));
    module(&sections)
}

fn validate(bytes: &[u8]) -> Result<ValidatedModule<'_>, ValidationError> {
    validate_module(
        decode(bytes).unwrap(),
        ValidationLimits::default(),
        &CancellationToken::default(),
    )
}

fn invalid(bytes: &[u8], kind: Fault, opcode: u8) {
    let error = validate(bytes).unwrap_err();
    assert_eq!(error.kind, kind, "{bytes:02x?}");
    assert_eq!(bytes[error.offset], opcode, "{error}");
}

#[test]
fn multi_value_loops_ifs_and_calls_use_their_declared_signatures() {
    let types = &[
        2, 0x60, 0, 2, 0x7f, 0x7e, 0x60, 2, 0x7f, 0x7e, 2, 0x7f, 0x7e,
    ];
    #[rustfmt::skip]
    let body = [
        0x41, 1, 0x42, 2,   // Initial block parameters.
        0x02, 1, 0x03, 1,   // Block and loop accept and return (i32, i64).
        0x41, 0, 0x0d, 0,   // A conditional branch carries loop parameters.
        0x41, 0, 0x0e, 1, 0, 1, // Both branch table targets accept the pair.
        END, END,
        0x41, 0, 0x04, 1, 0x05, END, // Both arms leave the original pair intact.
    ];
    let module = function(types, &[], &[0], &body);
    assert_eq!(validate(&module).unwrap().module().functions().len(), 1);

    let table = &[(4, &[1, 0x70, 0, 1][..])];
    let types = &[2, 0x60, 0, 1, 0x7f, 0x60, 1, 0x7f, 1, 0x7f];
    let call = function(types, table, &[0], &[0x41, 9, 0x41, 0, 0x11, 1, 0]);
    validate(&call).unwrap();
    let tail = function(types, table, &[0], &[0x41, 9, 0x41, 0, 0x13, 1, 0]);
    validate(&tail).unwrap();
    let wrong_tail = function(
        &[2, 0x60, 0, 1, 0x7f, 0x60, 0, 1, 0x7e],
        table,
        &[0],
        &[0x41, 0, 0x13, 1, 0],
    );
    invalid(&wrong_tail, Fault::InvalidSignature, 0x13);
}

#[test]
fn unreachable_stack_polymorphism_still_checks_known_operands() {
    let bottom = function(I32_RESULT, &[], &[0], &[0x00, 0x6a]);
    validate(&bottom).unwrap();
    let wrong = function(I32_RESULT, &[], &[0], &[0x00, 0x42, 0, 0x6a]);
    invalid(&wrong, Fault::TypeMismatch, 0x6a);
    let underflow = function(EMPTY, &[], &[0], &[0x1a]);
    invalid(&underflow, Fault::StackUnderflow, 0x1a);
    let above_frame = function(EMPTY, &[], &[0], &[0x41, 0, 0x02, 0x40, 0x1a, END, 0x1a]);
    invalid(&above_frame, Fault::StackUnderflow, 0x1a);
    let extra = function(EMPTY, &[], &[0], &[0x41, 0]);
    invalid(&extra, Fault::ExtraOperands, END);
    let invalid_if = function(I32_RESULT, &[], &[0], &[0x41, 1, 0x04, 0x7f, 0x41, 2, END]);
    invalid(&invalid_if, Fault::InvalidSignature, END);
    let invalid_label = function(EMPTY, &[], &[0], &[0x0c, 1]);
    invalid(&invalid_label, Fault::InvalidIndex(IndexSpace::Label), 0x0c);

    let mismatched_branch_table = function(
        I32_RESULT,
        &[],
        &[0],
        &[0x02, 0x40, 0x41, 7, 0x41, 0, 0x0e, 1, 0, 1, END, 0x41, 7],
    );
    invalid(&mismatched_branch_table, Fault::InvalidSignature, 0x0e);
}

#[test]
fn import_indexes_constants_and_reference_declarations_are_checked() {
    let imported_fn = &[1, 1, b'm', 1, b'f', 0, 0];
    let bytes = module(&[
        (1, EMPTY),
        (2, imported_fn),
        (3, &[1, 0]),
        (7, &[1, 1, b'x', 0, 1]),
        (10, &[1, 4, 0, 0x10, 0, END]),
    ]);
    validate(&bytes).unwrap();

    let globals = &[
        2, 0x7f, 0, 0x41, 5, END, 0x7f, 0, 0x23, 0, 0x41, 1, 0x6a, END,
    ];
    validate(&module(&[(6, globals)])).unwrap();
    let forward = module(&[(6, &[1, 0x7f, 0, 0x23, 0, END])]);
    invalid(&forward, Fault::InvalidIndex(IndexSpace::Global), 0x23);
    let mutable = module(&[(6, &[2, 0x7f, 1, 0x41, 0, END, 0x7f, 0, 0x23, 0, END])]);
    invalid(&mutable, Fault::NonConstant, 0x23);
    let write = function(EMPTY, &[(6, globals)], &[0], &[0x41, 1, 0x24, 0]);
    invalid(&write, Fault::ImmutableGlobal, 0x24);

    let body = &[0xd2, 0, 0xd1, 0x1a];
    invalid(
        &function(EMPTY, &[], &[0], body),
        Fault::UndeclaredFunction,
        0xd2,
    );
    let element = &[(9, &[1, 3, 0, 1, 0][..])];
    validate(&function(EMPTY, element, &[0], body)).unwrap();
    let table = &[(4, &[1, 0x40, 0, 0x70, 0, 1, 0xd2, 0, END][..])];
    validate(&function(EMPTY, table, &[0], body)).unwrap();
    let wrong_ref = function(
        EMPTY,
        &[(4, &[1, 0x6f, 0, 1])],
        &[0],
        &[0x41, 0, 0xd0, 0x70, 0x26, 0],
    );
    invalid(&wrong_ref, Fault::TypeMismatch, 0x26);
}

#[test]
fn validation_rejects_incompatible_declarations_and_memory_immediates() {
    for (bytes, fault, value) in [
        (module(&[(5, &[1, 1, 2, 1])]), Fault::InvalidLimits, 1),
        (
            module(&[(4, &[1, 0x70, 5, 2, 1])]),
            Fault::InvalidLimits,
            0x70,
        ),
        (
            module(&[(1, &[1, 0x60, 1, 0x7b, 0])]),
            Fault::UnsupportedType,
            0x60,
        ),
        (
            module(&[(1, I32_RESULT), (13, &[1, 0, 0])]),
            Fault::InvalidSignature,
            0,
        ),
        (
            function(I32_RESULT, &[(8, &[0])], &[0], &[0x41, 0]),
            Fault::InvalidSignature,
            0,
        ),
        (
            function(EMPTY, &[(7, &[2, 1, b'f', 0, 0, 1, b'f', 0, 0])], &[0], &[]),
            Fault::DuplicateExport,
            1,
        ),
        (
            module(&[(7, &[1, 0, 2, 0])]),
            Fault::InvalidIndex(IndexSpace::Memory),
            0,
        ),
    ] {
        invalid(&bytes, fault, value);
    }
    let memory = &[(5, &[1, 0, 1][..])];
    for immediate in [&[3, 0][..], &[2, 0x80, 0x80, 0x80, 0x80, 0x10]] {
        let code = [&[0x41, 0, 0x28][..], immediate, &[0x1a]].concat();
        invalid(
            &function(EMPTY, memory, &[0], &code),
            Fault::InvalidLimits,
            0x28,
        );
    }
    let copy = &[0x41, 0, 0x41, 0, 0x41, 1, 0xfc, 8, 0, 0];
    invalid(
        &function(EMPTY, memory, &[0], copy),
        Fault::MissingDataCount,
        0xfc,
    );
    let bytes = module(&[
        (1, EMPTY),
        (3, &[1, 0]),
        memory[0],
        (12, &[1]),
        (10, &[&[1, 12, 0][..], copy, &[END]].concat()),
        (11, &[1, 1, 1, 42]),
    ]);
    validate(&bytes).unwrap();
}

#[test]
fn memory64_checks_limits_offsets_and_each_bulk_operand_width() {
    let wide = &[(5, &[1, 4, 1][..])];
    let load = &[0x42, 0, 0x28, 2, 0x80, 0x80, 0x80, 0x80, 0x10, 0x1a];
    validate(&function(EMPTY, wide, &[0], load)).unwrap();
    invalid(
        &function(EMPTY, wide, &[0], &[0x41, 0, 0x28, 2, 0, 0x1a]),
        Fault::TypeMismatch,
        0x28,
    );
    let data = module(&[(5, &[1, 4, 1]), (11, &[1, 0, 0x41, 0, END, 0])]);
    assert_eq!(validate(&data).unwrap_err().kind, Fault::TypeMismatch);

    let mixed = &[(5, &[2, 4, 1, 0, 1][..])];
    let copy = [0x42, 0, 0x41, 0, 0x41, 0, 0xfc, 10, 0, 1];
    validate(&function(EMPTY, mixed, &[0], &copy)).unwrap();
    let mut invalid_length = copy;
    invalid_length[4] = 0x42;
    invalid(
        &function(EMPTY, mixed, &[0], &invalid_length),
        Fault::TypeMismatch,
        0xfc,
    );
    let maximum = module(&[(5, &[1, 5, 0, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x40])]);
    validate(&maximum).unwrap();
    let too_large = module(&[(5, &[1, 5, 0, 0x81, 0x80, 0x80, 0x80, 0x80, 0x80, 0x40])]);
    invalid(&too_large, Fault::InvalidLimits, 5);
}

#[test]
fn table64_checks_table_indices_and_the_width_of_mixed_copies() {
    let tables = &[(4, &[2, 0x70, 4, 2, 0x70, 0, 2][..])];
    let wide = &[0x42, 0, 0x25, 0, 0x1a];
    validate(&function(EMPTY, tables, &[0], wide)).unwrap();
    invalid(
        &function(EMPTY, tables, &[0], &[0x41, 0, 0x25, 0, 0x1a]),
        Fault::TypeMismatch,
        0x25,
    );
    invalid(
        &function(EMPTY, tables, &[0], &[0x42, 0, 0x25, 1, 0x1a]),
        Fault::TypeMismatch,
        0x25,
    );
    let copy = [0x42, 0, 0x41, 0, 0x41, 0, 0xfc, 14, 0, 1];
    validate(&function(EMPTY, tables, &[0], &copy)).unwrap();
    let mut wrong_count = copy;
    wrong_count[4] = 0x42;
    invalid(
        &function(EMPTY, tables, &[0], &wrong_count),
        Fault::TypeMismatch,
        0xfc,
    );
    let wrong_offset = module(&[tables[0], (9, &[1, 0, 0x41, 0, END, 0])]);
    assert_eq!(
        validate(&wrong_offset).unwrap_err().kind,
        Fault::TypeMismatch
    );

    #[rustfmt::skip]
    let largest = [1, 0x70, 5, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    validate(&module(&[(4, &largest)])).unwrap();
    invalid(
        &module(&[(4, &[1, 0x70, 1, 0, 0x80, 0x80, 0x80, 0x80, 0x10])]),
        Fault::InvalidLimits,
        0x70,
    );
}

#[test]
fn local_groups_numeric_reinterpretations_and_limits_are_enforced() {
    let locals = &[3, 2, 0x7f, 0, 0x7d, 1, 0x7e];
    let bytes = function(
        EMPTY,
        &[],
        locals,
        &[
            0x20, 1, 0x21, 0, 0x20, 2, 0xa7, 0x1a, 0x43, 0, 0, 0, 0, 0xbc, 0x1a,
        ],
    );
    validate(&bytes).unwrap();
    invalid(
        &function(EMPTY, &[], locals, &[0x20, 3, 0x1a]),
        Fault::InvalidIndex(IndexSpace::Local),
        0x20,
    );

    let token = CancellationToken::default();
    for limits in [
        ValidationLimits {
            locals: 2,
            ..ValidationLimits::default()
        },
        ValidationLimits {
            operands: 0,
            ..ValidationLimits::default()
        },
        ValidationLimits {
            work: 15,
            ..ValidationLimits::default()
        },
    ] {
        let error = validate_module(decode(&bytes).unwrap(), limits, &token).unwrap_err();
        assert_eq!(error.kind, Fault::LimitExceeded);
    }
    token.cancel();
    assert_eq!(
        validate_module(decode(&bytes).unwrap(), ValidationLimits::default(), &token)
            .unwrap_err()
            .kind,
        Fault::Cancelled,
    );
}
