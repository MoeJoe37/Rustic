//! Cyclic module evaluation with top-level await (ECMA-262 §16.2.1.6).

use std::hash::Hash;

use super::{GraphLimits, ModuleEvalError, ModuleGraph, ModuleInstance};
use crate::runtime::{
    Abrupt, Agent, Class, Internal, JsResult, Object, ObjectId, Tracer, Value,
    new_intrinsic_promise, reject_promise, resolve_promise, watch_module,
};

#[derive(Clone, Debug)]
struct Evaluation {
    module: ModuleInstance,
    visit: usize,
    ancestor: usize,
    cycle_root: usize,
    on_stack: bool,
    pending: usize,
    parents: Vec<usize>,
    order: Option<usize>,
    failed: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct AsyncModuleGraph {
    entries: Vec<Evaluation>,
    completion: ObjectId,
    cancellation: rustic_base::CancellationToken,
}

impl AsyncModuleGraph {
    pub(crate) fn trace(&self, tracer: &mut impl Tracer) {
        tracer.value(Value::object(self.completion));
        for entry in &self.entries {
            entry.module.trace(tracer);
        }
    }

    pub(crate) fn footprint(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(self.entries.as_slice())
            + self
                .entries
                .iter()
                .map(|entry| std::mem::size_of_val(entry.parents.as_slice()))
                .sum::<usize>()
    }
}

impl Agent {
    fn async_modules(&mut self, state: ObjectId) -> &mut AsyncModuleGraph {
        let Some(Internal::ModuleEvaluation(state)) = self.heap.object_mut(state).internal_mut()
        else {
            unreachable!("module evaluation job owns its graph");
        };
        state
    }

    pub(super) fn run_async_modules<K: Clone + Eq + Hash>(
        &mut self,
        graph: &ModuleGraph<K>,
        limits: &GraphLimits,
        start: usize,
    ) -> Result<(ObjectId, ObjectId), ModuleEvalError> {
        let completion = new_intrinsic_promise(self)?;
        let namespace = self.vm.modules[start].namespace;
        let entries = self.vm.modules[start..]
            .iter()
            .copied()
            .enumerate()
            .map(|(index, module)| Evaluation {
                module,
                visit: usize::MAX,
                ancestor: 0,
                cycle_root: index,
                on_stack: false,
                pending: 0,
                parents: Vec::new(),
                order: None,
                failed: false,
            })
            .collect();
        let evaluation = AsyncModuleGraph {
            entries,
            completion,
            cancellation: limits.parse.cancellation.clone(),
        };
        self.heap
            .charge(std::mem::size_of::<crate::runtime::Extra>() + evaluation.footprint())?;
        let mut object = Object::new(Class::Ordinary, None);
        object.extra_mut().internal = Some(Internal::ModuleEvaluation(Box::new(evaluation)));
        let state = self.heap.alloc_object(object)?;
        self.with_rooted(Value::object(state), |agent| {
            match agent.visit_modules(state, graph, limits) {
                Ok(()) => {
                    if agent.async_modules(state).entries[0].order.is_none() {
                        resolve_promise(agent, completion, Value::UNDEFINED)?;
                    }
                }
                Err(ModuleEvalError::Runtime(Abrupt::Throw(reason))) => {
                    // A synchronous error can abort the traversal while earlier
                    // siblings are already awaiting. Their bodies may finish,
                    // but their reactions must no longer start dependents.
                    for entry in &mut agent.async_modules(state).entries {
                        entry.failed = true;
                    }
                    reject_promise(agent, completion, reason);
                }
                Err(error) => {
                    for entry in &mut agent.async_modules(state).entries {
                        entry.failed = true;
                    }
                    return Err(error);
                }
            }
            Ok((namespace, completion))
        })
    }

