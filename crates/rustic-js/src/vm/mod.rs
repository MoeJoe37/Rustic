//! JS-04: bytecode, the compiler from the syntax tree, and the
//! interpreter (ADR 0017).
//!
//! A script is parsed, compiled to register bytecode and then run; the
//! syntax tree is dropped after compilation. [`Agent::evaluate`] does all
//! three.

pub(crate) mod bytecode;
mod compile;
pub(crate) mod interp;
mod scope;

use std::fmt;

pub use bytecode::CodeId;
pub use compile::CompileError;
pub use interp::VmLimits;

use crate::ast::Program;
use crate::parser::{ParseError, ParseLimits, parse_script};
use crate::runtime::{Abrupt, Agent, Value};
use crate::source::{MAX_SOURCE_UNITS, Source};

/// Why [`Agent::evaluate`] produced no value.
#[derive(Clone, Debug, PartialEq)]
pub enum EvalError {
    /// The source is longer than [`MAX_SOURCE_UNITS`].
    SourceTooLong,
    /// The source did not parse.
    Parse(ParseError),
    /// The script uses syntax the engine does not run yet, or is too
    /// large or too deeply nested to compile.
    Compile(CompileError),
    /// The script threw this value.
    Throw(Value),
    /// The heap budget ran out.
    OutOfMemory,
    /// The host cancelled the run.
    Cancelled,
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::SourceTooLong => f.write_str("source too long"),
            EvalError::Parse(e) => e.fmt(f),
            EvalError::Compile(e) => write!(f, "{e:?}"),
            EvalError::Throw(_) => f.write_str("uncaught exception"),
            EvalError::OutOfMemory => f.write_str("out of memory"),
            EvalError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for EvalError {}

impl From<Abrupt> for EvalError {
    fn from(abrupt: Abrupt) -> Self {
        match abrupt {
            Abrupt::Throw(value) => EvalError::Throw(value),
            Abrupt::OutOfMemory => EvalError::OutOfMemory,
            Abrupt::Cancelled => EvalError::Cancelled,
        }
    }
}

impl Agent {
    /// Compile a parsed script. The result is a heap record like any
    /// other: unless it is run or rooted it goes at the next collection.
    pub fn compile_script(&mut self, program: &Program) -> Result<CodeId, CompileError> {
        compile::compile_script(self, program)
    }

    /// Parse, compile and run `text` as a script, returning its completion
    /// value. The run collects garbage at its safe points, so handles the
    /// caller keeps across the call must be rooted ([`Agent::root`]).
    pub fn evaluate(&mut self, text: &str) -> Result<Value, EvalError> {
        let source = Source::from_str_limited(text, MAX_SOURCE_UNITS)
            .map_err(|_| EvalError::SourceTooLong)?;
        let mut limits = ParseLimits::default();
        if let Some(token) = self.cancellation() {
            limits.cancellation = token;
        }
        let code = {
            let program = parse_script(&source, &limits).map_err(|e| match e {
                ParseError::Cancelled => EvalError::Cancelled,
                other => EvalError::Parse(other),
            })?;
            self.compile_script(&program).map_err(EvalError::Compile)?
        };
        Ok(self.run_script(code)?)
    }
}
