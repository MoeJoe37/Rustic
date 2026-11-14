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
pub(crate) use compile::{ModuleCode, compile_module};
pub use interp::VmLimits;
pub(crate) use interp::exponentiate;
pub(crate) use scope::DEFAULT_EXPORT;

use crate::ast::Program;
use crate::parser::{ParseError, ParseLimits, parse_eval, parse_script};
use crate::runtime::{Abrupt, Agent, JsResult, Value};
use crate::runtime::{EnvId, ErrorKind, StringId};
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

    /// `CreateDynamicFunction` (§20.2.1.1.1) for ordinary and generator functions: the
    /// `Function` constructor. The parameters and body are parsed as one
    /// function expression in the global scope; text that closes the
    /// function early to smuggle in other code parses as something else
    /// and is a SyntaxError.
    /// `keyword` is `function` or `function*`.
    pub(crate) fn create_dynamic_function(
        &mut self,
        keyword: &str,
        params: &str,
        body: &str,
    ) -> JsResult<Value> {
        use crate::ast::{ExpressionKind, StatementKind};
        // The function is unnamed so that `anonymous` is not bound inside it;
        // its `name` is set afterwards.
        let text = format!("({keyword} ({params}\n) {{\n{body}\n}})");
        let syntax_error = |agent: &mut Agent, message: &str| {
            Err(agent.throw(crate::runtime::ErrorKind::SyntaxError, message))
        };
        let Ok(source) = Source::from_str_limited(&text, MAX_SOURCE_UNITS) else {
            return syntax_error(self, "function source too long");
        };
        let mut limits = ParseLimits::default();
        if let Some(token) = self.cancellation() {
            limits.cancellation = token;
        }
        let program = match parse_script(&source, &limits) {
            Ok(program) => program,
            Err(ParseError::Cancelled) => return Err(Abrupt::Cancelled),
            Err(e) => return syntax_error(self, &e.to_string()),
        };
        let single_function = match program.body.as_slice() {
            [statement] => match &statement.kind {
                StatementKind::Expression { expression, .. } => matches!(
                    &expression.kind,
                    ExpressionKind::Parenthesized(inner)
                        if matches!(inner.kind, ExpressionKind::Function(_))
                ),
                _ => false,
            },
            _ => false,
        };
        if !single_function {
            return syntax_error(self, "invalid function parameters or body");
        }
        let code = match self.compile_script(&program) {
            Ok(code) => code,
            Err(CompileError::OutOfMemory) => return Err(Abrupt::OutOfMemory),
            Err(e) => return syntax_error(self, &format!("{e:?}")),
        };
        let f = self.run_script(code)?;
        if let Some(o) = f.as_object() {
            let name = crate::runtime::PropertyKey::String(self.names.name);
            let anonymous = self.string("anonymous")?;
            self.define_property_or_throw(
                o,
                name,
                crate::runtime::PropertyDescriptor::data_with(anonymous, false, false, true),
            )?;
        }
        Ok(f)
    }

    /// PerformEval (§19.2.1.1) from its parse on: parse and compile the
    /// code, then EvalDeclarationInstantiation (§19.2.1.3). `env` is the
    /// caller's lexical environment; `var_hops` leads from it to the
    /// variable environment, or is `None` for the global environment.
    /// Returns the code to run on `env` with the caller's `this`.
    pub(crate) fn prepare_eval(
        &mut self,
        text: StringId,
        env: EnvId,
        var_hops: Option<u16>,
        strict_caller: bool,
        in_function: bool,
    ) -> JsResult<CodeId> {
        let units = self.heap.string(text).units().collect();
        let syntax_error =
            |agent: &mut Agent, message: &str| Err(agent.throw(ErrorKind::SyntaxError, message));
        let Ok(source) = Source::from_utf16(units, MAX_SOURCE_UNITS) else {
            return syntax_error(self, "eval source too long");
        };
        let mut limits = ParseLimits::default();
        if let Some(token) = self.cancellation() {
            limits.cancellation = token;
        }
        let program = match parse_eval(&source, &limits, strict_caller, in_function) {
            Ok(program) => program,
            Err(ParseError::Cancelled) => return Err(Abrupt::Cancelled),
            Err(e) => return syntax_error(self, &e.to_string()),
        };
        let code = match compile::compile_eval(self, &program, var_hops) {
            Ok(code) => code,
            Err(CompileError::OutOfMemory) => return Err(Abrupt::OutOfMemory),
            Err(e) => return syntax_error(self, &format!("{e:?}")),
        };
        if program.strict {
            // Strict eval code keeps its declarations to itself.
            return Ok(code);
        }
        let (function_names, var_names) = compile::eval_var_names(&program);
        let functions = function_names.len();
        let mut names = Vec::new();
        for name in function_names.iter().chain(&var_names) {
            names.push(self.heap.intern_str(name)?);
        }
        drop(program);
        // Nothing below collects garbage (this runs inside a native call
        // or an op), so the new code needs no root.
        let global = self.global_env;
        let var_env = match var_hops {
            Some(hops) => self.env_outer_n(env, hops),
            None => global,
        };
        // A var may not hoist past a lexical declaration of the same name
        // (step 3 for the global environment, step 4 for the rest).
        if var_env == global {
            for &name in &names {
                if self.has_lexical_declaration(global, name) {
                    return syntax_error(self, "redeclaration of a lexical binding by eval");
                }
            }
        }
        // Catch parameters are not lexical, so a var may share their name
        // (§B.3.4). A function's own top-level lexical declarations share
        // its variable environment here, so it is checked too.
        let mut at = env;
        loop {
            for &name in &names {
                if self.has_lexical_binding(at, name) {
                    return syntax_error(self, "redeclaration of a lexical binding by eval");
                }
            }
            if at == var_env {
                break;
            }
            match self.outer_environment(at) {
                Some(outer) => at = outer,
                None => break,
            }
        }
        if var_env == global {
            let object = self.intrinsics.global;
            for &name in &names[..functions] {
                if !self.can_declare_global_function(object, name)? {
                    return Err(self.throw(
                        ErrorKind::TypeError,
                        "cannot redefine a global property as a function",
                    ));
                }
            }
            for &name in &names[functions..] {
                if !self.can_declare_global_var(object, name)? {
                    return Err(self.throw(
                        ErrorKind::TypeError,
                        "cannot declare a global variable on a non-extensible global object",
                    ));
                }
            }
            for &name in &names {
                self.create_global_var_binding(global, name, true)?;
            }
        } else {
            for &name in &names {
                if !self.has_binding(var_env, name)? {
                    self.create_mutable_binding(var_env, name, true)?;
                    self.initialize_binding(var_env, name, Value::UNDEFINED)?;
                }
            }
        }
        Ok(code)
    }

    /// `eval(x)` called indirectly (§19.2.1.1 with `direct` false): global
    /// code with the global `this`.
    pub(crate) fn indirect_eval(&mut self, x: Value) -> JsResult<Value> {
        let Some(text) = x.as_string() else {
            return Ok(x);
        };
        let env = self.global_env;
        let code = self.prepare_eval(text, env, None, false, false)?;
        self.run_eval(code, env)
    }
}
