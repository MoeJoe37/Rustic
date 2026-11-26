//! Rustic's bounded WebAssembly decoder, validator, and interpreter.
//!
//! Follow the module boundary from [`decode_module`] through
//! [`validate_module`] to [`Instance::instantiate`]. The scanner also exposes
//! borrowed section and instruction spans for diagnostic tools.

#![warn(missing_docs)]

mod binary;
mod execute;
mod instructions;
mod module;
mod reader;
mod sections;
mod validate;

pub use binary::{
    BinaryLimits, BinaryModule, FunctionBody, LocalGroup, Section, SectionKind, ValueType,
    scan_module,
};
pub use execute::{
    ExecutionError, ExecutionErrorKind, ExecutionLimits, ExternalReference, FunctionAddress,
    FunctionImport, HostCall, HostDispatch, HostTrap, Instance, InstanceImports, InstanceLimits,
    LinearMemory, MEMORY_PAGE_BYTES, MEMORY32_MAX_PAGES, NumericGlobal, NumericInstance,
    PreparedInstance, ReferenceRoot, ReferenceTable, TableEntry, TracingHostDispatch, Value,
};
pub use instructions::{
    BlockType, Immediate, Instruction, InstructionLimits, Opcode, decode_instructions,
};
pub use module::{
    ActiveSegment, AddressType, DataSegment, Declaration, Declarations, DecodedModule,
    ElementItems, ElementMode, ElementSegment, Export, ExportIndex, Function, FunctionType, Global,
    GlobalType, Import, ImportType, Limits, Table, TableType,
};
pub use reader::{BinaryError, BinaryErrorKind};
pub use sections::{DecodeLimits, decode_module};
pub use validate::{
    IndexSpace, ValidatedModule, ValidationError, ValidationErrorKind, ValidationLimits,
    validate_module,
};
