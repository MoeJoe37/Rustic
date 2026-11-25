use rustic_wasm::{
    ExecutionError, ExecutionErrorKind, HostDispatch, HostTrap, LinearMemory, NumericGlobal,
};

use super::super::events::PrivateArray;
use super::instance::{LINK_ERROR, RUNTIME_ERROR, Runtime, execution_error, find};
use super::*;

// Direct numeric calls can cross instance boundaries without calling the
// JS VM. Bound those frames as well as callback-driven reentry.
const MAX_HOST_DEPTH: u8 = 32;
const MAX_RESULT_ITERATIONS: usize = 1_000_000;

#[derive(Clone, Copy)]
pub(super) struct Bindings {
    pub functions: PrivateArray,
    pub globals: PrivateArray,
    pub memories: PrivateArray,
}

#[derive(Default)]
pub(super) struct LinkedState {
    pub globals: Vec<NumericGlobal>,
    pub memories: Vec<LinearMemory>,
}

pub(super) fn read_imports(
    agent: &mut Agent,
    module: &Module,
    imports: Value,
) -> JsResult<PrivateArray> {
    if !module.imports.is_empty() && imports == Value::UNDEFINED {
        return agent.type_error("the WebAssembly module requires imports");
    }
    let resolved = PrivateArray::new(agent, &[])?;
    with_root(agent, resolved.value(), |agent| {
        for (index, import) in module.imports.iter().enumerate() {
            let name_bytes = (import.module.len() + import.name.len()) as u64;
            agent.spend_native_work(name_bytes.div_ceil(64).max(1))?;
            let key = agent.heap.key(module.text(&import.module))?;
            let namespace = agent.get_v(imports, key)?;
            let Some(namespace) = namespace.as_object() else {
                return agent.type_error("WebAssembly import namespace must be an object");
            };
            let key = agent.heap.key(module.text(&import.name))?;
            let value = agent.get(namespace, key)?;
            let accepted = match import.ty {
                ImportType::Function(_) => agent.is_callable(value),
                ImportType::Global(ty) => global::accepts_import(agent, value, ty),
                ImportType::Memory(_) => memory::find(agent, value).is_some(),
                _ => false,
            };
            if !accepted {
                return Err(wasm_error(agent, LINK_ERROR, "invalid WebAssembly import"));
            }
            resolved.set(agent, u32::try_from(index).map_err(|_| OutOfMemory)?, value)?;
        }
        Ok(resolved)
    })
}

pub(super) fn link(
    agent: &mut Agent,
    validated: &ValidatedModule<'_>,
    resolved: PrivateArray,
    bindings: Bindings,
) -> JsResult<LinkedState> {
    let declarations = validated.module().declarations();
    let mut linked = LinkedState::default();
    let globals = &mut linked.globals;
    let mut functions = 0u32;
    for (index, entry) in declarations.imports.iter().enumerate() {
        agent.check_native_work()?;
        let value = resolved.get(agent, u32::try_from(index).map_err(|_| OutOfMemory)?)?;
        match entry.value.ty {
            ImportType::Function(ty) => {
                let expected = &declarations.types[ty as usize].value;
                let size = expected.params.len() + expected.results.len();
                agent.spend_native_work(size as u64)?;
                if !expected
                    .params
                    .iter()
                    .chain(&expected.results)
                    .copied()
                    .all(is_numeric)
                {
                    return Err(wasm_error(
                        agent,
                        LINK_ERROR,
                        "only numeric function imports are supported",
                    ));
                }
                if let Some(record) = function_record(agent, value)? {
                    let (owner, function) = function_parts(agent, record)?;
                    let runtime = find(agent, owner).expect("export owner");
                    let actual = runtime
                        .function_type_at(function)
                        .expect("export signature");
                    if actual != expected {
                        return Err(wasm_error(
                            agent,
                            LINK_ERROR,
                            "WebAssembly function signature mismatch",
                        ));
                    }
                }
                bindings.functions.set(agent, functions, value)?;
                functions = functions.checked_add(1).ok_or(OutOfMemory)?;
            }
            ImportType::Global(ty) => {
                globals.try_reserve(1).map_err(|_| OutOfMemory)?;
                if let Some(global) = global::find(agent, value) {
                    let index = u32::try_from(globals.len()).map_err(|_| OutOfMemory)?;
                    bindings.globals.set(agent, index, value)?;
                    globals.push((*global).clone());
                } else {
                    // A primitive import is an independent immutable cell. The
                    // interpreter checks the declared mutability before start.
                    globals.push(NumericGlobal::new(
                        to_wasm(agent, value, ty.value_type)?,
                        false,
                    ));
                }
            }
            ImportType::Memory(_) => {
                let memory = memory::find(agent, value).expect("resolved Memory");
                let memories = &mut linked.memories;
                memories.try_reserve(1).map_err(|_| OutOfMemory)?;
                let index = u32::try_from(memories.len()).map_err(|_| OutOfMemory)?;
                bindings.memories.set(agent, index, value)?;
                memories.push((*memory).clone());
            }
            _ => unreachable!("only function, memory and numeric global bindings reach linking"),
        }
    }
    Ok(linked)
}

pub(super) fn function_record(
    agent: &mut Agent,
    function: Value,
) -> JsResult<Option<PrivateArray>> {
    if !agent.is_callable(function) {
        return Ok(None);
    }
    with_page(agent, |agent, page| {
        Ok(PrivateArray::from_value(
            agent.weak_map_get(page.wasm_calls, function),
        ))
    })
}

