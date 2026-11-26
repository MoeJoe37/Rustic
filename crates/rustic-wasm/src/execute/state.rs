use std::{cell::Cell, ops::Range, rc::Rc};

use super::memory::MemoryPool;
use super::table::{ActiveElements, Tables};
use super::*;
use crate::validate::numeric::memory_access;
use crate::{Declarations, Immediate, ImportType};

#[derive(Default)]
pub(super) struct State<'bytes> {
    pub(super) globals: Vec<GlobalSlot>,
    pub(super) tables: Tables,
    pub(super) has_host_references: Cell<bool>,
    pub(super) function_owner: Option<NonZeroU32>,
    pub(super) memories: Vec<LinearMemory>,
    data: Vec<Cow<'bytes, [u8]>>,
}

/// A thread-confined numeric global shared with a host or other instances.
#[derive(Clone, Debug)]
pub struct NumericGlobal(Rc<GlobalCell>);

#[derive(Debug)]
struct GlobalCell {
    value: Cell<Value>,
    mutable: bool,
}

impl NumericGlobal {
    /// Allocate an independent global with the type of its initial value.
    pub fn new(value: Value, mutable: bool) -> Self {
        Self(Rc::new(GlobalCell {
            value: Cell::new(value),
            mutable,
        }))
    }

    /// Heap bytes retained by one shared allocation, including its counts.
    pub const fn storage_bytes() -> usize {
        size_of::<GlobalCell>() + 2 * size_of::<usize>()
    }

    /// Read the numeric type and mutability.
    pub fn ty(&self) -> GlobalType {
        GlobalType {
            value_type: self.get().value_type(),
            mutable: self.0.mutable,
        }
    }

    /// Read the current value.
    pub fn get(&self) -> Value {
        self.0.value.get()
    }

    /// Replace the value if the cell is mutable and the numeric type matches.
    pub fn set(&self, value: Value) -> bool {
        let ty = self.ty();
        if !ty.mutable || ty.value_type != value.value_type() {
            return false;
        }
        self.0.value.set(value);
        true
    }
}

pub(super) enum GlobalSlot {
    Local { ty: GlobalType, value: Slot },
    Shared(NumericGlobal),
}

impl GlobalSlot {
    pub(super) fn ty(&self) -> GlobalType {
        match self {
            Self::Local { ty, .. } => *ty,
            Self::Shared(global) => global.ty(),
        }
    }

    pub(super) fn value(&self) -> Slot {
        match self {
            Self::Local { value, .. } => *value,
            Self::Shared(global) => Slot::Number(global.get()),
        }
    }

    pub(super) fn write_guest(&mut self, value: Slot) {
        match self {
            Self::Local { value: slot, .. } => *slot = value,
            Self::Shared(global) => {
                let updated = global.set(value.number());
                debug_assert!(updated, "validated numeric global write");
            }
        }
    }

    pub(super) fn share(&mut self) -> Option<NumericGlobal> {
        match self {
            Self::Shared(global) => Some(global.clone()),
            Self::Local {
                ty,
                value: Slot::Number(value),
            } => {
                let global = NumericGlobal::new(*value, ty.mutable);
                *self = Self::Shared(global.clone());
                Some(global)
            }
            Self::Local {
                value: Slot::Ref(_),
                ..
            } => None,
        }
    }
}

pub(super) struct Transfer {
    pub(super) destination: u64,
    pub(super) source: u64,
    pub(super) size: u64,
}

type ActiveData<'input> = (u32, Range<usize>, &'input [u8]);

pub(super) struct Initializers<'input> {
    elements: ActiveElements,
    data: Vec<ActiveData<'input>>,
    data_trap: Option<ExecutionError>,
}

impl Initializers<'_> {
    pub(super) fn apply(
        self,
        state: &State<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ExecutionError> {
        if cancellation.is_cancelled() {
            return Err(error(0, Fault::Cancelled));
        }
        self.elements.apply(&state.tables)?;
        for (target, range, bytes) in self.data {
            state.memories[target as usize].write(|memory| memory[range].copy_from_slice(bytes));
        }
        self.data_trap.map_or(Ok(()), Err)
    }
}

