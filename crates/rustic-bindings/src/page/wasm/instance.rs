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
    with_root(agent, Value::object(proto), |agent| {
        let resolved = host::read_imports(agent, &module, imports)?;
        start(agent, &module, resolved, proto)
    })
}

fn instantiate(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    promised(agent, |agent| {
        let source = arg(args, 0);
        let imports = import_object(agent, arg(args, 1))?;
        if source
            .as_object()
            .is_some_and(|id| agent.host_state::<Module>(id, MODULE).is_some())
        {
            let module = module(agent, source)?;
            let resolved = host::read_imports(agent, &module, imports)?;
            let context = PrivateArray::new(agent, &[source, resolved.value()])?;
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
    let resolved = PrivateArray::from_value(context.get(agent, 1)?).expect("resolved imports");
    let module = module(agent, compiled)?;
    let proto = prototype(agent, INSTANCE)?;
    start(agent, &module, resolved, proto)
}

fn source_job(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let compiled = arg(args, 0);
    let module = module(agent, compiled)?;
    let proto = prototype(agent, INSTANCE)?;
    let resolved = host::read_imports(agent, &module, agent.host_capture(0))?;
    let instance = start(agent, &module, resolved, proto)?;
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

fn start(
    agent: &mut Agent,
    module: &Module,
    resolved: PrivateArray,
    proto: ObjectId,
) -> JsResult<Value> {
    with_root(agent, resolved.value(), |agent| {
        let functions = PrivateArray::new(agent, &[])?;
        with_root(agent, functions.value(), |agent| {
            let globals = PrivateArray::new(agent, &[])?;
            with_root(agent, globals.value(), |agent| {
                let memories = PrivateArray::new(agent, &[])?;
                with_root(agent, memories.value(), |agent| {
                    let bindings = host::Bindings {
                        functions,
                        globals,
                        memories,
                    };
                    instantiate_module(agent, module, resolved, bindings, proto)
                })
            })
        })
    })
}

fn instantiate_module(
    agent: &mut Agent,
    module: &Module,
    resolved: PrivateArray,
    bindings: host::Bindings,
    proto: ObjectId,
) -> JsResult<Value> {
    let token = agent.cancellation().unwrap_or_default();
    let (runtime, allocation) = compilation_work(agent, module.bytes.len(), |agent| {
        // The branded module owns immutable bytes that passed the same validation limits.
        let validated = checked(&module.bytes, &Options::default(), &token)
            .map_err(|error| error.throw(agent))?;
        let declarations = validated.module().declarations();
        let state = host::link(agent, &validated, resolved, bindings)?;
        let imported_functions = bindings.functions.len(agent)? as usize;
        for export in &declarations.exports {
            let supported = match export.value.target {
                ExportIndex::Function(index) => {
                    match (index as usize).checked_sub(imported_functions) {
                        Some(local) => {
                            let function = &validated.module().functions()[local].value;
                            let signature = &declarations.types[function.type_index as usize].value;
                            signature
                                .params
                                .iter()
                                .chain(&signature.results)
                                .copied()
                                .all(is_numeric)
                        }
                        None => true,
                    }
                }
                ExportIndex::Global(index) => {
                    match (index as usize).checked_sub(state.globals.len()) {
                        Some(local) => is_numeric(declarations.globals[local].value.ty.value_type),
                        None => true,
                    }
                }
                ExportIndex::Memory(_) => true,
                ExportIndex::Table(index) => {
                    declarations
                        .tables
                        .get(index as usize)
                        .is_some_and(|table| {
                            matches!(
                                table.value.ty.element_type,
                                ValueType::Ref {
                                    nullable: true,
                                    heap_type: -16
                                }
                            )
                        })
                }
                _ => false,
            };
            if !supported {
                return Err(wasm_error(
                    agent,
                    LINK_ERROR,
                    "unsupported WebAssembly export type",
                ));
            }
        }
        let limits = InstanceLimits {
            memory_bytes: if declarations.memories.is_empty() {
                0
            } else {
                memory::MEMORY_LIMIT
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
            host::with_dispatch(agent, bindings.functions, true, |dispatch| {
                Instance::instantiate_owned_with_host(
                    validated,
                    limits,
                    &token,
                    &state.globals,
                    &state.memories,
                    dispatch,
                )
            })
        })?;
        Ok((runtime, allocation))
    })?;
    let object = Value::object(agent.new_host_state(INSTANCE, proto, runtime, allocation)?);
    with_root(agent, object, |agent| {
        let functions = PrivateArray::new(agent, &[])?;
        let record = PrivateArray::new(
            agent,
            &[
                bindings.functions.value(),
                functions.value(),
                bindings.globals.value(),
                bindings.memories.value(),
            ],
        )?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_calls, object, record.value())
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
    bindings: host::Bindings,
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
                    ExportIndex::Function(index) => function(agent, instance, &runtime, index)?,
                    ExportIndex::Global(index) => {
                        let imported = bindings.globals.get(agent, index)?;
                        if imported != Value::UNDEFINED {
                            imported
                        } else {
                            let state = runtime
                                .share_numeric_global(index)
                                .expect("numeric exported global");
                            global::export(agent, instance, state)?
                        }
                    }
                    ExportIndex::Memory(index) => {
                        let imported = bindings.memories.get(agent, index)?;
                        if imported != Value::UNDEFINED {
                            imported
                        } else {
                            let state = runtime.memory(index).expect("exported memory");
                            memory::export(agent, instance, state)?
                        }
                    }
                    ExportIndex::Table(index) => {
                        let limits = runtime
                            .function_table_limits(index)
                            .expect("admitted function table");
                        table::export(agent, instance, index, limits.address_type)?
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

pub(super) fn function(
    agent: &mut Agent,
    instance: Value,
    runtime: &Runtime,
    index: u32,
) -> JsResult<Value> {
    let record = host::instance_record(agent, instance)?;
    let imports = PrivateArray::from_value(record.get(agent, 0)?).expect("function imports");
    let imported = imports.get(agent, index)?;
    if host::function_record(agent, imported)?.is_some() {
        return Ok(imported);
    }
    let functions = PrivateArray::from_value(record.get(agent, 1)?).expect("function cache");
    let cached = functions.get(agent, index)?;
    if cached != Value::UNDEFINED {
        return Ok(cached);
    }
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
        functions.set(agent, index, function)?;
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
    let signature = runtime
        .function_type_at(function)
        .expect("function is present");
    agent.spend_native_work((signature.params.len() + signature.results.len()) as u64)?;
    if !signature
        .params
        .iter()
        .chain(&signature.results)
        .copied()
        .all(is_numeric)
    {
        return agent.type_error(
            "only numeric WebAssembly function signatures can be called from JavaScript",
        );
    }
    let types = &signature.params;
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
        | ExecutionErrorKind::MissingImport
            if linking =>
        {
            wasm_error(agent, LINK_ERROR, &error.to_string())
        }
        _ => wasm_error(agent, RUNTIME_ERROR, &error.to_string()),
    }
}