pub(super) fn function_parts(agent: &mut Agent, record: PrivateArray) -> JsResult<(Value, u32)> {
    let instance = record.get(agent, 0)?;
    // Private records store the original u32 index, exactly represented.
    let function = record.get(agent, 1)?.as_number().expect("export index");
    Ok((instance, function as u32))
}

pub(super) fn instance_record(agent: &mut Agent, instance: Value) -> JsResult<PrivateArray> {
    with_page(agent, |agent, page| {
        Ok(
            PrivateArray::from_value(agent.weak_map_get(page.wasm_calls, instance))
                .expect("instance bindings"),
        )
    })
}

pub(super) fn with_dispatch<T>(
    agent: &mut Agent,
    bindings: PrivateArray,
    linking: bool,
    execute: impl FnOnce(&mut HostDispatch<'_>) -> Result<T, ExecutionError>,
) -> JsResult<T> {
    let mut failure = None;
    let result = execute(&mut |index, arguments, results| {
        dispatch(agent, bindings, index, arguments, results).map_err(|error| {
            failure = Some(error);
            HostTrap
        })
    });
    match (result, failure) {
        (Err(error), _) if error.kind == ExecutionErrorKind::Cancelled => Err(Abrupt::Cancelled),
        (_, Some(failure)) => Err(failure),
        (result, None) => result.map_err(|error| execution_error(agent, error, linking)),
    }
}

pub(super) fn invoke(
    agent: &mut Agent,
    instance: Value,
    runtime: &Runtime,
    function: u32,
    arguments: &[WasmValue],
) -> JsResult<Vec<WasmValue>> {
    let record = instance_record(agent, instance)?;
    let bindings = PrivateArray::from_value(record.get(agent, 0)?).expect("function bindings");
    let token = agent.cancellation().unwrap_or_default();
    with_dispatch(agent, bindings, false, |dispatch| {
        runtime.invoke_function_with_host(function, arguments, &token, dispatch)
    })
}

fn dispatch(
    agent: &mut Agent,
    bindings: PrivateArray,
    index: usize,
    arguments: &[WasmValue],
    results: &mut [WasmValue],
) -> JsResult<()> {
    with_page(agent, |agent, page| {
        agent.check_native_work()?;
        let depth = page.wasm_call_depth.get();
        if depth >= MAX_HOST_DEPTH {
            return Err(agent.throw(
                rustic_js::runtime::ErrorKind::RangeError,
                "WebAssembly host call depth exceeded",
            ));
        }
        page.wasm_call_depth.set(depth + 1);
        let result = invoke_binding(agent, bindings, index, arguments, results);
        page.wasm_call_depth.set(depth);
        result
    })
}

fn invoke_binding(
    agent: &mut Agent,
    bindings: PrivateArray,
    index: usize,
    arguments: &[WasmValue],
    results: &mut [WasmValue],
) -> JsResult<()> {
    let function = bindings.get(agent, u32::try_from(index).map_err(|_| OutOfMemory)?)?;
    if let Some(record) = function_record(agent, function)? {
        let (instance, function) = function_parts(agent, record)?;
        let runtime = find(agent, instance).expect("imported Wasm function owner");
        let values = invoke(agent, instance, &runtime, function, arguments)?;
        results.copy_from_slice(&values);
        return Ok(());
    }

    agent.with_host_memory(std::mem::size_of_val(arguments), |agent| {
        let mut args = Vec::new();
        args.try_reserve_exact(arguments.len())
            .map_err(|_| OutOfMemory)?;
        for &value in arguments {
            agent.check_native_work()?;
            args.push(to_js(agent, value)?);
        }
        let value = agent.call(function, Value::UNDEFINED, &args)?;
        match results {
            [] => Ok(()),
            [result] => {
                *result = to_wasm(agent, value, result.value_type())?;
                Ok(())
            }
            results => with_root(agent, value, |agent| convert_results(agent, value, results)),
        }
    })
}

fn convert_results(agent: &mut Agent, value: Value, results: &mut [WasmValue]) -> JsResult<()> {
    let items = PrivateArray::new(agent, &[])?;
    with_root(agent, items.value(), |agent| {
        let key = PropertyKey::Symbol(agent.symbols.iterator);
        let Some(method) = agent.get_method(value, key)? else {
            return agent.type_error("WebAssembly callback result must be iterable");
        };
        // IteratorToList completes before checking the count or coercing any
        // item. Retain at most the expected items and keep counting to detect
        // excess values, bounded by the host work budget.
        let mut count = 0usize;
        agent.iterate_values_with_method(value, method, |agent, value| {
            agent.check_native_work()?;
            if count < results.len() {
                items.set(agent, u32::try_from(count).map_err(|_| OutOfMemory)?, value)?;
            }
            count += 1;
            if count >= MAX_RESULT_ITERATIONS {
                return Err(wasm_error(
                    agent,
                    RUNTIME_ERROR,
                    "WebAssembly callback iterator exceeds its work limit",
                ));
            }
            Ok(true)
        })?;
        if count != results.len() {
            return agent.type_error("WebAssembly callback returned the wrong number of values");
        }
        for (index, result) in results.iter_mut().enumerate() {
            let value = items.get(agent, u32::try_from(index).map_err(|_| OutOfMemory)?)?;
            *result = to_wasm(agent, value, result.value_type())?;
        }
        Ok(())
    })
}
