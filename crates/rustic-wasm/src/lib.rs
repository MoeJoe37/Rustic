//! WebAssembly binary admission for WASM-01.
//!
//! The scanner verifies version 1 framing and exposes borrowed section and
//! instruction spans. It reads custom names, function type indexes, local
//! declarations and the data count. The separate instruction decoder checks
//! control delimiters and reads the supported operators and operands.

#![warn(missing_docs)]

mod binary;
mod instructions;
mod reader;

pub use binary::{
    BinaryLimits, BinaryModule, FunctionBody, LocalGroup, Section, SectionKind, ValueType,
    scan_module,
};
pub use instructions::{
    BlockType, Immediate, Instruction, InstructionLimits, Opcode, decode_instructions,
};
pub use reader::{BinaryError, BinaryErrorKind};
