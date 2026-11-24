use std::collections::HashMap;

use rustic_wasm::{ExecutionError, ExecutionErrorKind, Instance, InstanceLimits};

use super::super::events::PrivateArray;
use super::*;

const INSTANCE: &str = "WebAssembly.Instance";
pub(super) const LINK_ERROR: &str = "WebAssembly.LinkError";
pub(super) const RUNTIME_ERROR: &str = "WebAssembly.RuntimeError";

pub(super) type Runtime = Instance<'static, 'static>;

pub(super) fn install(agent: &mut Agent, namespace: ObjectId) -> JsResult<()> {
    let proto = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, proto, INSTANCE)?;
    let getter = agent.new_native_function(exports, "get exports", 0)?;
    let key = agent.heap.key("exports")?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
    )?;
    agent.install_host_interface_on(namespace, INSTANCE, construct, 1, proto, |_, _| Ok(()))?;
    define_method(agent, namespace, "instantiate", instantiate, 1)?;
    define_method(
        agent,
        namespace,
        "instantiateStreaming",
        instantiate_streaming,
        1,
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("WebAssembly.Instance requires new");
    };
    let module = module(agent, arg(args, 0))?;
    let imports = import_object(agent, arg(args, 1))?;
    let proto = agent.host_prototype_from_new_target(INSTANCE, target)?;
    start(agent, &module, imports, proto)
}

fn instantiate(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    promised(agent, |agent| {
        let source = arg(args, 0);
        let imports = import_object(agent, arg(args, 1))?;
        if source
            .as_object()
            .is_some_and(|id| agent.host_state::<Module>(id, MODULE).is_some())
        {
            let context = PrivateArray::new(agent, &[source, imports])?;
            agent.react_to_host_value(Value::UNDEFINED, instance_job, context.value())
        } else {
            let compiled = compile(agent, Value::UNDEFINED, &[source, arg(args, 2)])?;
            agent.react_to_host_value(compiled, source_job, imports)
        }
    })
}

fn instantiate_streaming(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    promised(agent, |agent| {
        let imports = import_object(agent, arg(args, 1))?;
        let compiled = compile_streaming(agent, Value::UNDEFINED, &[arg(args, 0), arg(args, 2)])?;
        agent.react_to_host_value(compiled, source_job, imports)
    })
}

fn import_object(agent: &mut Agent, imports: Value) -> JsResult<Value> {
    if imports == Value::UNDEFINED || imports.as_object().is_some() {
        Ok(imports)
    } else {
        agent.type_error("WebAssembly imports must be an object")
    }
}

fn instance_job(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let context =
        PrivateArray::from_value(agent.host_capture(0)).expect("private instance context");
    let compiled = context.get(agent, 0)?;
    let imports = context.get(agent, 1)?;
    let module = module(agent, compiled)?;
    let proto = prototype(agent, INSTANCE)?;
    start(agent, &module, imports, proto)
}

fn source_job(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let compiled = arg(args, 0);
    let module = module(agent, compiled)?;
    let proto = prototype(agent, INSTANCE)?;
    let instance = start(agent, &module, agent.host_capture(0), proto)?;
    let result = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    for (name, value) in [("module", compiled), ("instance", instance)] {
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            result,
            key,
            PropertyDescriptor::data_with(value, true, true, true),
        )?;
    }
    Ok(Value::object(result))
}

fn start(agent: &mut Agent, module: &Module, imports: Value, proto: ObjectId) -> JsResult<Value> {
    if !module.imports.is_empty() && imports == Value::UNDEFINED {
        return agent.type_error("the WebAssembly module requires imports");
    }
    let bindings = PrivateArray::new(agent, &[])?;
    with_root(agent, bindings.value(), |agent| {
        with_root(agent, Value::object(proto), |agent| {
            instantiate_module(agent, module, imports, bindings, proto)
        })
    })
}

