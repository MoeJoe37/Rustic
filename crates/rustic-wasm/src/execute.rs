use std::{borrow::Cow, cell::RefCell, collections::HashMap, fmt};

use rustic_base::CancellationToken;

use crate::validate::numeric::effect;
use crate::{
    BlockType, Declaration, Declarations, Export, ExportIndex, Function, FunctionType, GlobalType,
    Instruction, Opcode, ValidatedModule, ValueType,
};

mod imports;
mod machine;
mod memory;
mod numeric;
mod state;
mod table;

use imports::BoundFunction;
pub use imports::{FunctionImport, HostDispatch, HostTrap};
pub use memory::{LinearMemory, MEMORY_PAGE_BYTES, MEMORY32_MAX_PAGES};
pub use state::NumericGlobal;
use state::State;

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
}

#[derive(Clone, Copy)]
enum Slot {
    Number(Value),
    Ref(Reference),
}

impl Slot {
    fn zero(ty: ValueType) -> Self {
        match ty {
            ValueType::I32 => Self::Number(Value::I32(0)),
            ValueType::I64 => Self::Number(Value::I64(0)),
            ValueType::F32 => Self::Number(Value::F32(0)),
            ValueType::F64 => Self::Number(Value::F64(0)),
            ValueType::Ref { heap_type, .. } => Self::Ref(Reference::null(heap_type)),
            ValueType::V128 => unreachable!("validated scalar locals"),
        }
    }

    fn number(self) -> Value {
        match self {
            Self::Number(value) => value,
            Self::Ref(_) => unreachable!("validated numeric operand"),
        }
    }

    fn reference(self) -> Reference {
        match self {
            Self::Ref(value) => value,
            Self::Number(_) => unreachable!("validated reference operand"),
        }
    }

    fn i32(self) -> i32 {
        let Value::I32(value) = self.number() else {
            unreachable!("validated i32 operand");
        };
        value
    }

    fn address(self) -> u64 {
        match self.number() {
            Value::I32(value) => u64::from(value as u32),
            Value::I64(value) => value as u64,
            _ => unreachable!("validated integer operand"),
        }
    }
}

#[derive(Clone, Copy)]
enum Reference {
    Function(Option<u32>),
    NullExtern,
}

impl Reference {
    fn null(heap_type: i64) -> Self {
        match heap_type {
            -16 => Self::Function(None),
            -17 => Self::NullExtern,
            _ => unreachable!("validated abstract reference type"),
        }
    }

    fn is_null(self) -> bool {
        matches!(self, Self::Function(None) | Self::NullExtern)
    }
}

/// Why the interpreter rejected a module, invocation, or instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionErrorKind {
    /// The module declares state, imports, or types outside the instance profile.
    UnsupportedModule,
    /// An instruction needs support outside the instance profile.
    UnsupportedInstruction,
    /// The specified function export does not exist.
    UnknownExport,
    /// The supplied arguments do not match the function signature.
    ArgumentType,
    /// The function returns a value outside the host numeric API.
    ResultType,
    /// The embedder supplied more than one binding for the same import name.
    DuplicateImportBinding,
    /// The embedder did not supply a required function import.
    MissingImport,
    /// A function import's binding has an incompatible or nonnumeric signature.
    ImportTypeMismatch,
    /// An imported host function returned a trap.
    HostTrap,
    /// An imported host function wrote a result of the wrong type.
    HostResultType,
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
    /// A load, store or segment operation accessed bytes outside an allocation.
    MemoryOutOfBounds,
    /// A table index or element range lies outside its allocation.
    TableOutOfBounds,
    /// An instruction required a non-null reference.
    NullReference,
    /// The selected function did not match the signature of the indirect call.
    IndirectCallTypeMismatch,
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
    executable: Executable<'module, 'bytes>,
}

