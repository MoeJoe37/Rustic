use rustic_base::CancellationToken;

use crate::binary::{FunctionBody, ValueType, heap_type, is_value_type, poll, reserve, value_type};
use crate::reader::{BinaryError, BinaryErrorKind as Fault, Reader};

/// Admission limits for decoding instructions from one function body.
#[derive(Clone, Copy, Debug)]
pub struct InstructionLimits {
    /// Maximum instruction count including structural delimiters.
    pub instructions: usize,
    /// Maximum nesting of explicit block, loop and if frames.
    pub nesting: usize,
    /// Total decoded vector operands in branch tables and typed selects.
    pub vector_entries: usize,
}

impl Default for InstructionLimits {
    fn default() -> Self {
        Self {
            instructions: 1_000_000,
            nesting: 4096,
            vector_entries: 1_000_000,
        }
    }
}

/// A binary opcode from the core or `0xFC` instruction group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Opcode {
    /// An unprefixed core opcode.
    Core(u8),
    /// An extended opcode following prefix `0xFC`.
    Misc(u32),
}

/// The type immediate of a block, loop, or conditional.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockType {
    /// No result values.
    Empty,
    /// A single result type.
    Result(ValueType),
    /// An index into the module's type section.
    TypeIndex(u32),
}

/// A decoded immediate, with indexes and operand types still to validate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Immediate {
    /// This opcode has no immediate.
    None,
    /// A block signature.
    Block(BlockType),
    /// Single index in the space selected by the opcode.
    Index(u32),
    /// Two indexes in the order specified for the opcode.
    Indices(u32, u32),
    /// Branch labels and their fallback destination.
    BranchTable {
        /// Destinations indexed by the branch operand.
        targets: Vec<u32>,
        /// Destination for an operand outside the table.
        default: u32,
    },
    /// The type vector of a typed select.
    Select(Vec<ValueType>),
    /// Load or store addressing.
    Memory {
        /// Index of the memory, defaulting to zero when omitted.
        memory: u32,
        /// Alignment as a power-of-two exponent.
        alignment: u8,
        /// Unsigned static offset.
        offset: u64,
    },
    /// Signed 32-bit integer constant.
    I32(i32),
    /// Signed 64-bit integer constant.
    I64(i64),
    /// Bit pattern of a single-precision constant.
    F32(u32),
    /// Bit pattern of a double-precision constant.
    F64(u64),
    /// Abstract heap type code or type index of `ref.null`.
    HeapType(i64),
}

/// An instruction and its source byte offset. A decoder preserves the
/// ordering of control openers, `else` delimiters and `end` delimiters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instruction {
    /// Offset of the opcode's first byte in the module.
    pub offset: usize,
    /// Decoded opcode.
    pub opcode: Opcode,
    /// Decoded immediate fields.
    pub immediate: Immediate,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Block,
    If,
    Else,
}