impl<'bytes> State<'bytes> {
    pub(super) fn new<'input>(
        declarations: &Declarations<'input>,
        imports: InstanceImports<'_>,
        limits: InstanceLimits,
        budget: &mut Budget<'_>,
        mut retain_data: impl FnMut(
            &'input [u8],
            usize,
            &mut Budget<'_>,
        ) -> Result<Cow<'bytes, [u8]>, ExecutionError>,
    ) -> Result<(Self, Initializers<'input>), ExecutionError> {
        let InstanceImports {
            mut globals,
            memories: mut imported_memories,
            tables,
            function_owner,
        } = imports;
        let mut state = Self {
            function_owner,
            ..Default::default()
        };
        for entry in &declarations.imports {
            let ImportType::Global(ty) = entry.value.ty else {
                continue;
            };
            budget.spend(entry.offset, 1)?;
            let Some((global, remaining)) = globals.split_first() else {
                return Err(error(entry.offset, Fault::MissingImport));
            };
            if ty != global.ty() {
                return Err(error(entry.offset, Fault::ImportTypeMismatch));
            }
            append(
                &mut state.globals,
                GlobalSlot::Shared(global.clone()),
                entry.offset,
            )?;
            globals = remaining;
        }
        if !globals.is_empty() {
            return Err(error(0, Fault::ImportTypeMismatch));
        }
        for entry in &declarations.globals {
            budget.spend(entry.offset, 1)?;
            let value = constant(
                &state.globals,
                function_owner,
                &entry.value.initializer,
                limits.execution.values,
                budget,
            )?;
            append(
                &mut state.globals,
                GlobalSlot::Local {
                    ty: entry.value.ty,
                    value,
                },
                entry.offset,
            )?;
        }
        for entry in &declarations.imports {
            let ImportType::Memory(ty) = entry.value.ty else {
                continue;
            };
            budget.spend(entry.offset, 1)?;
            let Some((memory, remaining)) = imported_memories.split_first() else {
                return Err(error(entry.offset, Fault::MissingImport));
            };
            if !memory.matches(ty) {
                return Err(error(entry.offset, Fault::ImportTypeMismatch));
            }
            append(&mut state.memories, memory.clone(), entry.offset)?;
            imported_memories = remaining;
        }
        if !imported_memories.is_empty() {
            return Err(error(0, Fault::ImportTypeMismatch));
        }
        let pool = MemoryPool::new(limits.memory_bytes);
        for entry in &declarations.memories {
            let memory = LinearMemory::local(entry.value, &pool, entry.offset, budget)?;
            append(&mut state.memories, memory, entry.offset)?;
        }
        let (allocated_tables, active_elements) = Tables::new(
            declarations,
            &state.globals,
            tables,
            function_owner,
            limits,
            budget,
        )?;
        state.tables = allocated_tables;
        state.has_host_references.set(!tables.is_empty());
        let mut active_segments = Vec::new();
        let mut segment_trap = None;
        // A trapping element segment stops instantiation before data writes.
        for entry in declarations
            .data
            .iter()
            .filter(|_| active_elements.trap.is_none())
        {
            budget.spend(entry.offset, 1)?;
            let bytes = entry.value.bytes;
            let passive = if let Some(active) = &entry.value.active {
                let address = constant(
                    &state.globals,
                    function_owner,
                    &active.offset,
                    limits.execution.values,
                    budget,
                )?
                .address();
                let memory = &state.memories[active.target as usize];
                let range = match checked_range(
                    memory.len(),
                    address,
                    bytes.len() as u64,
                    entry.offset,
                    Fault::MemoryOutOfBounds,
                ) {
                    Ok(range) => range,
                    Err(trap) => {
                        segment_trap = Some(trap);
                        break;
                    }
                };
                budget.spend(entry.offset, bytes.len())?;
                append(
                    &mut active_segments,
                    (active.target, range, bytes),
                    entry.offset,
                )?;
                Cow::Borrowed(&[][..])
            } else {
                retain_data(bytes, entry.offset, budget)?
            };
            append(&mut state.data, passive, entry.offset)?;
        }
        budget.spend(0, 0)?;
        Ok((
            state,
            Initializers {
                elements: active_elements,
                data: active_segments,
                data_trap: segment_trap,
            },
        ))
    }

    pub(super) fn size(&self, memory: u32) -> Value {
        let memory = &self.memories[memory as usize];
        memory.page_result(Some(memory.pages()))
    }

    pub(super) fn grow(
        &mut self,
        memory: u32,
        pages: u64,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Value, ExecutionError> {
        let memory = &self.memories[memory as usize];
        let old = memory.grow(pages, |work| budget.spend(offset, work))?;
        Ok(memory.page_result(old))
    }

    pub(super) fn access(
        &mut self,
        op: u8,
        memory: u32,
        address: u64,
        store: Option<Value>,
        offset: usize,
    ) -> Result<Option<Value>, ExecutionError> {
        let memory = &self.memories[memory as usize];
        let (_, alignment) = memory_access(op);
        let len = 1 << alignment;
        let range = checked_range(
            memory.len(),
            address,
            len as u64,
            offset,
            Fault::MemoryOutOfBounds,
        )?;
        let mut buffer = [0u8; 8];
        if let Some(value) = store {
            let bits = match value {
                Value::I32(value) => value as u32 as u64,
                Value::I64(value) => value as u64,
                Value::F32(bits) => u64::from(bits),
                Value::F64(bits) => bits,
            };
            memory.write(|bytes| bytes[range].copy_from_slice(&bits.to_le_bytes()[..len]));
            return Ok(None);
        }
        memory.read(|bytes| buffer[..len].copy_from_slice(&bytes[range]));
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
        let destination = &self.memories[memory as usize];
        let (src, dst) = transfer.ranges(
            source.len(),
            destination.len(),
            offset,
            Fault::MemoryOutOfBounds,
        )?;
        budget.spend(offset, dst.len())?;
        destination.write(|bytes| bytes[dst].copy_from_slice(&source[src]));
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
        let destination = &self.memories[destination as usize];
        let source = &self.memories[source as usize];
        let (src, dst) = transfer.ranges(
            source.len(),
            destination.len(),
            offset,
            Fault::MemoryOutOfBounds,
        )?;
        budget.spend(offset, dst.len())?;
        if destination.aliases(source) {
            destination.write(|bytes| bytes.copy_within(src, dst.start));
        } else {
            source.read(|input| {
                destination.write(|output| output[dst].copy_from_slice(&input[src]));
            });
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
        let memory = &self.memories[memory as usize];
        let range = checked_range(
            memory.len(),
            transfer.destination,
            transfer.size,
            offset,
            Fault::MemoryOutOfBounds,
        )?;
        budget.spend(offset, range.len())?;
        memory.write(|bytes| bytes[range].fill(transfer.source as u8));
        Ok(())
    }
}

pub(super) fn constant(
    globals: &[GlobalSlot],
    function_owner: Option<NonZeroU32>,
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
            (Opcode::Core(0x23), Immediate::Index(index)) => globals[*index as usize].value(),
            (Opcode::Core(0xd0), Immediate::HeapType(ty)) => Slot::Ref(Reference::null(*ty)),
            (Opcode::Core(0xd2), Immediate::Index(index)) => {
                Slot::Ref(FunctionAddress::reference(function_owner, *index))
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
            checked_range(source_len, self.source, self.size, offset, kind)?,
            checked_range(destination_len, self.destination, self.size, offset, kind)?,
        ))
    }
}

pub(super) fn checked_range(
    len: usize,
    address: u64,
    bytes: u64,
    offset: usize,
    kind: Fault,
) -> Result<Range<usize>, ExecutionError> {
    let end = address
        .checked_add(bytes)
        .filter(|&end| end <= len as u64)
        .ok_or_else(|| error(offset, kind))?;
    Ok(address as usize..end as usize)
}
