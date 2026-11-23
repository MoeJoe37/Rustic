use std::{collections::HashSet, fmt};

use rustic_base::CancellationToken;

use crate::module::*;
use crate::{Immediate, Instruction, Opcode, ValueType};

mod body;
pub(crate) mod numeric;

/// Index spaces checked by the validator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexSpace {
    /// Function signatures.
    Type,
    /// Imported and defined functions.
    Function,
    /// Imported and defined tables.
    Table,
    /// Imported and defined memories.
    Memory,
    /// Imported and defined globals.
    Global,
    /// Imported and defined exception tags.
    Tag,
    /// Elements in a module's element section.
    Element,
    /// Segments in a module's data section.
    Data,
    /// The current function's parameters and locals.
    Local,
    /// Open control frames, starting with the innermost one.
    Label,
}

/// Why the module failed validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationErrorKind {
    /// An instruction or declaration names a missing or invisible index.
    InvalidIndex(IndexSpace),
    /// An instruction consumes a value of the wrong type.
    TypeMismatch,
    /// An instruction reads below its reachable control frame.
    StackUnderflow,
    /// An expression ends with extra operands.
    ExtraOperands,
    /// A select, branch, if, tail call, start function or tag has the wrong signature.
    InvalidSignature,
    /// A declaration needs type support outside the current profile.
    UnsupportedType,
    /// An opcode needs validation outside the current profile.
    UnsupportedInstruction,
    /// A memory or table limit is invalid or a load/store has an invalid immediate.
    InvalidLimits,
    /// An initializer contains an instruction that is not a constant expression.
    NonConstant,
    /// An instruction would write to an immutable global.
    ImmutableGlobal,
    /// A function reference was not declared outside the function bodies.
    UndeclaredFunction,
    /// The module exports the same name more than once.
    DuplicateExport,
    /// A bulk-memory instruction refers to a data segment without a data count section.
    MissingDataCount,
    /// A validation budget was exhausted.
    LimitExceeded,
    /// The host cancelled validation.
    Cancelled,
}

/// A validation error located at its instruction or declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationError {
    /// Absolute offset of the instruction or declaration.
    pub offset: usize,
    /// Reason for validation failure.
    pub kind: ValidationErrorKind,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, writer: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            writer,
            "WebAssembly validation {:?} at byte {}",
            self.kind, self.offset
        )
    }
}

impl std::error::Error for ValidationError {}

/// Budgets for validating a decoded module.
#[derive(Clone, Copy, Debug)]
pub struct ValidationLimits {
    /// Maximum operand stack height during validation.
    pub operands: usize,
    /// Maximum parameters and locals in one function.
    pub locals: usize,
    /// Total work across declarations, instruction visits and stack operations.
    pub work: usize,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            operands: 100_000,
            locals: 100_000,
            work: 16_000_000,
        }
    }
}

/// A module that passed the supported profile's type and index checks.
#[derive(Debug)]
pub struct ValidatedModule<'a> {
    module: DecodedModule<'a>,
}

impl<'a> ValidatedModule<'a> {
    /// Inspect the original declarations and type-checked instructions.
    pub fn module(&self) -> &DecodedModule<'a> {
        &self.module
    }
}

/// Validate numeric and multi-value control flow, local and global access,
/// direct and indirect calls, 32-bit memories and tables, and bulk operations.
/// Reference validation covers abstract function and external references.
/// GC types, SIMD, non-null declared storage and 64-bit addressing are outside
/// this profile. Validation performs no linking, allocation of guest memories,
/// or execution.
pub fn validate_module<'a>(
    module: DecodedModule<'a>,
    limits: ValidationLimits,
    cancellation: &CancellationToken,
) -> Result<ValidatedModule<'a>, ValidationError> {
    let mut budget = Budget {
        limits,
        remaining: limits.work,
        cancellation,
    };
    let context = Context::new(&module, &mut budget)?;
    context.validate_declarations(&mut budget)?;
    for function in &module.functions {
        body::function(&context, function, &mut budget)?;
    }
    budget.spend(0, 0)?;
    Ok(ValidatedModule { module })
}

use ValidationErrorKind as Fault;
use ValueType as Ty;

const FUNCREF: Ty = Ty::Ref {
    heap_type: -16,
    nullable: true,
};
const EXTERNREF: Ty = Ty::Ref {
    heap_type: -17,
    nullable: true,
};