fn instantiate_module(
    agent: &mut Agent,
    module: &Module,
    imports: Value,
    bindings: PrivateArray,
    proto: ObjectId,
) -> JsResult<Value> {
    let token = agent.cancellation().unwrap_or_default();
    let (runtime, allocation) = compilation_work(agent, module.bytes.len(), |agent| {
        // The branded module owns immutable bytes that passed the same validation limits.
        let validated = checked(&module.bytes, &Options::default(), &token)
            .map_err(|error| error.throw(agent))?;
        let declarations = validated.module().declarations();
        host::link(agent, &validated, imports, bindings)?;
        let imported = declarations.imports.len();
        for export in &declarations.exports {
            let supported = match export.value.target {
                ExportIndex::Function(index) => {
                    let index = index as usize;
                    let type_index = match declarations.imports.get(index) {
                        Some(entry) => match entry.value.ty {
                            ImportType::Function(ty) => ty,
                            _ => unreachable!("only function imports were linked"),
                        },
                        None => {
                            validated.module().functions()[index - imported]
                                .value
                                .type_index
                        }
                    };
                    let signature = &declarations.types[type_index as usize].value;
                    signature
                        .params
                        .iter()
                        .chain(&signature.results)
                        .copied()
                        .all(is_numeric)
                }
                ExportIndex::Global(index) => {
                    is_numeric(declarations.globals[index as usize].value.ty.value_type)
                }
                _ => false,
            };
            if !supported {
                return Err(wasm_error(
                    agent,
                    LINK_ERROR,
                    "this WebAssembly instance supports numeric function and global exports",
                ));
            }
        }
        let limits = InstanceLimits {
            memory_bytes: if declarations.memories.is_empty() {
                0
            } else {
                8 * 1024 * 1024
            },
            table_elements: if declarations.tables.is_empty() && declarations.elements.is_empty() {
                0
            } else {
                65_536
            },
            ..InstanceLimits::default()
        };
        // Reserve the instance's possible guest growth against the JS heap.
        let allocation = compilation_scratch(module.bytes.len())
            + limits.memory_bytes
            + limits.table_elements * 2 * size_of::<usize>();
        let runtime = agent.with_host_memory(allocation, |agent| {
            host::with_dispatch(agent, bindings, true, |dispatch| {
                Instance::instantiate_owned_with_host(validated, limits, &token, dispatch)
            })
        })?;
        Ok((runtime, allocation))
    })?;
    let object = Value::object(agent.new_host_state(INSTANCE, proto, runtime, allocation)?);
    with_root(agent, object, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_calls, object, bindings.value())
        })?;
        let exports = Value::object(agent.new_object_with(None)?);
        with_root(agent, exports, |agent| {
            populate_exports(
                agent,
                object,
                exports.as_object().expect("exports object"),
                module,
                bindings,
            )?;
            with_page(agent, |agent, page| {
                agent.weak_map_set(page.wasm_links, object, exports)
            })
        })?;
        Ok(object)
    })
}

fn populate_exports(
    agent: &mut Agent,
    instance: Value,
    exports: ObjectId,
    module: &Module,
    bindings: PrivateArray,
) -> JsResult<()> {
    let runtime = find(agent, instance).expect("new WebAssembly instance");
    let mut entries = HashMap::new();
    for item in &module.exports {
        agent.check_native_work()?;
        let name = module.text(&item.name);
        let value = match entries.get(&item.target) {
            Some(&value) => value,
            None => {
                entries.try_reserve(1).map_err(|_| OutOfMemory)?;
                let value = match item.target {
                    ExportIndex::Function(index) => {
                        let imported = bindings.get(agent, index)?;
                        if host::function_record(agent, imported)?.is_some() {
                            imported
                        } else {
                            function_export(agent, instance, &runtime, index)?
                        }
                    }
                    ExportIndex::Global(index) => {
                        let ty = runtime.global_type(index).expect("exported global");
                        global::export(agent, instance, index, ty)?
                    }
                    _ => unreachable!("admitted exports checked before instantiation"),
                };
                entries.insert(item.target, value);
                value
            }
        };
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            exports,
            key,
            PropertyDescriptor::data_with(value, false, true, false),
        )?;
    }
    agent.prevent_extensions(exports)?;
    Ok(())
}

fn function_export(
    agent: &mut Agent,
    instance: Value,
    runtime: &Runtime,
    index: u32,
) -> JsResult<Value> {
    let size = runtime
        .function_type_at(index)
        .expect("export signature")
        .params
        .len();
    let size = u32::try_from(size).map_err(|_| OutOfMemory)?;
    let record = PrivateArray::new(agent, &[instance, Value::number(f64::from(index))])?;
    with_root(agent, record.value(), |agent| {
        let function = Value::object(agent.new_host_closure(
            call,
            &index.to_string(),
            size,
            &[record.value()],
        )?);
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_calls, function, record.value())
        })?;
        Ok(function)
    })
}

pub(super) fn find(agent: &Agent, value: Value) -> Option<Rc<Runtime>> {
    agent.host_state(value.as_object()?, INSTANCE)
}

fn exports(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    if find(agent, this).is_none() {
        return agent.type_error("invalid WebAssembly.Instance receiver");
    }
    with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.wasm_links, this))
    })
}

fn call(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let record = PrivateArray::from_value(agent.host_capture(0)).expect("export function record");
    let (instance, function) = host::function_parts(agent, record)?;
    let runtime = find(agent, instance).expect("export function owner");
    let types = &runtime
        .function_type_at(function)
        .expect("export is present")
        .params;
    let mut input = Vec::new();
    input
        .try_reserve_exact(types.len())
        .map_err(|_| OutOfMemory)?;
    for (index, &ty) in types.iter().enumerate() {
        agent.check_native_work()?;
        input.push(to_wasm(agent, arg(args, index), ty)?);
    }
    let results = host::invoke(agent, instance, &runtime, function, &input)?;
    match results.as_slice() {
        [] => Ok(Value::UNDEFINED),
        &[one] => to_js(agent, one),
        _ => sequence(agent, results.into_iter(), to_js),
    }
}

pub(super) fn execution_error(agent: &mut Agent, error: ExecutionError, linking: bool) -> Abrupt {
    match error.kind {
        ExecutionErrorKind::Cancelled => Abrupt::Cancelled,
        ExecutionErrorKind::UnsupportedModule
        | ExecutionErrorKind::UnsupportedInstruction
        | ExecutionErrorKind::ImportTypeMismatch
            if linking =>
        {
            wasm_error(agent, LINK_ERROR, &error.to_string())
        }
        _ => wasm_error(agent, RUNTIME_ERROR, &error.to_string()),
    }
}
