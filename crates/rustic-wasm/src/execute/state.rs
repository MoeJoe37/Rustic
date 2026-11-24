use std::ops::Range;

use super::table::Tables;
use super::*;
use crate::validate::numeric::memory_access;
use crate::{Declarations, Immediate};

const PAGE: usize = 65_536;

#[derive(Default)]
pub(super) struct State<'bytes> {
    pub(super) globals: Vec<GlobalSlot>,
    pub(super) tables: Tables,
    memories: Vec<Memory>,
    data: Vec<Cow<'bytes, [u8]>>,
    memory_limit: usize,
    memory_bytes: usize,
}

pub(super) struct GlobalSlot {
    pub(super) ty: GlobalType,
    pub(super) value: Slot,
}

struct Memory {
    bytes: Vec<u8>,
    max_pages: usize,
}

pub(super) struct Transfer {
    pub(super) destination: u32,
    pub(super) source: u32,
    pub(super) size: u32,
}

impl<'bytes> State<'bytes> {
    pub(super) fn new<'input>(
        declarations: &Declarations<'input>,
        limits: InstanceLimits,
        budget: &mut Budget<'_>,
        mut retain_data: impl FnMut(
            &'input [u8],
            usize,
            &mut Budget<'_>,
        ) -> Result<Cow<'bytes, [u8]>, ExecutionError>,
    ) -> Result<Self, ExecutionError> {
        let mut state = Self {
            memory_limit: limits.memory_bytes,
            ..Self::default()
        };
        for entry in &declarations.globals {
            budget.spend(entry.offset, 1)?;
            let value = constant(
                &state.globals,
                &entry.value.initializer,
                limits.execution.values,
                budget,
            )?;
            append(
                &mut state.globals,
                GlobalSlot {
                    ty: entry.value.ty,
                    value,
                },
                entry.offset,
            )?;
        }
        state.tables = Tables::new(declarations, &state.globals, limits, budget)?;
        for entry in &declarations.memories {
            let offset = entry.offset;
            let len = (entry.value.min as usize)
                .checked_mul(PAGE)
                .filter(|&len| len <= state.memory_limit - state.memory_bytes)
                .ok_or_else(|| error(offset, Fault::LimitExceeded))?;
            budget.spend(offset, 1 + len)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(len)
                .map_err(|_| error(offset, Fault::LimitExceeded))?;
            bytes.resize(len, 0);
            let max_pages = entry.value.max.unwrap_or(65_536) as usize;
            append(&mut state.memories, Memory { bytes, max_pages }, offset)?;
            state.memory_bytes += len;
        }
        for entry in &declarations.data {
            budget.spend(entry.offset, 1)?;
            let bytes = entry.value.bytes;
            let passive = if let Some(active) = &entry.value.active {
                let address = constant(
                    &state.globals,
                    &active.offset,
                    limits.execution.values,
                    budget,
                )?
                .i32();
                let memory = &mut state.memories[active.target as usize].bytes;
                let range = checked_range(
                    memory.len(),
                    u64::from(address as u32),
                    bytes.len(),
                    entry.offset,
                    Fault::MemoryOutOfBounds,
                )?;
                budget.spend(entry.offset, bytes.len())?;
                memory[range].copy_from_slice(bytes);
                Cow::Borrowed(&[][..])
            } else {
                retain_data(bytes, entry.offset, budget)?
            };
            append(&mut state.data, passive, entry.offset)?;
        }
        Ok(state)
    }

    pub(super) fn size(&self, memory: u32) -> i32 {
        (self.memories[memory as usize].bytes.len() / PAGE) as i32
    }

    pub(super) fn grow(
        &mut self,
        memory: u32,
        pages: u32,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<i32, ExecutionError> {
        let memory = &mut self.memories[memory as usize];
        let old_len = memory.bytes.len();
        let old_pages = old_len / PAGE;
        let new_len = old_pages
            .checked_add(pages as usize)
            .filter(|&pages| pages <= memory.max_pages)
            .and_then(|pages| pages.checked_mul(PAGE))
            .filter(|&len| len - old_len <= self.memory_limit - self.memory_bytes);
        let Some(new_len) = new_len else {
            return Ok(-1);
        };
        let additional = new_len - old_len;
        let work = if new_len > memory.bytes.capacity() {
            new_len
        } else {
            additional
        };
        budget.spend(offset, work)?;
        if memory.bytes.try_reserve_exact(additional).is_err() {
            return Ok(-1);
        }
        memory.bytes.resize(new_len, 0);
        self.memory_bytes += additional;
        Ok(old_pages as i32)
    }

    pub(super) fn access(
        &mut self,
        op: u8,
        memory: u32,
        address: u64,
        store: Option<Value>,
        offset: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let bytes = &mut self.memories[memory as usize].bytes;
        let (_, alignment) = memory_access(op);
        let len = 1 << alignment;
        let range = checked_range(bytes.len(), address, len, offset, Fault::MemoryOutOfBounds)?;
        let mut buffer = [0u8; 8];
        if let Some(value) = store {
            let bits = match value {
                Value::I32(value) => value as u32 as u64,
                Value::I64(value) => value as u64,
                Value::F32(bits) => u64::from(bits),
                Value::F64(bits) => bits,
            };
            bytes[range].copy_from_slice(&bits.to_le_bytes()[..len]);
            return Ok(None);
        }
        buffer[..len].copy_from_slice(&bytes[range]);
        let bits = u64::from_le_bytes(buffer);
        let value = match op {
            0x28 | 0x2d | 0x2f => Value::I32(bits as i32),
            0x29 | 0x31 | 0x33 | 0x35 => Value::I64(bits as i64),
            0x2a => Value::F32(bits as u32),
            0x2b => Value::F64(bits),
            0x2c => Value::I32(i32::from(bits as i8)),
            0x2e => Value::I32(i32::from(bits as i16)),
            0x30 => Value::I64(i64::from(bits as i8)),
            0x32 => Value::I64(i64::from(bits as i16)),
            0x34 => Value::I64(i64::from(bits as i32)),
            _ => unreachable!("validated memory load"),
        };
        Ok(Some(value))
    }

    pub(super) fn init(
        &mut self,
        data: u32,
        memory: u32,
        transfer: Transfer,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), ExecutionError> {
        let source = self.data[data as usize].as_ref();
        let destination = &mut self.memories[memory as usize].bytes;
        let (src, dst) = transfer.ranges(
            source.len(),
            destination.len(),
            offset,
            Fault::MemoryOutOfBounds,
        )?;
        budget.spend(offset, transfer.size as usize)?;
        destination[dst].copy_from_slice(&source[src]);
        Ok(())
    }

    pub(super) fn drop_data(&mut self, data: u32) {
        self.data[data as usize] = Cow::Borrowed(&[]);
    }

    pub(super) fn copy(
        &mut self,
        destination: u32,
        source: u32,
        transfer: Transfer,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), ExecutionError> {
        let (destination, source) = (destination as usize, source as usize);
        let (src, dst) = transfer.ranges(
            self.memories[source].bytes.len(),
            self.memories[destination].bytes.len(),
            offset,
            Fault::MemoryOutOfBounds,
        )?;
        budget.spend(offset, transfer.size as usize)?;
        if destination == source {
            self.memories[destination].bytes.copy_within(src, dst.start);
        } else {
            let (before, after) = self.memories.split_at_mut(destination.max(source));
            let (target, origin) = if destination < source {
                (&mut before[destination], &after[0])
            } else {
                (&mut after[0], &before[source])
            };
            target.bytes[dst].copy_from_slice(&origin.bytes[src]);
        }
        Ok(())
    }

    pub(super) fn fill(
        &mut self,
        memory: u32,
        transfer: Transfer,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), ExecutionError> {
        let bytes = &mut self.memories[memory as usize].bytes;
        let range = checked_range(
            bytes.len(),
            u64::from(transfer.destination),
            transfer.size as usize,
            offset,
            Fault::MemoryOutOfBounds,
        )?;
        budget.spend(offset, transfer.size as usize)?;
        bytes[range].fill(transfer.source as u8);
        Ok(())
    }
}