fn error(offset: usize, kind: Fault) -> ValidationError {
    ValidationError { offset, kind }
}

struct Budget<'a> {
    limits: ValidationLimits,
    remaining: usize,
    cancellation: &'a CancellationToken,
}

impl Budget<'_> {
    fn spend(&mut self, offset: usize, work: usize) -> Result<(), ValidationError> {
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

fn declared_type(ty: Ty, offset: usize) -> Result<(), ValidationError> {
    match ty {
        Ty::I32 | Ty::I64 | Ty::F32 | Ty::F64 | FUNCREF | EXTERNREF => Ok(()),
        _ => Err(error(offset, Fault::UnsupportedType)),
    }
}

fn matches_type(actual: Option<Ty>, expected: Ty) -> bool {
    match (actual, expected) {
        (None, _) => true,
        (
            Some(Ty::Ref {
                heap_type: a,
                nullable: false,
            }),
            Ty::Ref { heap_type: b, .. },
        ) => a == b,
        (Some(actual), expected) => actual == expected,
    }
}

fn matches_values(
    actual: &[Ty],
    expected: &[Ty],
    offset: usize,
    budget: &mut Budget<'_>,
) -> Result<bool, ValidationError> {
    budget.spend(offset, actual.len())?;
    Ok(actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(&a, &b)| matches_type(Some(a), b)))
}

fn index<T>(
    items: &[T],
    index: u32,
    space: IndexSpace,
    offset: usize,
) -> Result<&T, ValidationError> {
    items
        .get(index as usize)
        .ok_or_else(|| error(offset, Fault::InvalidIndex(space)))
}

fn append<T>(items: &mut Vec<T>, value: T, offset: usize) -> Result<(), ValidationError> {
    items
        .try_reserve(1)
        .map_err(|_| error(offset, Fault::LimitExceeded))?;
    items.push(value);
    Ok(())
}

fn allocation_limits(limits: Limits, max: u64, offset: usize) -> Result<(), ValidationError> {
    if limits.address_type != AddressType::I32 {
        return Err(error(offset, Fault::UnsupportedType));
    }
    if limits.min > max
        || limits
            .max
            .is_some_and(|bound| bound > max || bound < limits.min)
    {
        return Err(error(offset, Fault::InvalidLimits));
    }
    Ok(())
}

struct Context<'a, 'bytes> {
    declarations: &'a Declarations<'bytes>,
    functions: Vec<&'a FunctionType>,
    globals: Vec<GlobalType>,
    imported_globals: usize,
    memories: usize,
    tables: Vec<TableType>,
    tags: usize,
    referenced_functions: Vec<bool>,
}

impl<'a, 'bytes> Context<'a, 'bytes> {
    fn new(
        module: &'a DecodedModule<'bytes>,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ValidationError> {
        let declarations = &module.declarations;
        let mut context = Self {
            declarations,
            functions: Vec::new(),
            globals: Vec::new(),
            imported_globals: 0,
            memories: 0,
            tables: Vec::new(),
            tags: 0,
            referenced_functions: Vec::new(),
        };
        budget.spend(0, 0)?;
        for ty in &declarations.types {
            budget.spend(
                ty.offset,
                1 + ty.value.params.len() + ty.value.results.len(),
            )?;
            for &value in ty.value.params.iter().chain(&ty.value.results) {
                declared_type(value, ty.offset)?;
            }
        }
        for import in &declarations.imports {
            let offset = import.offset;
            budget.spend(offset, 1)?;
            match import.value.ty {
                ImportType::Function(ty) => context.add_function(ty, offset)?,
                ImportType::Table(ty) => context.add_table(ty, offset)?,
                ImportType::Memory(limits) => context.add_memory(limits, offset)?,
                ImportType::Global(ty) => context.add_global(ty, offset)?,
                ImportType::Tag(ty) => context.add_tag(ty, offset)?,
            }
        }
        context.imported_globals = context.globals.len();
        for entry in &module.functions {
            budget.spend(entry.offset, 1)?;
            context.add_function(entry.value.type_index, entry.offset)?;
        }
        for (entry, offset) in declarations
            .globals
            .iter()
            .map(|item| (item.value.ty, item.offset))
        {
            budget.spend(offset, 1)?;
            context.add_global(entry, offset)?;
        }
        for entry in &declarations.memories {
            budget.spend(entry.offset, 1)?;
            context.add_memory(entry.value, entry.offset)?;
        }
        for entry in &declarations.tables {
            budget.spend(entry.offset, 1)?;
            context.add_table(entry.value.ty, entry.offset)?;
        }
        for entry in &declarations.tags {
            budget.spend(entry.offset, 1)?;
            context.add_tag(entry.value, entry.offset)?;
        }
        context
            .referenced_functions
            .try_reserve_exact(context.functions.len())
            .map_err(|_| error(0, Fault::LimitExceeded))?;
        context
            .referenced_functions
            .resize(context.functions.len(), false);
        context.collect_function_references(budget)?;
        Ok(context)
    }

    fn signature(&self, ty: u32, offset: usize) -> Result<&'a FunctionType, ValidationError> {
        Ok(&index(&self.declarations.types, ty, IndexSpace::Type, offset)?.value)
    }