/// Decode the function's instruction bytes and check structural control
/// delimiters. Covers core numeric, control, reference and memory operators,
/// plus saturating conversions and bulk memory/table operations. GC, SIMD,
/// threads and exception operators are rejected by this decoder. A syntax
/// result still needs index, stack and feature validation before execution.
pub fn decode_instructions(
    body: &FunctionBody<'_>,
    limits: InstructionLimits,
    cancellation: &CancellationToken,
) -> Result<Vec<Instruction>, BinaryError> {
    let mut source = Reader::new(body.instructions, body.offset);
    if body.offset.checked_add(body.instructions.len()).is_none() {
        return Err(source.error(Fault::LimitExceeded));
    }
    let mut controls = Vec::new();
    let mut result = Vec::new();
    let mut vector_entries = limits.vector_entries;
    loop {
        poll(&source, cancellation)?;
        if source.remaining().is_empty() {
            return Err(source.error(Fault::MissingEnd));
        }
        if result.len() >= limits.instructions {
            return Err(source.error(Fault::LimitExceeded));
        }
        let offset = source.position();
        let code = source.byte()?;
        let mut opcode = Opcode::Core(code);
        let mut end = false;
        let immediate = match code {
            0x02..=0x04 => {
                if controls.len() >= limits.nesting {
                    return Err(source.error(Fault::LimitExceeded));
                }
                let signature = block_type(&mut source)?;
                controls
                    .try_reserve(1)
                    .map_err(|_| source.error(Fault::LimitExceeded))?;
                controls.push(if code == 0x04 {
                    Control::If
                } else {
                    Control::Block
                });
                Immediate::Block(signature)
            }
            0x05 => match controls.last_mut() {
                Some(control @ Control::If) => {
                    *control = Control::Else;
                    Immediate::None
                }
                _ => {
                    return Err(BinaryError {
                        offset,
                        kind: Fault::InvalidControl,
                    });
                }
            },
            0x0b => {
                end = controls.pop().is_none();
                Immediate::None
            }
            0x0e => {
                let count = source.u32()? as usize;
                let mut targets = take_vector(&source, count, &mut vector_entries)?;
                for _ in 0..count {
                    poll(&source, cancellation)?;
                    targets.push(source.u32()?);
                }
                Immediate::BranchTable {
                    targets,
                    default: source.u32()?,
                }
            }
            0x1c => {
                let count = source.u32()? as usize;
                let mut types = take_vector(&source, count, &mut vector_entries)?;
                for _ in 0..count {
                    poll(&source, cancellation)?;
                    types.push(value_type(&mut source)?);
                }
                Immediate::Select(types)
            }
            0x0c
            | 0x0d
            | 0x10
            | 0x12
            | 0x14
            | 0x15
            | 0x20..=0x26
            | 0x3f
            | 0x40
            | 0xd2
            | 0xd5
            | 0xd6 => Immediate::Index(source.u32()?),
            0x11 | 0x13 => Immediate::Indices(source.u32()?, source.u32()?),
            0x28..=0x3e => memory_arg(&mut source)?,
            0x41 => Immediate::I32(source.s32()?),
            0x42 => Immediate::I64(source.s64()?),
            0x43 => Immediate::F32(u32::from_le_bytes(
                source.take(4)?.try_into().expect("four bytes"),
            )),
            0x44 => Immediate::F64(u64::from_le_bytes(
                source.take(8)?.try_into().expect("eight bytes"),
            )),
            0xd0 => Immediate::HeapType(heap_type(&mut source)?),
            0xfc => {
                let subcode = source.u32()?;
                opcode = Opcode::Misc(subcode);
                match subcode {
                    0..=7 => Immediate::None,
                    9 | 11 | 13 | 15..=17 => Immediate::Index(source.u32()?),
                    8 | 10 | 12 | 14 => Immediate::Indices(source.u32()?, source.u32()?),
                    _ => {
                        return Err(BinaryError {
                            offset,
                            kind: Fault::UnsupportedInstruction,
                        });
                    }
                }
            }
            0x00 | 0x01 | 0x0f | 0x1a | 0x1b | 0x45..=0xc4 | 0xd1 | 0xd3 | 0xd4 => Immediate::None,
            0x06..=0x0a | 0x18 | 0x19 | 0x1f | 0xfb | 0xfd | 0xfe => {
                return Err(BinaryError {
                    offset,
                    kind: Fault::UnsupportedInstruction,
                });
            }
            _ => {
                return Err(BinaryError {
                    offset,
                    kind: Fault::InvalidInstruction,
                });
            }
        };
        result
            .try_reserve(1)
            .map_err(|_| source.error(Fault::LimitExceeded))?;
        result.push(Instruction {
            offset,
            opcode,
            immediate,
        });
        if end {
            source.finish()?;
            return Ok(result);
        }
    }
}

fn take_vector<T>(
    source: &Reader<'_>,
    count: usize,
    remaining: &mut usize,
) -> Result<Vec<T>, BinaryError> {
    let result = reserve(source, count, *remaining)?;
    *remaining -= count;
    Ok(result)
}

fn block_type(source: &mut Reader<'_>) -> Result<BlockType, BinaryError> {
    match source.remaining().first().copied() {
        Some(0x40) => {
            source.byte()?;
            Ok(BlockType::Empty)
        }
        Some(code) if is_value_type(code) => Ok(BlockType::Result(value_type(source)?)),
        _ => {
            let error = source.error(Fault::InvalidType);
            let index = u32::try_from(source.s33()?).map_err(|_| error)?;
            Ok(BlockType::TypeIndex(index))
        }
    }
}

fn memory_arg(source: &mut Reader<'_>) -> Result<Immediate, BinaryError> {
    let error = source.error(Fault::InvalidInstruction);
    let flags = source.u32()?;
    if flags >= 128 {
        return Err(error);
    }
    let memory = if flags & 64 != 0 { source.u32()? } else { 0 };
    Ok(Immediate::Memory {
        memory,
        alignment: (flags & 63) as u8,
        offset: source.u64()?,
    })
}