pub(super) fn constant(
    globals: &[GlobalSlot],
    instructions: &[Instruction],
    stack_limit: usize,
    budget: &mut Budget<'_>,
) -> Result<Slot, ExecutionError> {
    let mut values = Vec::new();
    for instruction in instructions {
        let offset = instruction.offset;
        budget.spend(offset, 1)?;
        let value = match (instruction.opcode, &instruction.immediate) {
            (Opcode::Core(0x0b), _) => break,
            (Opcode::Core(0x23), Immediate::Index(index)) => globals[*index as usize].value,
            (Opcode::Core(0xd0), Immediate::HeapType(ty)) => Slot::Ref(Reference::null(*ty)),
            (Opcode::Core(0xd2), Immediate::Index(index)) => {
                Slot::Ref(Reference::Function(Some(*index)))
            }
            (op, immediate) => Slot::Number(
                numeric::apply(op, immediate, &mut values).map_err(|kind| error(offset, kind))?,
            ),
        };
        if values.len() >= stack_limit {
            return Err(error(offset, Fault::LimitExceeded));
        }
        append(&mut values, value, offset)?;
    }
    Ok(values.pop().expect("validated constant expression"))
}

impl Transfer {
    pub(super) fn ranges(
        &self,
        source_len: usize,
        destination_len: usize,
        offset: usize,
        kind: Fault,
    ) -> Result<(Range<usize>, Range<usize>), ExecutionError> {
        Ok((
            checked_range(
                source_len,
                u64::from(self.source),
                self.size as usize,
                offset,
                kind,
            )?,
            checked_range(
                destination_len,
                u64::from(self.destination),
                self.size as usize,
                offset,
                kind,
            )?,
        ))
    }
}

pub(super) fn checked_range(
    len: usize,
    address: u64,
    bytes: usize,
    offset: usize,
    kind: Fault,
) -> Result<Range<usize>, ExecutionError> {
    let start = usize::try_from(address).map_err(|_| error(offset, kind))?;
    let end = start
        .checked_add(bytes)
        .filter(|&end| end <= len)
        .ok_or_else(|| error(offset, kind))?;
    Ok(start..end)
}
