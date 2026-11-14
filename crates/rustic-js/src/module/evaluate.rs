//! Linking and synchronous evaluation of source text modules (§16.2.1.5).

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::hash::Hash;

use super::{GraphError, GraphLimits, ImportName, ModuleGraph, ResolvedBinding, ResolvedExport};
use crate::runtime::{
    Abrupt, Agent, EnvId, ImportBinding, JsStr, NamespaceBinding, ObjectId, Value,
};
use crate::vm::{CodeId, CompileError, DEFAULT_EXPORT, ModuleCode, compile_module};

/// Failure while instantiating or evaluating a module graph.
#[derive(Debug, PartialEq)]
pub enum ModuleEvalError {
    /// A module-count or linking limit was exceeded, or linking was cancelled.
    Graph(GraphError<Infallible>),
    /// A module's code could not be compiled.
    Compile {
        /// Index of the failed module.
        module: usize,
        /// Compilation error with its original source span, when applicable.
        error: CompileError,
    },
    /// Evaluation failed with an exception, cancellation or heap exhaustion.
    Runtime(Abrupt),
}

impl fmt::Display for ModuleEvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(error) => write!(f, "module linking failed: {error:?}"),
            Self::Compile { module, error } => {
                write!(f, "module {module} compilation failed: {error:?}")
            }
            Self::Runtime(error) => write!(f, "module evaluation failed: {error:?}"),
        }
    }
}

impl std::error::Error for ModuleEvalError {}

impl From<Abrupt> for ModuleEvalError {
    fn from(abrupt: Abrupt) -> Self {
        Self::Runtime(abrupt)
    }
}

impl From<crate::runtime::OutOfMemory> for ModuleEvalError {
    fn from(_: crate::runtime::OutOfMemory) -> Self {
        Self::Runtime(Abrupt::OutOfMemory)
    }
}

