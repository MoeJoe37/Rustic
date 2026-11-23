use rustic_base::CancellationToken;

use crate::binary::{poll, reserve, value_type};
use crate::instructions::expression;
use crate::module::*;
use crate::reader::{BinaryErrorKind as Fault, Reader};
use crate::{BinaryError, BinaryLimits, InstructionLimits, SectionKind, ValueType, scan_module};

/// Admission limits for scanning and decoding a complete module.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    /// Framing, functions, and local declarations.
    pub binary: BinaryLimits,
    /// Instructions and operand entries across the whole module. The nesting
    /// limit is applied independently to each expression.
    pub code: InstructionLimits,
    /// Total vector entries in module declarations, including nested vectors.
    pub declaration_entries: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            binary: BinaryLimits::default(),
            code: InstructionLimits::default(),
            declaration_entries: 1_000_000,
        }
    }
}

/// Decode version 1 framing, declarations and instruction sequences.
/// Function signatures use the compact function type encoding. Recursive
/// type groups, GC shapes, shared limits and additional opcodes return
/// unsupported-feature errors where encountered.
pub fn decode_module<'a>(
    bytes: &'a [u8],
    limits: DecodeLimits,
    cancellation: &CancellationToken,
) -> Result<DecodedModule<'a>, BinaryError> {
    let mut binary = scan_module(bytes, limits.binary, cancellation)?;
    let mut decoder = Decoder {
        cancellation,
        remaining_entries: limits.declaration_entries,
        code: limits.code,
    };
    let mut declarations = Declarations {
        data_count: binary.data_count,
        ..Declarations::default()
    };
    let mut functions = Vec::new();
    for section in &binary.sections {
        let mut reader = Reader::new(section.bytes, section.offset);
        poll(&reader, cancellation)?;
        match section.kind {
            SectionKind::Type => {
                declarations.types = decoder.declarations(&mut reader, function_type)?
            }
            SectionKind::Import => {
                declarations.imports = decoder.declarations(&mut reader, import)?
            }
            SectionKind::Table => declarations.tables = decoder.declarations(&mut reader, table)?,
            SectionKind::Memory => {
                declarations.memories =
                    decoder.declarations(&mut reader, |_, r| allocation_limits(r))?
            }
            SectionKind::Global => {
                declarations.globals = decoder.declarations(&mut reader, global)?
            }
            SectionKind::Tag => {
                declarations.tags = decoder.declarations(&mut reader, |_, r| tag_type(r))?
            }
            SectionKind::Export => {
                declarations.exports = decoder.declarations(&mut reader, export)?
            }
            SectionKind::Start => {
                declarations.start = Some(Declaration {
                    offset: reader.position(),
                    value: reader.u32()?,
                })
            }
            SectionKind::Element => {
                declarations.elements = decoder.declarations(&mut reader, element)?
            }
            SectionKind::Data => declarations.data = decoder.declarations(&mut reader, data)?,
            SectionKind::Code => {
                functions
                    .try_reserve_exact(binary.functions.len())
                    .map_err(|_| reader.error(Fault::LimitExceeded))?;
                for body in binary.functions.drain(..) {
                    let mut source = Reader::new(body.instructions, body.offset);
                    let instructions = expression(&mut source, &mut decoder.code, cancellation)?;
                    source.finish()?;
                    functions.push(Declaration {
                        offset: body.offset,
                        value: Function {
                            type_index: body.type_index,
                            locals: body.locals,
                            instructions,
                        },
                    });
                }
                continue;
            }
            SectionKind::Custom | SectionKind::Function | SectionKind::DataCount => continue,
        }
        reader.finish()?;
    }
    binary
        .sections
        .retain(|section| section.kind == SectionKind::Custom);
    Ok(DecodedModule {
        declarations,
        functions,
        custom_sections: binary.sections,
    })
}

struct Decoder<'a> {
    cancellation: &'a CancellationToken,
    remaining_entries: usize,
    code: InstructionLimits,
}

