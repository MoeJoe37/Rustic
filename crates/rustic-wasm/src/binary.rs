use rustic_base::CancellationToken;

use crate::reader::{BinaryError, BinaryErrorKind as Fault, Reader};

/// Limits for the structural binary scan. The borrowed module bytes are
/// bounded separately from copied section, function and locals metadata.
#[derive(Clone, Copy, Debug)]
pub struct BinaryLimits {
    /// Maximum input length before reading a module.
    pub module_bytes: usize,
    /// Maximum section count, including custom sections.
    pub sections: usize,
    /// Maximum number of locally defined functions.
    pub functions: usize,
    /// Maximum sum of local declaration groups across all function bodies.
    pub local_groups: usize,
    /// Maximum number of decoded non-parameter locals per function.
    pub locals_per_function: u32,
}

impl Default for BinaryLimits {
    fn default() -> Self {
        Self {
            module_bytes: 32 * 1024 * 1024,
            sections: 4096,
            functions: 100_000,
            local_groups: 1_000_000,
            locals_per_function: 100_000,
        }
    }
}

/// Section order in a WebAssembly version 1 binary. Numeric section IDs
/// for tags and the data count differ from their position in that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionKind {
    /// Custom, advisory data.
    Custom,
    /// Types available to the module.
    Type,
    /// Definitions supplied by an embedder.
    Import,
    /// Type indexes of functions defined here.
    Function,
    /// Table definitions.
    Table,
    /// Linear memory definitions.
    Memory,
    /// Exception tags.
    Tag,
    /// Global definitions.
    Global,
    /// Public names and their indexes.
    Export,
    /// Optional start function.
    Start,
    /// Table segments.
    Element,
    /// Optional data segment count before code.
    DataCount,
    /// Locals and instructions for defined functions.
    Code,
    /// Linear memory segments.
    Data,
}

impl SectionKind {
    fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Custom,
            1 => Self::Type,
            2 => Self::Import,
            3 => Self::Function,
            4 => Self::Table,
            5 => Self::Memory,
            6 => Self::Global,
            7 => Self::Export,
            8 => Self::Start,
            9 => Self::Element,
            10 => Self::Code,
            11 => Self::Data,
            12 => Self::DataCount,
            13 => Self::Tag,
            _ => return None,
        })
    }
}

/// A bounded borrowed section span. The content bytes of a custom section
/// exclude its length-prefixed name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    /// Kind from the binary section identifier.
    pub kind: SectionKind,
    /// The decoded UTF-8 name of a custom section.
    pub name: Option<&'a str>,
    /// Absolute offset of the first content byte.
    pub offset: usize,
    /// Section content to be decoded by the corresponding reader.
    pub bytes: &'a [u8],
}

/// Binary local declaration type. Reference indexes and abstract type
/// codes are resolved by later validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    /// 32-bit integer.
    I32,
    /// 64-bit integer.
    I64,
    /// 32-bit floating point.
    F32,
    /// 64-bit floating point.
    F64,
    /// 128-bit SIMD value.
    V128,
    /// A typed reference with a signed 33-bit heap type encoding.
    Ref {
        /// Whether the reference allows null.
        nullable: bool,
        /// A non-negative type index or a negative abstract heap type code.
        heap_type: i64,
    },
}

/// A run of locals of the same type, preserving the original groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalGroup {
    /// Number of locals of this type.
    pub count: u32,
    /// Local binary value type.
    pub value_type: ValueType,
}

/// Function body bounded by its declared byte length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionBody<'a> {
    /// Type index from the matching entry in the function section.
    pub type_index: u32,
    /// Grouped local declarations, excluding parameters.
    pub locals: Vec<LocalGroup>,
    /// Absolute offset of the first instruction byte.
    pub offset: usize,
    /// Instruction bytes, including the last end byte. The instruction
    /// reader must still check opcode syntax and matching block ends.
    pub instructions: &'a [u8],
}

/// Result of scanning the envelope and selected framing information.
#[derive(Debug, PartialEq, Eq)]
pub struct BinaryModule<'a> {
    /// Sections in encounter order, including every custom section.
    pub sections: Vec<Section<'a>>,
    /// Locally defined functions, with the imported functions excluded.
    pub functions: Vec<FunctionBody<'a>>,
    /// Data count section value if supplied.
    pub data_count: Option<u32>,
}

