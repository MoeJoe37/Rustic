use std::{collections::HashMap, fmt};

use rustic_base::CancellationToken;

use crate::validate::numeric::effect;
use crate::{
    BlockType, ExportIndex, Function, FunctionType, Instruction, Opcode, ValidatedModule, ValueType,
};

mod machine;
mod numeric;

/// A WebAssembly number crossing the interpreter boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    /// A 32-bit integer.
    I32(i32),
    /// A 64-bit integer.
    I64(i64),
    /// A single-precision float stored as its original bits.
    F32(u32),
    /// A double-precision float stored as its original bits.
    F64(u64),
}

impl Value {
    /// The corresponding WebAssembly type.
    pub fn value_type(self) -> ValueType {
        match self {
            Self::I32(_) => ValueType::I32,
            Self::I64(_) => ValueType::I64,
            Self::F32(_) => ValueType::F32,
            Self::F64(_) => ValueType::F64,
        }
    }

    fn zero(ty: ValueType) -> Self {
        match ty {
            ValueType::I32 => Self::I32(0),
            ValueType::I64 => Self::I64(0),
            ValueType::F32 => Self::F32(0),
            ValueType::F64 => Self::F64(0),
            _ => unreachable!("preparation checked numeric locals"),
        }
    }
}

/// Why the numeric interpreter rejected a module, invocation, or instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionErrorKind {
    /// The module declares state, imports, or types outside the numeric profile.
    UnsupportedModule,
    /// An instruction needs support outside the numeric profile.
    UnsupportedInstruction,
    /// The specified function export does not exist.
    UnknownExport,
    /// The supplied arguments do not match the function signature.
    ArgumentType,
    /// The host cancelled preparation or execution.
    Cancelled,
    /// A work or stack budget was exhausted.
    LimitExceeded,
    /// Execution reached an explicit unreachable instruction.
    Unreachable,
    /// Integer division or remainder used a zero divisor.
    IntegerDivideByZero,
    /// An integer division or conversion was outside the destination range.
    IntegerOverflow,
    /// A trapping float-to-integer conversion received a NaN.
    InvalidConversionToInteger,
}

use ExecutionErrorKind as Fault;

/// A preparation error, invalid invocation, or WebAssembly trap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionError {
    /// The byte offset of the declaration or instruction, or zero at host entry.
    pub offset: usize,
    /// Why the operation failed.
    pub kind: ExecutionErrorKind,
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, writer: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            writer,
            "WebAssembly execution {:?} at byte {}",
            self.kind, self.offset
        )
    }
}

impl std::error::Error for ExecutionError {}

/// Resource limits for preparing and invoking a numeric module.
#[derive(Clone, Copy, Debug)]
pub struct ExecutionLimits {
    /// Work to prepare the module, including instruction and declaration visits.
    pub preparation_work: usize,
    /// Work per invocation, including instructions and bulk stack moves.
    pub invocation_work: usize,
    /// Combined number of live locals and operands.
    pub values: usize,
    /// Maximum simultaneously active function calls.
    pub calls: usize,
    /// Maximum total depth of all active control frames.
    pub control_frames: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            preparation_work: 8_000_000,
            invocation_work: 1_000_000,
            values: 100_000,
            calls: 1024,
            control_frames: 4096,
        }
    }
}

/// An executable instance of a validated numeric module.
///
/// The supported profile covers all scalar numeric instructions, local
/// variables, multi-value control flow, and direct and direct tail calls.
/// Modules with imports, memories, tables, globals, tags, segments, or
/// non-numeric storage types require a wider interpreter profile.
pub struct NumericInstance<'module, 'bytes> {
    module: &'module ValidatedModule<'bytes>,
    functions: Vec<PreparedFunction<'module>>,
    exports: HashMap<&'bytes str, usize>,
    limits: ExecutionLimits,
}