impl<'module, 'bytes> NumericInstance<'module, 'bytes> {
    /// Prepare the control targets and run the optional start function once.
    pub fn instantiate(
        module: &'module ValidatedModule<'bytes>,
        limits: ExecutionLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, ExecutionError> {
        let mut budget = Budget {
            remaining: limits.preparation_work,
            cancellation,
        };
        let executable =
            Executable::prepare(module, limits, Profile::NumericOnly, None, &mut budget)?;
        executable.run_start(&RefCell::new(State::default()), cancellation, None)?;
        Ok(Self { executable })
    }

    /// Invoke an exported function with a fresh execution budget.
    pub fn invoke(
        &self,
        export: &str,
        arguments: &[Value],
        cancellation: &CancellationToken,
    ) -> Result<Vec<Value>, ExecutionError> {
        self.executable.invoke(
            export,
            arguments,
            &RefCell::new(State::default()),
            cancellation,
        )
    }
}

/// Resource limits for an instance with its own guest state.
#[derive(Clone, Copy, Debug)]
pub struct InstanceLimits {
    /// Preparation and execution limits.
    pub execution: ExecutionLimits,
    /// Maximum total bytes of linear memory across this instance.
    pub memory_bytes: usize,
    /// Maximum total entries in tables and retained passive element segments.
    pub table_elements: usize,
}

impl Default for InstanceLimits {
    fn default() -> Self {
        Self {
            execution: ExecutionLimits::default(),
            memory_bytes: 32 * 1024 * 1024,
            table_elements: 1_000_000,
        }
    }
}

/// An executable instance with local globals, 32-bit linear memories and tables.
///
/// State changes remain visible across invocations, including changes made
/// before a trap. Function references stay within the instance. An external
/// reference can hold null until the host bridge supplies external identities.
/// Host calls through [`Self::invoke`] exchange scalar numbers.
pub struct Instance<'module, 'bytes> {
    executable: Executable<'module, 'bytes>,
    state: RefCell<State<'bytes>>,
}

impl<'module, 'bytes> Instance<'module, 'bytes> {
    /// Prepare code, allocate state, apply active segments and run the start function.
    pub fn instantiate(
        module: &'module ValidatedModule<'bytes>,
        limits: InstanceLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, ExecutionError> {
        Self::instantiate_linked(module, limits, None, cancellation)
    }

    /// Link declared function imports to explicit bindings, then instantiate.
    ///
    /// Every declared import must name a supplied numeric function with the
    /// exact signature. Bindings remain borrowed for the instance's lifetime.
    pub fn instantiate_with_imports(
        module: &'module ValidatedModule<'bytes>,
        limits: InstanceLimits,
        imports: &[FunctionImport<'module>],
        cancellation: &CancellationToken,
    ) -> Result<Self, ExecutionError> {
        Self::instantiate_linked(module, limits, Some(imports), cancellation)
    }

    fn instantiate_linked(
        module: &'module ValidatedModule<'bytes>,
        limits: InstanceLimits,
        imports: Option<&[FunctionImport<'module>]>,
        cancellation: &CancellationToken,
    ) -> Result<Self, ExecutionError> {
        let mut budget = Budget {
            remaining: limits.execution.preparation_work,
            cancellation,
        };
        let executable = Executable::prepare(
            module,
            limits.execution,
            Profile::LocalState,
            imports,
            &mut budget,
        )?;
        let state = RefCell::new(State::new(
            module.module().declarations(),
            &[],
            &[],
            limits,
            &mut budget,
            |bytes, _, _| Ok(Cow::Borrowed(bytes)),
        )?);
        executable.run_start(&state, cancellation, None)?;
        Ok(Self { executable, state })
    }

    /// Look up a function export's signature without entering the instance.
    pub fn function_type(&self, export: &str) -> Option<&FunctionType> {
        let &index = self.executable.exports.get(export)?;
        Some(self.executable.function_signature(index))
    }

    /// Read a function's signature by its validated module index.
    pub fn function_type_at(&self, index: u32) -> Option<&FunctionType> {
        let index = index as usize;
        let code = &self.executable;
        (index < code.imports.len() + code.functions.len()).then(|| code.function_signature(index))
    }

    /// Read a global declaration's type.
    pub fn global_type(&self, index: u32) -> Option<GlobalType> {
        self.state
            .borrow()
            .globals
            .get(index as usize)
            .map(|slot| slot.ty())
    }

    /// Read a numeric global's current value, or return `None` for an invalid
    /// index or a reference-valued global.
    pub fn global_value(&self, index: u32) -> Option<Value> {
        match self.state.borrow().globals.get(index as usize)?.value() {
            Slot::Number(value) => Some(value),
            Slot::Ref(_) => None,
        }
    }

    /// Write a mutable numeric global. Invalid indices, types and writes to an
    /// immutable global return `false` and leave every global unchanged.
    pub fn set_global_value(&self, index: u32, value: Value) -> bool {
        let mut state = self.state.borrow_mut();
        let Some(slot) = state.globals.get_mut(index as usize) else {
            return false;
        };
        let ty = slot.ty();
        if !ty.mutable || ty.value_type != value.value_type() {
            return false;
        }
        slot.write_guest(Slot::Number(value));
        true
    }

    /// Share a numeric global with the host or another instance.
    /// Local storage is promoted once, preserving its value and mutability.
    pub fn share_numeric_global(&self, index: u32) -> Option<NumericGlobal> {
        self.state
            .borrow_mut()
            .globals
            .get_mut(index as usize)?
            .share()
    }

    /// Retain a handle to a memory in this instance's memory index space.
    pub fn memory(&self, index: u32) -> Option<LinearMemory> {
        self.state.borrow().memories.get(index as usize).cloned()
    }

    /// Invoke a numeric function export with fresh budgets and persistent state.
    pub fn invoke(
        &mut self,
        export: &str,
        arguments: &[Value],
        cancellation: &CancellationToken,
    ) -> Result<Vec<Value>, ExecutionError> {
        self.executable
            .invoke(export, arguments, &self.state, cancellation)
    }

    /// Invoke a function index with a per-call dispatcher for the declared
    /// numeric host imports. A dispatcher may reenter this instance: no
    /// interpreter state borrow crosses a call to the host.
    pub fn invoke_function_with_host(
        &self,
        function: u32,
        arguments: &[Value],
        cancellation: &CancellationToken,
        dispatch: &mut HostDispatch<'_>,
    ) -> Result<Vec<Value>, ExecutionError> {
        if self.function_type_at(function).is_none() {
            return Err(error(0, Fault::UnknownExport));
        }
        machine::invoke(
            &self.executable,
            &self.state,
            function as usize,
            arguments,
            0,
            cancellation,
            Some(dispatch),
        )
    }
}

impl Instance<'static, 'static> {
    /// Consume a validated module and retain the code and local instance state.
    ///
    /// The original input can be released after this call. Function bodies
    /// and types move into the instance. Export names and retained passive
    /// data are copied against the preparation budget. Borrowed callbacks
    /// use [`Self::instantiate_with_imports`].
    pub fn instantiate_owned(
        module: ValidatedModule<'_>,
        limits: InstanceLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, ExecutionError> {
        let instance = Self::prepare_owned(module, limits, cancellation, false, &[], &[])?;
        instance
            .executable
            .run_start(&instance.state, cancellation, None)?;
        Ok(instance)
    }

    /// Instantiate an owned module with function, memory and numeric global imports.
    /// Supply memories and globals in their own index spaces, and dispatch
    /// functions in function index order. The host resolves callable bindings.
    /// The start function runs with this dispatcher before returning.
    pub fn instantiate_owned_with_host(
        module: ValidatedModule<'_>,
        limits: InstanceLimits,
        cancellation: &CancellationToken,
        globals: &[NumericGlobal],
        memories: &[LinearMemory],
        dispatch: &mut HostDispatch<'_>,
    ) -> Result<Self, ExecutionError> {
        let instance = Self::prepare_owned(module, limits, cancellation, true, globals, memories)?;
        instance
            .executable
            .run_start(&instance.state, cancellation, Some(dispatch))?;
        Ok(instance)
    }

    fn prepare_owned(
        module: ValidatedModule<'_>,
        limits: InstanceLimits,
        cancellation: &CancellationToken,
        allow_imports: bool,
        globals: &[NumericGlobal],
        memories: &[LinearMemory],
    ) -> Result<Self, ExecutionError> {
        let module = module.into_module();
        let declarations = module.declarations;
        let mut budget = Budget {
            remaining: limits.execution.preparation_work,
            cancellation,
        };
        check_profile(
            &declarations,
            Profile::LocalState,
            allow_imports,
            &mut budget,
        )?;
        let imports = imports::for_dispatch(&declarations, &mut budget)?;
        let exports = prepare_exports(&declarations.exports, &mut budget, |name, offset| {
            let mut owned = String::new();
            owned
                .try_reserve_exact(name.len())
                .map_err(|_| error(offset, Fault::LimitExceeded))?;
            owned.push_str(name);
            Ok(Cow::Owned(owned))
        })?;
        let functions = prepare_functions(
            module
                .functions
                .into_iter()
                .map(|entry| (entry.offset, Cow::Owned(entry.value))),
            Profile::LocalState,
            &mut budget,
        )?;
        let state = RefCell::new(State::new(
            &declarations,
            globals,
            memories,
            limits,
            &mut budget,
            |bytes, offset, budget| {
                budget.spend(offset, bytes.len())?;
                let mut owned = Vec::new();
                owned
                    .try_reserve_exact(bytes.len())
                    .map_err(|_| error(offset, Fault::LimitExceeded))?;
                owned.extend_from_slice(bytes);
                Ok(Cow::Owned(owned))
            },
        )?);
        let executable = Executable {
            types: Cow::Owned(declarations.types),
            start: declarations.start.map(|entry| entry.value),
            imports,
            functions,
            exports,
            limits: limits.execution,
        };
        Ok(Self { executable, state })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Profile {
    NumericOnly,
    LocalState,
}

struct Executable<'module, 'bytes> {
    types: Cow<'module, [Declaration<FunctionType>]>,
    start: Option<u32>,
    imports: Vec<BoundFunction<'module>>,
    functions: Vec<PreparedFunction<'module>>,
    exports: HashMap<Cow<'bytes, str>, usize>,
    limits: ExecutionLimits,
}

impl<'module, 'bytes> Executable<'module, 'bytes> {
    fn prepare(
        module: &'module ValidatedModule<'bytes>,
        limits: ExecutionLimits,
        profile: Profile,
        imports: Option<&[FunctionImport<'module>]>,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ExecutionError> {
        let decoded = module.module();
        let declarations = decoded.declarations();
        check_profile(declarations, profile, imports.is_some(), budget)?;
        let imports = match imports {
            Some(bindings) => imports::link(declarations, bindings, budget)?,
            None => Vec::new(),
        };
        let functions = prepare_functions(
            decoded
                .functions()
                .iter()
                .map(|entry| (entry.offset, Cow::Borrowed(&entry.value))),
            profile,
            budget,
        )?;
        let exports = prepare_exports(&declarations.exports, budget, |name, _| {
            Ok(Cow::Borrowed(name))
        })?;
        Ok(Self {
            types: Cow::Borrowed(&declarations.types),
            start: declarations.start.as_ref().map(|entry| entry.value),
            imports,
            functions,
            exports,
            limits,
        })
    }

    fn run_start(
        &self,
        state: &RefCell<State<'bytes>>,
        cancellation: &CancellationToken,
        dispatch: Option<&mut HostDispatch<'_>>,
    ) -> Result<(), ExecutionError> {
        if let Some(start) = self.start {
            machine::invoke(self, state, start as usize, &[], 0, cancellation, dispatch)?;
        }
        Ok(())
    }

    fn invoke(
        &self,
        export: &str,
        arguments: &[Value],
        state: &RefCell<State<'bytes>>,
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
        machine::invoke(
            self,
            state,
            *function,
            arguments,
            entry_work,
            cancellation,
            None,
        )
    }

    fn block_arity(&self, block: BlockType) -> (usize, usize) {
        match block {
            BlockType::Empty => (0, 0),
            BlockType::Result(_) => (0, 1),
            BlockType::TypeIndex(index) => {
                let ty = self.signature(index);
                (ty.params.len(), ty.results.len())
            }
        }
    }

    fn signature(&self, index: u32) -> &FunctionType {
        &self.types[index as usize].value
    }

    fn function_signature(&self, index: usize) -> &FunctionType {
        self.signature(self.function(index).type_index())
    }

    fn function(&self, index: usize) -> Callee<'_, 'module> {
        if let Some(import) = self.imports.get(index) {
            Callee::Host(import)
        } else {
            let local = index - self.imports.len();
            Callee::Local(local, &self.functions[local])
        }
    }
}

enum Callee<'code, 'module> {
    Host(&'code BoundFunction<'module>),
    Local(usize, &'code PreparedFunction<'module>),
}

impl Callee<'_, '_> {
    fn type_index(&self) -> u32 {
        match self {
            Self::Host(import) => import.type_index,
            Self::Local(_, local) => local.function.type_index,
        }
    }
}

fn check_profile(
    declarations: &Declarations<'_>,
    profile: Profile,
    allow_imports: bool,
    budget: &mut Budget<'_>,
) -> Result<(), ExecutionError> {
    budget.spend(0, 0)?;
    let always_unsupported = [
        declarations
            .imports
            .first()
            .filter(|_| !allow_imports)
            .map(|entry| entry.offset),
        declarations.tags.first().map(|entry| entry.offset),
    ];
    let stateful = [
        declarations.tables.first().map(|entry| entry.offset),
        declarations.elements.first().map(|entry| entry.offset),
        declarations.memories.first().map(|entry| entry.offset),
        declarations.globals.first().map(|entry| entry.offset),
        declarations.data.first().map(|entry| entry.offset),
    ];
    let unsupported = always_unsupported.into_iter().chain(
        stateful
            .into_iter()
            .filter(|_| profile == Profile::NumericOnly),
    );
    if let Some(offset) = unsupported.flatten().min() {
        return Err(error(offset, Fault::UnsupportedModule));
    }
    for entry in &declarations.types {
        let types = entry.value.params.iter().chain(&entry.value.results);
        budget.spend(
            entry.offset,
            1 + entry.value.params.len() + entry.value.results.len(),
        )?;
        if profile == Profile::NumericOnly && !types.copied().all(is_numeric) {
            return Err(error(entry.offset, Fault::UnsupportedModule));
        }
    }
    Ok(())
}

fn prepare_functions<'a>(
    functions: impl Iterator<Item = (usize, Cow<'a, Function>)>,
    profile: Profile,
    budget: &mut Budget<'_>,
) -> Result<Vec<PreparedFunction<'a>>, ExecutionError> {
    let mut prepared = Vec::new();
    for (offset, function) in functions {
        let function = PreparedFunction::new(function, offset, profile, budget)?;
        append(&mut prepared, function, offset)?;
    }
    Ok(prepared)
}

fn prepare_exports<'input, 'output>(
    entries: &[Declaration<Export<'input>>],
    budget: &mut Budget<'_>,
    mut name: impl FnMut(&'input str, usize) -> Result<Cow<'output, str>, ExecutionError>,
) -> Result<HashMap<Cow<'output, str>, usize>, ExecutionError> {
    let mut exports = HashMap::new();
    for entry in entries {
        let offset = entry.offset;
        budget.spend(offset, 1 + entry.value.name.len())?;
        let ExportIndex::Function(index) = entry.value.target else {
            continue;
        };
        exports
            .try_reserve(1)
            .map_err(|_| error(offset, Fault::LimitExceeded))?;
        exports.insert(name(entry.value.name, offset)?, index as usize);
    }
    Ok(exports)
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
    function: Cow<'a, Function>,
    locals: usize,
    jumps: Vec<Jump>,
}

impl<'a> PreparedFunction<'a> {
    fn new(
        function: Cow<'a, Function>,
        offset: usize,
        profile: Profile,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ExecutionError> {
        budget.spend(offset, 1 + function.instructions.len())?;
        let mut locals = 0usize;
        for group in &function.locals {
            budget.spend(offset, 1)?;
            if profile == Profile::NumericOnly && !is_numeric(group.value_type) {
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
                Opcode::Core(
                    0x00 | 0x01 | 0x0c..=0x10 | 0x12 | 0x1a..=0x1c | 0x20..=0x24 | 0x28..=0x40,
                )
                | Opcode::Misc(8..=11) => {}
                Opcode::Core(0x11 | 0x13 | 0x25..=0x26 | 0xd0..=0xd2 | 0xd4)
                | Opcode::Misc(12..=17)
                    if profile == Profile::LocalState => {}
                opcode if effect(opcode).is_some() => {}
                _ => return Err(error(instruction.offset, Fault::UnsupportedInstruction)),
            }
            if profile == Profile::NumericOnly {
                prepare_numeric_types(instruction)?;
            }
        }
        Ok(Self {
            function,
            locals,
            jumps,
        })
    }
}

fn prepare_numeric_types(instruction: &Instruction) -> Result<(), ExecutionError> {
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
