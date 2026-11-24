use rustic_wasm::{ExecutionError, ExecutionErrorKind, HostDispatch, HostTrap};

use super::super::events::PrivateArray;
use super::instance::{LINK_ERROR, RUNTIME_ERROR, Runtime, execution_error, find};
use super::*;

// Direct numeric calls can cross instance boundaries without calling the
// JS VM. Bound those frames as well as callback-driven reentry.
const MAX_HOST_DEPTH: u8 = 32;
const MAX_RESULT_ITERATIONS: usize = 1_000_000;

pub(super) fn link(
    agent: &mut Agent,
    validated: &ValidatedModule<'_>,
    imports: Value,
    bindings: PrivateArray,
) -> JsResult<()> {
    let declarations = validated.module().declarations();
    for (index, entry) in declarations.imports.iter().enumerate() {
        agent.check_native_work()?;
        let import = &entry.value;
        let ImportType::Function(ty) = import.ty else {
            return Err(wasm_error(
                agent,
                LINK_ERROR,
                "only function imports are supported",
            ));
        };
        let expected = &declarations.types[ty as usize].value;
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
        let key = agent.heap.key(import.module)?;
        let namespace = agent.get_v(imports, key)?;
        let Some(namespace) = namespace.as_object() else {
            return agent.type_error("WebAssembly import namespace must be an object");
        };
        let key = agent.heap.key(import.name)?;
        let function = agent.get(namespace, key)?;
        if !agent.is_callable(function) {
            return Err(wasm_error(
                agent,
                LINK_ERROR,
                "WebAssembly function import is not callable",
            ));
        }
        if let Some(record) = function_record(agent, function)? {
            let (owner, function) = function_parts(agent, record)?;
            let runtime = find(agent, owner).expect("export owner");
            let actual = runtime
                .function_type_at(function)
                .expect("export signature");
            agent.spend_native_work((expected.params.len() + expected.results.len()) as u64)?;
            if actual != expected {
                return Err(wasm_error(
                    agent,
                    LINK_ERROR,
                    "WebAssembly function signature mismatch",
                ));
            }
        }
        let index = u32::try_from(index).map_err(|_| OutOfMemory)?;
        bindings.set(agent, index, function)?;
    }
    Ok(())
}

pub(super) fn function_record(
    agent: &mut Agent,
    function: Value,
) -> JsResult<Option<PrivateArray>> {
    if function.as_object().is_none() {
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
    let bindings = with_page(agent, |agent, page| {
        Ok(
            PrivateArray::from_value(agent.weak_map_get(page.wasm_calls, instance))
                .expect("instance bindings"),
        )
    })?;
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