impl<'module, 'bytes> NumericInstance<'module, 'bytes> {
    /// Prepare the control targets and run the optional start function once.
    pub fn instantiate(
        module: &'module ValidatedModule<'bytes>,
        limits: ExecutionLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, ExecutionError> {
        let decoded = module.module();
        let declarations = decoded.declarations();
        let mut budget = Budget {
            remaining: limits.preparation_work,
            cancellation,
        };
        budget.spend(0, 0)?;
        let unsupported = [
            declarations.imports.first().map(|entry| entry.offset),
            declarations.tables.first().map(|entry| entry.offset),
            declarations.memories.first().map(|entry| entry.offset),
            declarations.globals.first().map(|entry| entry.offset),
            declarations.tags.first().map(|entry| entry.offset),
            declarations.elements.first().map(|entry| entry.offset),
            declarations.data.first().map(|entry| entry.offset),
        ];
        if let Some(offset) = unsupported.into_iter().flatten().min() {
            return Err(error(offset, Fault::UnsupportedModule));
        }
        for entry in &declarations.types {
            let types = entry.value.params.iter().chain(&entry.value.results);
            budget.spend(
                entry.offset,
                1 + entry.value.params.len() + entry.value.results.len(),
            )?;
            if !types.copied().all(is_numeric) {
                return Err(error(entry.offset, Fault::UnsupportedModule));
            }
        }
        let mut functions = Vec::new();
        for entry in decoded.functions() {
            let signature = &declarations.types[entry.value.type_index as usize].value;
            let prepared =
                PreparedFunction::new(&entry.value, signature, entry.offset, &mut budget)?;
            append(&mut functions, prepared, entry.offset)?;
        }
        let mut exports = HashMap::new();
        for entry in &declarations.exports {
            budget.spend(entry.offset, 1 + entry.value.name.len())?;
            let ExportIndex::Function(index) = entry.value.target else {
                unreachable!("validated module has only function exports in this profile");
            };
            exports
                .try_reserve(1)
                .map_err(|_| error(entry.offset, Fault::LimitExceeded))?;
            exports.insert(entry.value.name, index as usize);
        }
        let instance = Self {
            module,
            functions,
            exports,
            limits,
        };
        if let Some(start) = &declarations.start {
            machine::invoke(&instance, start.value as usize, &[], 0, cancellation)?;
        }
        Ok(instance)
    }

    /// Invoke an exported function with a fresh execution budget.
    pub fn invoke(
        &self,
        export: &str,
        arguments: &[Value],
        cancellation: &CancellationToken,
    ) -> Result<Vec<Value>, ExecutionError> {
        let entry_work = export.len();
        if entry_work > self.limits.invocation_work {
            return Err(error(0, Fault::LimitExceeded));
        }
        cancellation
            .check()
            .map_err(|_| error(0, Fault::Cancelled))?;
        let function = self
            .exports
            .get(export)
            .ok_or_else(|| error(0, Fault::UnknownExport))?;
        machine::invoke(self, *function, arguments, entry_work, cancellation)
    }

    fn block_arity(&self, block: BlockType) -> (usize, usize) {
        match block {
            BlockType::Empty => (0, 0),
            BlockType::Result(_) => (0, 1),
            BlockType::TypeIndex(index) => {
                let ty = &self.module.module().declarations().types[index as usize].value;
                (ty.params.len(), ty.results.len())
            }
        }
    }
}

fn is_numeric(ty: ValueType) -> bool {
    matches!(
        ty,
        ValueType::I32 | ValueType::I64 | ValueType::F32 | ValueType::F64
    )
}

#[derive(Clone, Copy, Default)]
struct Jump {
    end: usize,
    otherwise: Option<usize>,
}

struct PreparedFunction<'a> {
    function: &'a Function,
    signature: &'a FunctionType,
    locals: usize,
    jumps: Vec<Jump>,
}

impl<'a> PreparedFunction<'a> {
    fn new(
        function: &'a Function,
        signature: &'a FunctionType,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ExecutionError> {
        budget.spend(offset, 1 + function.instructions.len())?;
        let mut locals = 0usize;
        for group in &function.locals {
            budget.spend(offset, 1)?;
            if !is_numeric(group.value_type) {
                return Err(error(offset, Fault::UnsupportedModule));
            }
            locals = locals
                .checked_add(group.count as usize)
                .ok_or_else(|| error(offset, Fault::LimitExceeded))?;
        }
        let mut jumps = Vec::new();
        jumps
            .try_reserve_exact(function.instructions.len())
            .map_err(|_| error(offset, Fault::LimitExceeded))?;
        jumps.resize(function.instructions.len(), Jump::default());
        let mut open: Vec<usize> = Vec::new();
        for (pc, instruction) in function.instructions.iter().enumerate() {
            budget.spend(instruction.offset, 1)?;
            match instruction.opcode {
                Opcode::Core(0x02..=0x04) => append(&mut open, pc, instruction.offset)?,
                Opcode::Core(0x05) => {
                    jumps[*open.last().expect("validated if")].otherwise = Some(pc + 1);
                }
                Opcode::Core(0x0b) => {
                    if let Some(opener) = open.pop() {
                        jumps[opener].end = pc;
                    }
                }
                Opcode::Core(0x00 | 0x01 | 0x0c..=0x10 | 0x12 | 0x1a..=0x1c | 0x20..=0x22) => {}
                opcode if effect(opcode).is_some() => {}
                _ => return Err(error(instruction.offset, Fault::UnsupportedInstruction)),
            }
            prepare_types(instruction)?;
        }
        Ok(Self {
            function,
            signature,
            locals,
            jumps,
        })
    }
}

fn prepare_types(instruction: &Instruction) -> Result<(), ExecutionError> {
    use crate::Immediate;
    let types_supported = match &instruction.immediate {
        Immediate::Block(BlockType::Result(ty)) => is_numeric(*ty),
        Immediate::Select(types) => types.iter().copied().all(is_numeric),
        _ => true,
    };
    if types_supported {
        Ok(())
    } else {
        Err(error(instruction.offset, Fault::UnsupportedModule))
    }
}

struct Budget<'a> {
    remaining: usize,
    cancellation: &'a CancellationToken,
}

impl Budget<'_> {
    fn spend(&mut self, offset: usize, work: usize) -> Result<(), ExecutionError> {
        self.cancellation
            .check()
            .map_err(|_| error(offset, Fault::Cancelled))?;
        self.remaining = self
            .remaining
            .checked_sub(work)
            .ok_or_else(|| error(offset, Fault::LimitExceeded))?;
        Ok(())
    }
}

fn append<T>(items: &mut Vec<T>, value: T, offset: usize) -> Result<(), ExecutionError> {
    items
        .try_reserve(1)
        .map_err(|_| error(offset, Fault::LimitExceeded))?;
    items.push(value);
    Ok(())
}

fn error(offset: usize, kind: Fault) -> ExecutionError {
    ExecutionError { offset, kind }
}
