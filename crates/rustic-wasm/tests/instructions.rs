use rustic_base::CancellationToken;
use rustic_wasm::{
    BinaryError, BinaryErrorKind, BlockType, FunctionBody, Immediate, Instruction,
    InstructionLimits, Opcode, decode_instructions,
};

fn body(bytes: &[u8]) -> FunctionBody<'_> {
    FunctionBody {
        type_index: 0,
        locals: Vec::new(),
        offset: 100,
        instructions: bytes,
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<Instruction>, BinaryError> {
    decode_instructions(
        &body(bytes),
        InstructionLimits::default(),
        &CancellationToken::default(),
    )
}

#[test]
fn decodes_control_and_vector_operands_with_absolute_offsets() {
    let ops = decode(&[
        0x02, 0xc0, 0, // Block with type index 64.
        0x04, 0x7f, // If with an i32 result.
        0x0e, 2, 0, 3, 4,    // Branch table.
        0x05, // Else.
        0x1c, 1, 0x7f, // Typed select.
        0x0b, 0x0b, 0x0b, // Close if, block and function.
    ])
    .unwrap();
    assert_eq!(
        ops[0],
        Instruction {
            offset: 100,
            opcode: Opcode::Core(0x02),
            immediate: Immediate::Block(BlockType::TypeIndex(64))
        }
    );
    assert_eq!(
        ops[1].immediate,
        Immediate::Block(BlockType::Result(rustic_wasm::ValueType::I32))
    );
    assert_eq!(ops[2].offset, 105);
    assert_eq!(
        ops[2].immediate,
        Immediate::BranchTable {
            targets: vec![0, 3],
            default: 4
        }
    );
    assert_eq!(
        ops[4].immediate,
        Immediate::Select(vec![rustic_wasm::ValueType::I32])
    );
    assert_eq!(ops.last().unwrap().offset, 116);
}

#[test]
fn signed_limits_floating_bits_and_memory_indexes_survive_decoding() {
    let bytes = [
        0x41, 0x80, 0x80, 0x80, 0x80, 0x78, // i32::MIN
        0x42, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x7f, // i64::MIN
        0x43, 0x01, 0, 0xc0, 0x7f, // f32 NaN payload.
        0x28, 0x42, 0x01, 0x80, 0x80, 0x80, 0x80, 0x10, // align 2, memory 1, offset 2^32.
        0xfc, 8, 9, 2, // memory.init data index and memory index.
        0xd0, 0x70, // ref.null func.
        0x0b,
    ];
    let ops = decode(&bytes).unwrap();
    assert_eq!(ops[0].immediate, Immediate::I32(i32::MIN));
    assert_eq!(ops[1].immediate, Immediate::I64(i64::MIN));
    assert_eq!(ops[2].immediate, Immediate::F32(0x7fc0_0001));
    assert_eq!(
        ops[3].immediate,
        Immediate::Memory {
            memory: 1,
            alignment: 2,
            offset: 1 << 32
        }
    );
    assert_eq!(ops[4].opcode, Opcode::Misc(8));
    assert_eq!(ops[4].immediate, Immediate::Indices(9, 2));
    assert_eq!(ops[5].immediate, Immediate::HeapType(-16));
}

#[test]
fn malformed_and_unsupported_instructions_have_distinct_errors() {
    for (bytes, error) in [
        (&[0x05, 0x0b][..], BinaryErrorKind::InvalidControl),
        (
            &[0x04, 0x40, 0x05, 0x05, 0x0b, 0x0b],
            BinaryErrorKind::InvalidControl,
        ),
        (&[0x02, 0x40, 0x0b], BinaryErrorKind::MissingEnd),
        (&[0x0b, 0x0b], BinaryErrorKind::TrailingBytes),
        (&[0x41, 0x80], BinaryErrorKind::UnexpectedEnd),
        (
            &[0x41, 0x80, 0x80, 0x80, 0x80, 0x08, 0x0b],
            BinaryErrorKind::InvalidLeb,
        ),
        (
            &[0x28, 0x80, 0x01, 0x00, 0x0b],
            BinaryErrorKind::InvalidInstruction,
        ),
        (&[0xd0, 0x68, 0x0b], BinaryErrorKind::InvalidType),
        (&[0xff, 0x0b], BinaryErrorKind::InvalidInstruction),
        (&[0xfd, 0x0b], BinaryErrorKind::UnsupportedInstruction),
    ] {
        assert_eq!(decode(bytes).unwrap_err().kind, error, "{bytes:?}");
    }
}

#[test]
fn instruction_limits_apply_to_nesting_and_cumulative_vector_allocations() {
    let source = body(&[0x03, 0x40, 0x0e, 1, 0, 0, 0x1c, 1, 0x7f, 0x0b, 0x0b]);
    let defaults = InstructionLimits::default();
    let token = CancellationToken::default();
    for limits in [
        InstructionLimits {
            instructions: 4,
            ..defaults
        },
        InstructionLimits {
            nesting: 0,
            ..defaults
        },
        InstructionLimits {
            vector_entries: 1,
            ..defaults
        },
    ] {
        assert_eq!(
            decode_instructions(&source, limits, &token)
                .unwrap_err()
                .kind,
            BinaryErrorKind::LimitExceeded
        );
    }
    token.cancel();
    assert_eq!(
        decode_instructions(&source, defaults, &token).unwrap_err(),
        BinaryError {
            offset: 100,
            kind: BinaryErrorKind::Cancelled
        }
    );
}