    fn add_function(&mut self, ty: u32, offset: usize) -> Result<(), ValidationError> {
        let signature = self.signature(ty, offset)?;
        append(&mut self.functions, signature, offset)
    }

    fn add_table(&mut self, ty: TableType, offset: usize) -> Result<(), ValidationError> {
        declared_type(ty.element_type, offset)?;
        allocation_limits(ty.limits, u32::MAX.into(), offset)?;
        append(&mut self.tables, ty, offset)
    }

    fn add_memory(&mut self, limits: Limits, offset: usize) -> Result<(), ValidationError> {
        allocation_limits(limits, 65_536, offset)?;
        self.memories += 1;
        Ok(())
    }

    fn add_global(&mut self, ty: GlobalType, offset: usize) -> Result<(), ValidationError> {
        declared_type(ty.value_type, offset)?;
        append(&mut self.globals, ty, offset)
    }

    fn add_tag(&mut self, ty: u32, offset: usize) -> Result<(), ValidationError> {
        if !self.signature(ty, offset)?.results.is_empty() {
            return Err(error(offset, Fault::InvalidSignature));
        }
        self.tags += 1;
        Ok(())
    }

    fn collect_function_references(
        &mut self,
        budget: &mut Budget<'_>,
    ) -> Result<(), ValidationError> {
        let declarations = self.declarations;
        let expressions = declarations
            .globals
            .iter()
            .map(|g| g.value.initializer.as_slice())
            .chain(
                declarations
                    .tables
                    .iter()
                    .filter_map(|t| t.value.initializer.as_deref()),
            );
        let refs = &mut self.referenced_functions;
        for instructions in expressions {
            collect_refs(refs, instructions, budget)?;
        }
        for element in &declarations.elements {
            budget.spend(element.offset, 1)?;
            if let ElementMode::Active(active) = &element.value.mode {
                collect_refs(refs, &active.offset, budget)?;
            }
            match &element.value.items {
                ElementItems::Functions(functions) => {
                    budget.spend(element.offset, functions.len())?;
                    for &function in functions {
                        mark_ref(refs, function, element.offset)?;
                    }
                }
                ElementItems::Expressions(expressions) => {
                    for instructions in expressions {
                        collect_refs(refs, instructions, budget)?;
                    }
                }
            }
        }
        for export in &declarations.exports {
            budget.spend(export.offset, 1)?;
            if let ExportIndex::Function(function) = export.value.target {
                mark_ref(refs, function, export.offset)?;
            }
        }
        Ok(())
    }