    fn visit_modules<K: Clone + Eq + Hash>(
        &mut self,
        state: ObjectId,
        graph: &ModuleGraph<K>,
        limits: &GraphLimits,
    ) -> Result<(), ModuleEvalError> {
        let mut dfs = vec![(0, 0)];
        let mut component = vec![0];
        let mut next_visit = 1;
        let mut next_order = 0;
        let mut remaining = limits.max_resolution_steps;
        let entry = &mut self.async_modules(state).entries[0];
        entry.visit = 0;
        entry.on_stack = true;
        // An edge whose child has not yet been entered is processed again
        // when its DFS frame returns, after the child's cycle is known.
        while let Some((index, next)) = dfs.last_mut() {
            self.check_module_cancellation(limits)?;
            let index = *index;
            if let Some(&dependency) = graph.dependencies(index).get(*next) {
                let entries = &mut self.async_modules(state).entries;
                let required = &mut entries[dependency];
                if required.visit == usize::MAX {
                    required.visit = next_visit;
                    required.ancestor = next_visit;
                    required.on_stack = true;
                    next_visit += 1;
                    dfs.push((dependency, 0));
                    component.push(dependency);
                    continue;
                }
                super::graph::spend(limits, &mut remaining)?;
                *next += 1;
                let target = if required.on_stack {
                    let ancestor = required.ancestor;
                    entries[index].ancestor = entries[index].ancestor.min(ancestor);
                    dependency
                } else {
                    required.cycle_root
                };
                if entries[target].order.is_some() {
                    self.heap.charge(std::mem::size_of::<usize>())?;
                    let entries = &mut self.async_modules(state).entries;
                    entries[target].parents.push(index);
                    entries[index].pending += 1;
                }
                continue;
            }

            let entry = &mut self.async_modules(state).entries[index];
            let pending = entry.pending;
            let has_await = entry.module.has_await;
            if has_await || pending != 0 {
                entry.order = Some(next_order);
                next_order += 1;
            }
            if pending == 0 {
                self.run_module_body(state, index)?;
            }
            let entries = &mut self.async_modules(state).entries;
            if entries[index].ancestor == entries[index].visit {
                while let Some(member) = component.pop() {
                    entries[member].on_stack = false;
                    entries[member].cycle_root = index;
                    if member == index {
                        break;
                    }
                }
            }
            dfs.pop();
        }
        Ok(())
    }

    fn run_module_body(&mut self, state: ObjectId, index: usize) -> JsResult<()> {
        let module = self.async_modules(state).entries[index].module;
        let value = self.run_in_environment(module.body, module.env, Value::UNDEFINED)?;
        if module.has_await {
            let promise = value
                .as_object()
                .expect("async module body returns a promise");
            let index = u32::try_from(index).map_err(|_| Abrupt::OutOfMemory)?;
            watch_module(self, promise, state, index)?;
        }
        Ok(())
    }

    pub(crate) fn resume_module_evaluation(
        &mut self,
        state: ObjectId,
        index: usize,
        fulfilled: bool,
        argument: Value,
    ) -> JsResult<()> {
        let graph = self.async_modules(state);
        if graph.entries[index].failed {
            return Ok(());
        }
        if graph.cancellation.is_cancelled()
            || self
                .cancellation()
                .is_some_and(|token| token.is_cancelled())
        {
            for entry in &mut self.async_modules(state).entries {
                entry.failed = true;
            }
            return Err(Abrupt::Cancelled);
        }
        if !fulfilled {
            self.reject_module_ancestors(state, index, argument);
            return Ok(());
        }
        self.complete_module(state, index)?;
        let entries = &mut self.async_modules(state).entries;
        let mut scan = vec![index];
        let mut ready = Vec::new();
        while let Some(child) = scan.pop() {
            for position in 0..entries[child].parents.len() {
                let parent = entries[child].parents[position];
                let entry = &mut entries[parent];
                if entry.failed {
                    continue;
                }
                entry.pending -= 1;
                if entry.pending == 0 {
                    ready.push((entry.order.expect("pending async dependency"), parent));
                    if !entry.module.has_await {
                        scan.push(parent);
                    }
                }
            }
        }
        ready.sort_unstable();
        for (_, parent) in ready {
            if self.async_modules(state).entries[parent].failed {
                continue;
            }
            match self.run_module_body(state, parent) {
                Ok(()) => {
                    if !self.async_modules(state).entries[parent].module.has_await {
                        self.complete_module(state, parent)?;
                    }
                }
                Err(Abrupt::Throw(error)) => self.reject_module_ancestors(state, parent, error),
                Err(other) => {
                    for entry in &mut self.async_modules(state).entries {
                        entry.failed = true;
                    }
                    return Err(other);
                }
            }
        }
        Ok(())
    }

    fn complete_module(&mut self, state: ObjectId, index: usize) -> JsResult<()> {
        let graph = self.async_modules(state);
        graph.entries[index].order = None;
        if index == 0 {
            let completion = graph.completion;
            resolve_promise(self, completion, Value::UNDEFINED)?;
        }
        Ok(())
    }

    fn reject_module_ancestors(&mut self, state: ObjectId, index: usize, reason: Value) {
        let graph = self.async_modules(state);
        let mut pending = vec![index];
        let mut reject = false;
        while let Some(child) = pending.pop() {
            let entry = &mut graph.entries[child];
            if std::mem::replace(&mut entry.failed, true) {
                continue;
            }
            entry.order = None;
            reject |= child == 0;
            pending.extend(entry.parents.iter().copied());
        }
        if reject {
            let completion = graph.completion;
            reject_promise(self, completion, reason);
        }
    }
}
