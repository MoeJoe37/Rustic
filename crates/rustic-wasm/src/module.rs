//! Decoded module declarations. Names and data bytes borrow the original input.

use crate::{Instruction, Section, ValueType};

/// A declaration and its absolute byte offset in the module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration<T> {
    /// Offset of the declaration's first byte.
    pub offset: usize,
    /// Contents of the declaration.
    pub value: T,
}

/// A function signature, including multi-value results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionType {
    /// Parameters in call order.
    pub params: Vec<ValueType>,
    /// Returned values in stack order.
    pub results: Vec<ValueType>,
}

/// The address type of a memory or table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressType {
    /// Addresses use i32.
    I32,
    /// Addresses use i64.
    I64,
}

impl AddressType {
    /// Integer type of addresses and page counts in guest instructions.
    pub const fn value_type(self) -> ValueType {
        match self {
            Self::I32 => ValueType::I32,
            Self::I64 => ValueType::I64,
        }
    }

    /// The maximum number of 64 KiB pages for this address width.
    pub const fn max_memory_pages(self) -> u64 {
        match self {
            Self::I32 => 1 << 16,
            Self::I64 => 1 << 48,
        }
    }
}

/// Declared memory pages or table elements. Validation checks their range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Address width.
    pub address_type: AddressType,
    /// Initial length.
    pub min: u64,
    /// Optional bound on growth.
    pub max: Option<u64>,
}

impl Limits {
    pub(crate) fn fits(self, max: u64) -> bool {
        self.min <= max
            && self
                .max
                .is_none_or(|bound| (self.min..=max).contains(&bound))
    }
}

/// Type of a table, excluding its optional initializer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableType {
    /// Reference type of elements.
    pub element_type: ValueType,
    /// Length limits in elements.
    pub limits: Limits,
}

/// Type of a global, excluding its initializer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalType {
    /// Type of the stored value.
    pub value_type: ValueType,
    /// Whether instructions may set the global.
    pub mutable: bool,
}

/// Type information supplied for a module import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportType {
    /// Index of the function signature.
    Function(u32),
    /// Table type.
    Table(TableType),
    /// Limits in 64 KiB memory pages.
    Memory(Limits),
    /// Global type.
    Global(GlobalType),
    /// Index of the exception tag signature.
    Tag(u32),
}

/// An unresolved import name and type. Decoding performs no resolution or I/O.
#[derive(Debug, PartialEq, Eq)]
pub struct Import<'a> {
    /// Module name.
    pub module: &'a str,
    /// Name within the imported module.
    pub name: &'a str,
    /// External type.
    pub ty: ImportType,
}

/// Index space and index selected by an export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportIndex {
    /// Function index.
    Function(u32),
    /// Table index.
    Table(u32),
    /// Memory index.
    Memory(u32),
    /// Global index.
    Global(u32),
    /// Exception tag index.
    Tag(u32),
}

/// An exported name and the definition it refers to.
#[derive(Debug, PartialEq, Eq)]
pub struct Export<'a> {
    /// Exported name.
    pub name: &'a str,
    /// Target definition.
    pub target: ExportIndex,
}

/// A table definition.
#[derive(Debug, PartialEq, Eq)]
pub struct Table {
    /// Element type and length limits.
    pub ty: TableType,
    /// Initial value expression, or implicit null for the compact encoding.
    pub initializer: Option<Vec<Instruction>>,
}

/// A global definition.
#[derive(Debug, PartialEq, Eq)]
pub struct Global {
    /// Global type.
    pub ty: GlobalType,
    /// Initial value expression.
    pub initializer: Vec<Instruction>,
}

/// Destination of an active data or element segment.
#[derive(Debug, PartialEq, Eq)]
pub struct ActiveSegment {
    /// Target memory or table index, according to segment kind.
    pub target: u32,
    /// Initial offset expression.
    pub offset: Vec<Instruction>,
}

/// Lifecycle of an element segment.
#[derive(Debug, PartialEq, Eq)]
pub enum ElementMode {
    /// Available to table instructions after instantiation.
    Passive,
    /// Instantiation initializes a table from this segment.
    Active(ActiveSegment),
    /// Declares references without populating a table.
    Declarative,
}

/// Element initializers as function indexes or explicit expressions.
#[derive(Debug, PartialEq, Eq)]
pub enum ElementItems {
    /// Compact encoding for function references.
    Functions(Vec<u32>),
    /// An expression for each element.
    Expressions(Vec<Vec<Instruction>>),
}

/// A table initialization, passive element list or declaration.
#[derive(Debug, PartialEq, Eq)]
pub struct ElementSegment {
    /// Initialization mode.
    pub mode: ElementMode,
    /// Reference type shared by the elements.
    pub element_type: ValueType,
    /// Elements in source order.
    pub items: ElementItems,
}

/// A byte span optionally used to initialize a memory.
#[derive(Debug, PartialEq, Eq)]
pub struct DataSegment<'a> {
    /// Active initialization. A passive segment has no target.
    pub active: Option<ActiveSegment>,
    /// Original bytes, borrowed without allocation.
    pub bytes: &'a [u8],
}

/// A function body after instruction decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    /// Index of the function signature.
    pub type_index: u32,
    /// Grouped non-parameter locals.
    pub locals: Vec<crate::LocalGroup>,
    /// The decoded body through the outer end.
    pub instructions: Vec<Instruction>,
}

/// The module's declarations in their respective index spaces. Imported
/// definitions are stored separately and precede local definitions in each space.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Declarations<'a> {
    /// Function signatures.
    pub types: Vec<Declaration<FunctionType>>,
    /// Unresolved imports.
    pub imports: Vec<Declaration<Import<'a>>>,
    /// Locally declared tables.
    pub tables: Vec<Declaration<Table>>,
    /// Locally declared memories, with lengths in 64 KiB pages.
    pub memories: Vec<Declaration<Limits>>,
    /// Locally declared globals.
    pub globals: Vec<Declaration<Global>>,
    /// Type indexes of locally declared exception tags.
    pub tags: Vec<Declaration<u32>>,
    /// Export names and targets.
    pub exports: Vec<Declaration<Export<'a>>>,
    /// Optional start function index.
    pub start: Option<Declaration<u32>>,
    /// Element segments.
    pub elements: Vec<Declaration<ElementSegment>>,
    /// Data segments.
    pub data: Vec<Declaration<DataSegment<'a>>>,
    /// Data count section if the binary supplied one.
    pub data_count: Option<u32>,
}

/// A decoded module with syntax checked in the supported binary profile.
/// A separate type and index validation pass is required before execution.
#[derive(Debug, PartialEq, Eq)]
pub struct DecodedModule<'a> {
    pub(crate) declarations: Declarations<'a>,
    pub(crate) functions: Vec<Declaration<Function>>,
    pub(crate) custom_sections: Vec<Section<'a>>,
}

impl<'a> DecodedModule<'a> {
    /// Decoded declarations and initializer expressions.
    pub fn declarations(&self) -> &Declarations<'a> {
        &self.declarations
    }

    /// Locally defined functions by their local order.
    pub fn functions(&self) -> &[Declaration<Function>] {
        &self.functions
    }

    /// Advisory custom sections in encounter order.
    pub fn custom_sections(&self) -> &[Section<'a>] {
        &self.custom_sections
    }
}