    fn validate_declarations(&self, budget: &mut Budget<'_>) -> Result<(), ValidationError> {
        let declarations = self.declarations;
        for (index, global) in declarations.globals.iter().enumerate() {
            body::constant(
                self,
                &global.value.initializer,
                global.value.ty.value_type,
                self.imported_globals + index,
                budget,
            )?;
        }
        for table in &declarations.tables {
            if let Some(initializer) = &table.value.initializer {
                body::constant(
                    self,
                    initializer,
                    table.value.ty.element_type,
                    self.imported_globals,
                    budget,
                )?;
            }
        }
        for segment in &declarations.data {
            budget.spend(segment.offset, 1)?;
            if let Some(active) = &segment.value.active {
                self.memory(active.target, segment.offset)?;
                body::constant(self, &active.offset, Ty::I32, self.globals.len(), budget)?;
            }
        }
        for segment in &declarations.elements {
            let ty = segment.value.element_type;
            match &segment.value.items {
                ElementItems::Functions(_) => {}
                ElementItems::Expressions(expressions) => {
                    declared_type(ty, segment.offset)?;
                    for initializer in expressions {
                        body::constant(self, initializer, ty, self.globals.len(), budget)?;
                    }
                }
            }
            if let ElementMode::Active(active) = &segment.value.mode {
                let table = self.table(active.target, segment.offset)?;
                if !matches_type(Some(ty), table.element_type) {
                    return Err(error(segment.offset, Fault::TypeMismatch));
                }
                body::constant(self, &active.offset, Ty::I32, self.globals.len(), budget)?;
            }
        }
        if let Some(start) = &declarations.start {
            let signature = self.function(start.value, start.offset)?;
            if !signature.params.is_empty() || !signature.results.is_empty() {
                return Err(error(start.offset, Fault::InvalidSignature));
            }
        }
        let mut names = HashSet::new();
        names
            .try_reserve(declarations.exports.len())
            .map_err(|_| error(0, Fault::LimitExceeded))?;
        for export in &declarations.exports {
            let offset = export.offset;
            budget.spend(offset, 1 + export.value.name.len())?;
            if !names.insert(export.value.name) {
                return Err(error(offset, Fault::DuplicateExport));
            }
            match export.value.target {
                ExportIndex::Function(value) => {
                    self.function(value, offset)?;
                }
                ExportIndex::Memory(value) => self.memory(value, offset)?,
                ExportIndex::Table(value) => {
                    self.table(value, offset)?;
                }
                ExportIndex::Global(value) => {
                    self.global(value, offset)?;
                }
                ExportIndex::Tag(value) => check_index(value, self.tags, IndexSpace::Tag, offset)?,
            }
        }
        Ok(())
    }

    fn function(&self, value: u32, offset: usize) -> Result<&'a FunctionType, ValidationError> {
        Ok(*index(
            &self.functions,
            value,
            IndexSpace::Function,
            offset,
        )?)
    }

    fn memory(&self, value: u32, offset: usize) -> Result<(), ValidationError> {
        check_index(value, self.memories, IndexSpace::Memory, offset)
    }

    fn global(&self, value: u32, offset: usize) -> Result<GlobalType, ValidationError> {
        Ok(*index(&self.globals, value, IndexSpace::Global, offset)?)
    }

    fn table(&self, value: u32, offset: usize) -> Result<TableType, ValidationError> {
        Ok(*index(&self.tables, value, IndexSpace::Table, offset)?)
    }

    fn element(&self, value: u32, offset: usize) -> Result<Ty, ValidationError> {
        Ok(index(
            &self.declarations.elements,
            value,
            IndexSpace::Element,
            offset,
        )?
        .value
        .element_type)
    }

    fn data(&self, value: u32, offset: usize) -> Result<(), ValidationError> {
        if self.declarations.data_count.is_none() {
            return Err(error(offset, Fault::MissingDataCount));
        }
        check_index(
            value,
            self.declarations.data.len(),
            IndexSpace::Data,
            offset,
        )
    }
}

fn collect_refs(
    refs: &mut [bool],
    instructions: &[Instruction],
    budget: &mut Budget<'_>,
) -> Result<(), ValidationError> {
    for op in instructions {
        budget.spend(op.offset, 1)?;
        if let (Opcode::Core(0xd2), Immediate::Index(function)) = (op.opcode, &op.immediate) {
            mark_ref(refs, *function, op.offset)?;
        }
    }
    Ok(())
}

fn mark_ref(refs: &mut [bool], function: u32, offset: usize) -> Result<(), ValidationError> {
    let reference = refs
        .get_mut(function as usize)
        .ok_or_else(|| error(offset, Fault::InvalidIndex(IndexSpace::Function)))?;
    *reference = true;
    Ok(())
}

fn check_index(
    value: u32,
    len: usize,
    space: IndexSpace,
    offset: usize,
) -> Result<(), ValidationError> {
    if (value as usize) < len {
        Ok(())
    } else {
        Err(error(offset, Fault::InvalidIndex(space)))
    }
}