impl Decoder<'_> {
    fn vector<'a, T>(
        &mut self,
        reader: &mut Reader<'a>,
        mut read_item: impl FnMut(&mut Self, &mut Reader<'a>) -> Result<T, BinaryError>,
    ) -> Result<Vec<T>, BinaryError> {
        let count = reader.u32()? as usize;
        let mut items = reserve(reader, count, self.remaining_entries)?;
        self.remaining_entries -= count;
        for _ in 0..count {
            poll(reader, self.cancellation)?;
            items.push(read_item(self, reader)?);
        }
        Ok(items)
    }

    fn declarations<'a, T>(
        &mut self,
        reader: &mut Reader<'a>,
        read_item: fn(&mut Self, &mut Reader<'a>) -> Result<T, BinaryError>,
    ) -> Result<Vec<Declaration<T>>, BinaryError> {
        self.vector(reader, |decoder, reader| {
            let offset = reader.position();
            Ok(Declaration {
                offset,
                value: read_item(decoder, reader)?,
            })
        })
    }

    fn instructions(
        &mut self,
        reader: &mut Reader<'_>,
    ) -> Result<Vec<crate::Instruction>, BinaryError> {
        expression(reader, &mut self.code, self.cancellation)
    }

    fn activate(
        &mut self,
        reader: &mut Reader<'_>,
        explicit: bool,
    ) -> Result<ActiveSegment, BinaryError> {
        let target = if explicit { reader.u32()? } else { 0 };
        Ok(ActiveSegment {
            target,
            offset: self.instructions(reader)?,
        })
    }
}

fn function_type(
    decoder: &mut Decoder<'_>,
    reader: &mut Reader<'_>,
) -> Result<FunctionType, BinaryError> {
    let offset = reader.position();
    match reader.byte()? {
        0x60 => Ok(FunctionType {
            params: decoder.vector(reader, |_, r| value_type(r))?,
            results: decoder.vector(reader, |_, r| value_type(r))?,
        }),
        0x4e..=0x50 | 0x5e | 0x5f => Err(BinaryError {
            offset,
            kind: Fault::UnsupportedType,
        }),
        _ => Err(BinaryError {
            offset,
            kind: Fault::InvalidType,
        }),
    }
}

fn allocation_limits(reader: &mut Reader<'_>) -> Result<Limits, BinaryError> {
    let offset = reader.position();
    let flags = reader.byte()?;
    let address_type = match flags {
        0 | 1 => AddressType::I32,
        4 | 5 => AddressType::I64,
        2 | 3 | 6 | 7 => {
            return Err(BinaryError {
                offset,
                kind: Fault::UnsupportedType,
            });
        }
        _ => {
            return Err(BinaryError {
                offset,
                kind: Fault::InvalidType,
            });
        }
    };
    let min = reader.u64()?;
    let max = if flags & 1 != 0 {
        Some(reader.u64()?)
    } else {
        None
    };
    Ok(Limits {
        address_type,
        min,
        max,
    })
}

fn reference_type(reader: &mut Reader<'_>) -> Result<ValueType, BinaryError> {
    let error = reader.error(Fault::InvalidType);
    let ty = value_type(reader)?;
    if matches!(ty, ValueType::Ref { .. }) {
        Ok(ty)
    } else {
        Err(error)
    }
}

fn table_type(reader: &mut Reader<'_>) -> Result<TableType, BinaryError> {
    Ok(TableType {
        element_type: reference_type(reader)?,
        limits: allocation_limits(reader)?,
    })
}

fn global_type(reader: &mut Reader<'_>) -> Result<GlobalType, BinaryError> {
    let value_type = value_type(reader)?;
    let error = reader.error(Fault::InvalidType);
    let mutable = match reader.byte()? {
        0 => false,
        1 => true,
        _ => return Err(error),
    };
    Ok(GlobalType {
        value_type,
        mutable,
    })
}

fn tag_type(reader: &mut Reader<'_>) -> Result<u32, BinaryError> {
    require_byte(reader, 0, Fault::InvalidType)?;
    reader.u32()
}