/// Scan a version 1 binary under explicit limits with cooperative cancellation.
/// Check framing before handing section bytes to subsequent readers. The
/// result is suitable for syntax decoding, not for execution.
pub fn scan_module<'a>(
    bytes: &'a [u8],
    limits: BinaryLimits,
    cancellation: &CancellationToken,
) -> Result<BinaryModule<'a>, BinaryError> {
    let mut source = Reader::new(bytes, 0);
    poll(&source, cancellation)?;
    if bytes.len() > limits.module_bytes {
        return Err(source.error(Fault::LimitExceeded));
    }
    if source.take(4)? != b"\0asm" {
        return Err(BinaryError {
            offset: 0,
            kind: Fault::BadMagic,
        });
    }
    if source.take(4)? != [1, 0, 0, 0] {
        return Err(BinaryError {
            offset: 4,
            kind: Fault::UnsupportedVersion,
        });
    }

    let mut module = BinaryModule {
        sections: Vec::new(),
        functions: Vec::new(),
        data_count: None,
    };
    let mut type_indexes = Vec::new();
    let mut segment_count = 0;
    let mut previous = SectionKind::Custom;
    while !source.remaining().is_empty() {
        poll(&source, cancellation)?;
        if module.sections.len() >= limits.sections {
            return Err(source.error(Fault::LimitExceeded));
        }
        let at = source.position();
        let id = source.byte()?;
        let kind = SectionKind::from_id(id)
            .filter(|kind| *kind == SectionKind::Custom || *kind > previous)
            .ok_or(BinaryError {
                offset: at,
                kind: Fault::InvalidSection,
            })?;
        if kind != SectionKind::Custom {
            previous = kind;
        }
        let mut payload = source.sized_span()?;
        let name = if kind == SectionKind::Custom {
            Some(payload.name()?)
        } else {
            None
        };
        module
            .sections
            .try_reserve(1)
            .map_err(|_| source.error(Fault::LimitExceeded))?;
        module.sections.push(Section {
            kind,
            name,
            offset: payload.position(),
            bytes: payload.remaining(),
        });

        match kind {
            SectionKind::Function => {
                type_indexes = function_types(payload, limits.functions, cancellation)?
            }
            SectionKind::Code => {
                module.functions = function_bodies(payload, &type_indexes, limits, cancellation)?
            }
            SectionKind::DataCount => {
                module.data_count = Some(payload.u32()?);
                payload.finish()?;
            }
            SectionKind::Data => segment_count = payload.u32()?,
            _ => {}
        }
    }
    if module.functions.len() != type_indexes.len() {
        return Err(source.error(Fault::FunctionCountMismatch));
    }
    if module
        .data_count
        .is_some_and(|declared| declared != segment_count)
    {
        return Err(source.error(Fault::DataCountMismatch));
    }
    poll(&source, cancellation)?;
    Ok(module)
}

fn function_types(
    mut payload: Reader<'_>,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u32>, BinaryError> {
    let count = payload.u32()? as usize;
    let mut types = reserve(&payload, count, limit)?;
    for _ in 0..count {
        poll(&payload, cancellation)?;
        types.push(payload.u32()?);
    }
    payload.finish()?;
    Ok(types)
}

fn function_bodies<'a>(
    mut payload: Reader<'a>,
    types: &[u32],
    limits: BinaryLimits,
    cancellation: &CancellationToken,
) -> Result<Vec<FunctionBody<'a>>, BinaryError> {
    let at = payload.position();
    let count = payload.u32()? as usize;
    if count != types.len() {
        return Err(BinaryError {
            offset: at,
            kind: Fault::FunctionCountMismatch,
        });
    }
    let mut bodies = reserve(&payload, count, limits.functions)?;
    let mut group_limit = limits.local_groups;
    for &type_index in types {
        poll(&payload, cancellation)?;
        let mut body = payload.sized_span()?;
        let groups = body.u32()? as usize;
        let mut locals = reserve(&body, groups, group_limit)?;
        group_limit -= groups;
        let mut local_count = 0_u32;
        for _ in 0..groups {
            poll(&body, cancellation)?;
            let count = body.u32()?;
            local_count = local_count
                .checked_add(count)
                .filter(|total| *total <= limits.locals_per_function)
                .ok_or_else(|| body.error(Fault::LimitExceeded))?;
            locals.push(LocalGroup {
                count,
                value_type: value_type(&mut body)?,
            });
        }
        let instructions = body.remaining();
        if instructions.last() != Some(&0x0b) {
            return Err(BinaryError {
                offset: body.position() + instructions.len(),
                kind: Fault::MissingEnd,
            });
        }
        bodies.push(FunctionBody {
            type_index,
            locals,
            offset: body.position(),
            instructions,
        });
    }
    payload.finish()?;
    Ok(bodies)
}

pub(crate) fn is_value_type(code: u8) -> bool {
    matches!(code, 0x63 | 0x64 | 0x69..=0x74 | 0x7b..=0x7f)
}

pub(crate) fn value_type(body: &mut Reader<'_>) -> Result<ValueType, BinaryError> {
    let offset = body.position();
    Ok(match body.byte()? {
        0x7f => ValueType::I32,
        0x7e => ValueType::I64,
        0x7d => ValueType::F32,
        0x7c => ValueType::F64,
        0x7b => ValueType::V128,
        code @ 0x69..=0x74 => ValueType::Ref {
            nullable: true,
            heap_type: i64::from(code) - 128,
        },
        code @ (0x63 | 0x64) => ValueType::Ref {
            nullable: code == 0x63,
            heap_type: heap_type(body)?,
        },
        _ => {
            return Err(BinaryError {
                offset,
                kind: Fault::InvalidType,
            });
        }
    })
}

pub(crate) fn heap_type(body: &mut Reader<'_>) -> Result<i64, BinaryError> {
    if matches!(body.remaining().first(), Some(0x69..=0x74)) {
        return Ok(i64::from(body.byte()?) - 128);
    }
    let error = body.error(Fault::InvalidType);
    let index = body.s33()?;
    if index < 0 { Err(error) } else { Ok(index) }
}

pub(crate) fn reserve<T>(
    reader: &Reader<'_>,
    count: usize,
    limit: usize,
) -> Result<Vec<T>, BinaryError> {
    if count > limit {
        return Err(reader.error(Fault::LimitExceeded));
    }
    // Each declaration must use at least one byte in its own payload.
    if count > reader.remaining().len() {
        return Err(reader.error(Fault::UnexpectedEnd));
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| reader.error(Fault::LimitExceeded))?;
    Ok(result)
}

pub(crate) fn poll(reader: &Reader<'_>, token: &CancellationToken) -> Result<(), BinaryError> {
    token.check().map_err(|_| reader.error(Fault::Cancelled))
}