impl From<GraphError<Infallible>> for ModuleEvalError {
    fn from(error: GraphError<Infallible>) -> Self {
        Self::Graph(error)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ModuleInstance {
    pub(crate) env: EnvId,
    pub(crate) namespace: ObjectId,
    pub(crate) initialize: CodeId,
    pub(crate) body: CodeId,
}

impl Agent {
    /// Instantiate and execute a host-loaded static module graph, returning the
    /// entry module's namespace. Each call creates fresh module instances.
    /// Dependencies run at most once per call, in depth-first request order.
    /// Root any handles kept across this call, just as for [`Self::evaluate`].
    pub fn evaluate_module_graph<K: Clone + Eq + Hash>(
        &mut self,
        graph: &ModuleGraph<K>,
        limits: &GraphLimits,
    ) -> Result<ObjectId, ModuleEvalError> {
        let start = self.vm.modules.len();
        let result = self.evaluate_modules(graph, limits, start);
        self.vm.modules.truncate(start);
        result
    }

    fn evaluate_modules<K: Clone + Eq + Hash>(
        &mut self,
        graph: &ModuleGraph<K>,
        limits: &GraphLimits,
        start: usize,
    ) -> Result<ObjectId, ModuleEvalError> {
        let count = graph.module_count();
        if count > limits.max_modules || graph.source_units() > limits.max_source_units {
            return Err(GraphError::GraphLimit.into());
        }
        let mut remaining = limits.max_resolution_steps;
        let mut slots = Vec::with_capacity(count);
        for index in 0..count {
            self.check_module_cancellation(limits)?;
            let (_, record) = graph.module(index).expect("loaded module");
            let ModuleCode {
                initialize,
                body,
                bindings,
            } = compile_module(self, record.program()).map_err(|error| {
                ModuleEvalError::Compile {
                    module: index,
                    error,
                }
            })?;
            let env = self.new_module_environment(&bindings)?;
            let namespace = self.new_module_namespace()?;
            slots.push(
                bindings
                    .iter()
                    .enumerate()
                    .map(|(slot, &(name, _))| {
                        (
                            name,
                            u16::try_from(slot).expect("compiler bounds slot count"),
                        )
                    })
                    .collect::<HashMap<_, _>>(),
            );
            self.vm.modules.push(ModuleInstance {
                env,
                namespace,
                initialize,
                body,
            });
        }

        // Resolve all targets before any module body can run. The static
        // graph has already validated every named import and re-export.
        for index in 0..count {
            self.check_module_cancellation(limits)?;
            let (_, record) = graph.module(index).expect("loaded module");
            let current = self.vm.modules[start + index];
            let mut imports: Vec<ImportBinding> = Vec::new();
            for entry in record.imports() {
                super::graph::spend(limits, &mut remaining)?;
                let dependency = graph
                    .dependency(index, entry.request)
                    .expect("loaded dependency");
                let local = self.heap.intern_str(&entry.local)?;
                match &entry.imported {
                    ImportName::Namespace => {
                        let object = self.vm.modules[start + dependency].namespace;
                        self.set_env_slot(current.env, slots[index][&local], Value::object(object));
                    }
                    ImportName::Binding(name) => {
                        let binding = graph.resolve(dependency, name, limits, &mut remaining)?;
                        let ResolvedExport::Binding { module, binding } = binding else {
                            unreachable!("static graph validated the import");
                        };
                        let target = self.module_binding(start, &slots, module, binding)?;
                        imports.push((slots[index][&local], target));
                    }
                }
            }
            self.set_module_imports(current.env, imports)?;

            let names = graph.collect_exported_names(index, limits, &mut remaining)?;
            let mut exports = Vec::with_capacity(names.len());
            for name in names {
                if let ResolvedExport::Binding { module, binding } =
                    graph.resolve(index, &name, limits, &mut remaining)?
                {
                    let key = self.heap.key_for(&JsStr::from_units(&name))?;
                    let binding = self.module_binding(start, &slots, module, binding)?;
                    exports.push((key, binding));
                }
            }
            self.set_namespace_exports(current.namespace, exports)?;
        }

        for index in 0..count {
            self.check_module_cancellation(limits)?;
            let module = self.vm.modules[start + index];
            self.run_in_environment(module.initialize, module.env, Value::UNDEFINED)?;
        }

        // Explicit DFS frames preserve request order and avoid Rust
        // recursion. Mark modules on entry so back edges in cycles are skipped.
        let mut stack = vec![(0, 0)];
        let mut entered = vec![false; count];
        entered[0] = true;
        while let Some((index, next)) = stack.last_mut() {
            self.check_module_cancellation(limits)?;
            if let Some(&dependency) = graph.dependencies(*index).get(*next) {
                *next += 1;
                if !std::mem::replace(&mut entered[dependency], true) {
                    stack.push((dependency, 0));
                }
            } else {
                let module = self.vm.modules[start + *index];
                self.run_in_environment(module.body, module.env, Value::UNDEFINED)?;
                stack.pop();
            }
        }
        Ok(self.vm.modules[start].namespace)
    }

    fn check_module_cancellation(&self, limits: &GraphLimits) -> Result<(), ModuleEvalError> {
        limits
            .parse
            .cancellation
            .check()
            .map_err(|_| GraphError::Cancelled)?;
        if let Some(token) = self.cancellation() {
            token.check().map_err(|_| Abrupt::Cancelled)?;
        }
        Ok(())
    }

    fn module_binding(
        &mut self,
        start: usize,
        slots: &[HashMap<crate::runtime::StringId, u16>],
        module: usize,
        binding: ResolvedBinding,
    ) -> Result<NamespaceBinding, ModuleEvalError> {
        let instance = self.vm.modules[start + module];
        let name = match binding {
            ResolvedBinding::Local(name) => self.heap.intern_str(&name)?,
            ResolvedBinding::Default => self.heap.intern_str(DEFAULT_EXPORT)?,
            ResolvedBinding::Namespace => {
                return Ok(NamespaceBinding::Namespace(instance.namespace));
            }
        };
        Ok(NamespaceBinding::Local(instance.env, slots[module][&name]))
    }
}