fn require_byte(reader: &mut Reader<'_>, byte: u8, fault: Fault) -> Result<(), BinaryError> {
    let error = reader.error(fault);
    if reader.byte()? == byte {
        Ok(())
    } else {
        Err(error)
    }
}

fn import<'a>(_: &mut Decoder<'_>, reader: &mut Reader<'a>) -> Result<Import<'a>, BinaryError> {
    let module = reader.name()?;
    let name = reader.name()?;
    let error = reader.error(Fault::InvalidDescriptor);
    let ty = match reader.byte()? {
        0 => ImportType::Function(reader.u32()?),
        1 => ImportType::Table(table_type(reader)?),
        2 => ImportType::Memory(allocation_limits(reader)?),
        3 => ImportType::Global(global_type(reader)?),
        4 => ImportType::Tag(tag_type(reader)?),
        _ => return Err(error),
    };
    Ok(Import { module, name, ty })
}

fn export<'a>(_: &mut Decoder<'_>, reader: &mut Reader<'a>) -> Result<Export<'a>, BinaryError> {
    let name = reader.name()?;
    let error = reader.error(Fault::InvalidDescriptor);
    let constructor = match reader.byte()? {
        0 => ExportIndex::Function,
        1 => ExportIndex::Table,
        2 => ExportIndex::Memory,
        3 => ExportIndex::Global,
        4 => ExportIndex::Tag,
        _ => return Err(error),
    };
    Ok(Export {
        name,
        target: constructor(reader.u32()?),
    })
}

fn table(decoder: &mut Decoder<'_>, reader: &mut Reader<'_>) -> Result<Table, BinaryError> {
    let explicit = reader.remaining().first() == Some(&0x40);
    if explicit {
        reader.byte()?;
        require_byte(reader, 0, Fault::InvalidType)?;
    }
    let ty = table_type(reader)?;
    let initializer = explicit.then(|| decoder.instructions(reader)).transpose()?;
    Ok(Table { ty, initializer })
}

fn global(decoder: &mut Decoder<'_>, reader: &mut Reader<'_>) -> Result<Global, BinaryError> {
    Ok(Global {
        ty: global_type(reader)?,
        initializer: decoder.instructions(reader)?,
    })
}

fn element(
    decoder: &mut Decoder<'_>,
    reader: &mut Reader<'_>,
) -> Result<ElementSegment, BinaryError> {
    let error = reader.error(Fault::InvalidSegment);
    let flags = reader.u32()?;
    if flags > 7 {
        return Err(error);
    }
    let mode = match flags & 3 {
        0 | 2 => ElementMode::Active(decoder.activate(reader, flags & 2 != 0)?),
        1 => ElementMode::Passive,
        _ => ElementMode::Declarative,
    };
    let (element_type, items) = if flags & 4 == 0 {
        if flags != 0 {
            require_byte(reader, 0, Fault::InvalidSegment)?;
        }
        let ty = ValueType::Ref {
            nullable: false,
            heap_type: -16,
        };
        (
            ty,
            ElementItems::Functions(decoder.vector(reader, |_, r| r.u32())?),
        )
    } else {
        let ty = if flags == 4 {
            ValueType::Ref {
                nullable: true,
                heap_type: -16,
            }
        } else {
            reference_type(reader)?
        };
        (
            ty,
            ElementItems::Expressions(decoder.vector(reader, Decoder::instructions)?),
        )
    };
    Ok(ElementSegment {
        mode,
        element_type,
        items,
    })
}

fn data<'a>(
    decoder: &mut Decoder<'_>,
    reader: &mut Reader<'a>,
) -> Result<DataSegment<'a>, BinaryError> {
    let error = reader.error(Fault::InvalidSegment);
    let active = match reader.u32()? {
        0 => Some(decoder.activate(reader, false)?),
        1 => None,
        2 => Some(decoder.activate(reader, true)?),
        _ => return Err(error),
    };
    let bytes = reader.sized_span()?.remaining();
    Ok(DataSegment { active, bytes })
}
