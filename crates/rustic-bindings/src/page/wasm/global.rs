use rustic_wasm::{GlobalType, HostValue, Reference, SharedGlobal};

use super::*;

const GLOBAL: &str = "WebAssembly.Global";

pub(super) fn install(agent: &mut Agent, namespace: ObjectId) -> JsResult<()> {
    let proto = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, proto, GLOBAL)?;
    define_method(agent, proto, "valueOf", get_value, 0)?;
    let getter = agent.new_native_function(get_value, "get value", 0)?;
    let setter = agent.new_native_function(set_value, "set value", 1)?;
    let key = agent.heap.key("value")?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::object(setter), true, true),
    )?;
    agent.install_host_interface_on(namespace, GLOBAL, construct, 1, proto, |_, _| Ok(()))
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("WebAssembly.Global requires new");
    };
    let Some(descriptor) = arg(args, 0).as_object() else {
        return agent.type_error("WebAssembly.Global requires a descriptor");
    };
    let mutable = agent.heap.key("mutable")?;
    let mutable = agent.get(descriptor, mutable)?;
    let mutable = agent.to_boolean(mutable);
    let value = agent.heap.key("value")?;
    let value = agent.get(descriptor, value)?;
    let value = agent.to_string(value)?;
    let name = agent.heap.string(value);
    let value_type = match name.len() {
        3 => match name.to_text().as_str() {
            "i32" => Some(ValueType::I32),
            "i64" => Some(ValueType::I64),
            "f32" => Some(ValueType::F32),
            "f64" => Some(ValueType::F64),
            _ => None,
        },
        7 if name.to_text() == "anyfunc" => Some(ValueType::Ref {
            nullable: true,
            heap_type: -16,
        }),
        9 if name.to_text() == "externref" => Some(ValueType::Ref {
            nullable: true,
            heap_type: -17,
        }),
        _ => None,
    };
    let Some(value_type) = value_type else {
        return agent.type_error("unsupported WebAssembly.Global type");
    };
    let initial = match (value_type, args.get(1).copied()) {
        (ValueType::Ref { heap_type: -16, .. }, None) => HostValue::Ref(Reference::NullFunction),
        (ValueType::I32, None | Some(Value::UNDEFINED)) => HostValue::Number(WasmValue::I32(0)),
        (ValueType::I64, None | Some(Value::UNDEFINED)) => HostValue::Number(WasmValue::I64(0)),
        (ValueType::F32, None | Some(Value::UNDEFINED)) => HostValue::Number(WasmValue::F32(0)),
        (ValueType::F64, None | Some(Value::UNDEFINED)) => HostValue::Number(WasmValue::F64(0)),
        (ty, value) => host::to_host_value(agent, value.unwrap_or(Value::UNDEFINED), ty)?,
    };
    let proto = agent.host_prototype_from_new_target(GLOBAL, target)?;
    let state = SharedGlobal::new(initial, mutable);
    Ok(Value::object(agent.new_traced_host_state(
        GLOBAL,
        proto,
        state,
        SharedGlobal::storage_bytes(),
        Some(trace_global),
    )?))
}

pub(super) fn export(agent: &mut Agent, instance: Value, state: SharedGlobal) -> JsResult<Value> {
    let proto = prototype(agent, GLOBAL)?;
    let global =
        Value::object(agent.new_traced_host_state(GLOBAL, proto, state, 0, Some(trace_global))?);
    with_root(agent, global, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_links, global, instance)
        })
    })?;
    Ok(global)
}

fn trace_global(state: &dyn std::any::Any, mark: &mut dyn FnMut(Value)) {
    let global = state.downcast_ref::<SharedGlobal>().expect("global state");
    if let Some(root) = global.get().host_root() {
        mark(instance::root_value(root));
    }
}

pub(super) fn find(agent: &Agent, value: Value) -> Option<Rc<SharedGlobal>> {
    agent.host_state(value.as_object()?, GLOBAL)
}

pub(super) fn accepts_import(agent: &mut Agent, value: Value, ty: GlobalType) -> JsResult<bool> {
    if !HostValue::supports(ty.value_type) {
        return Ok(false);
    }
    if find(agent, value).is_some() {
        return Ok(true);
    }
    if ty.mutable {
        return Ok(false);
    }
    Ok(match ty.value_type {
        ValueType::I64 => agent.type_of(value) == "bigint",
        ValueType::Ref { heap_type: -16, .. } => {
            value == Value::NULL || host::function_record(agent, value)?.is_some()
        }
        ValueType::Ref { heap_type: -17, .. } => true,
        _ => value.as_number().is_some(),
    })
}

fn state(agent: &mut Agent, this: Value) -> JsResult<Rc<SharedGlobal>> {
    match find(agent, this) {
        Some(global) => Ok(global),
        None => agent.type_error("invalid WebAssembly.Global receiver"),
    }
}

fn get_value(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let value = state(agent, this)?.get();
    host::to_js_value(agent, value)
}

fn set_value(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let global = state(agent, this)?;
    let ty = global.ty();
    if !ty.mutable {
        return agent.type_error("cannot set an immutable WebAssembly.Global");
    }
    let value = host::to_host_value(agent, arg(args, 0), ty.value_type)?;
    let updated = global.set(value);
    debug_assert!(updated, "checked global conversion");
    Ok(Value::UNDEFINED)
}
