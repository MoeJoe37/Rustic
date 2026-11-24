use std::cell::Cell;

use rustic_wasm::GlobalType;

use super::*;

const GLOBAL: &str = "WebAssembly.Global";

struct Global {
    ty: GlobalType,
    storage: Storage,
}

enum Storage {
    Local(Cell<WasmValue>),
    Exported(u32),
}

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
        _ => None,
    };
    let Some(value_type) = value_type else {
        return agent.type_error("unsupported WebAssembly.Global type");
    };
    let initial = arg(args, 1);
    let initial = if initial == Value::UNDEFINED {
        match value_type {
            ValueType::I32 => WasmValue::I32(0),
            ValueType::I64 => WasmValue::I64(0),
            ValueType::F32 => WasmValue::F32(0),
            ValueType::F64 => WasmValue::F64(0),
            _ => unreachable!("numeric descriptor type"),
        }
    } else {
        to_wasm(agent, initial, value_type)?
    };
    let proto = agent.host_prototype_from_new_target(GLOBAL, target)?;
    let state = Global {
        ty: GlobalType {
            value_type,
            mutable,
        },
        storage: Storage::Local(Cell::new(initial)),
    };
    Ok(Value::object(
        agent.new_host_state(GLOBAL, proto, state, 0)?,
    ))
}

pub(super) fn export(
    agent: &mut Agent,
    instance: Value,
    index: u32,
    ty: GlobalType,
) -> JsResult<Value> {
    let proto = prototype(agent, GLOBAL)?;
    let state = Global {
        ty,
        storage: Storage::Exported(index),
    };
    let global = Value::object(agent.new_host_state(GLOBAL, proto, state, 0)?);
    with_root(agent, global, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_links, global, instance)
        })
    })?;
    Ok(global)
}

fn state(agent: &mut Agent, this: Value) -> JsResult<Rc<Global>> {
    match this.as_object().and_then(|id| agent.host_state(id, GLOBAL)) {
        Some(global) => Ok(global),
        None => agent.type_error("invalid WebAssembly.Global receiver"),
    }
}

fn owner(agent: &mut Agent, global: Value) -> JsResult<Rc<instance::Runtime>> {
    with_page(agent, |agent, page| {
        let instance = agent.weak_map_get(page.wasm_links, global);
        Ok(instance::find(agent, instance).expect("exported global owner"))
    })
}

fn get_value(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let global = state(agent, this)?;
    let value = match &global.storage {
        Storage::Local(value) => value.get(),
        Storage::Exported(index) => owner(agent, this)?
            .global_value(*index)
            .expect("numeric exported global"),
    };
    to_js(agent, value)
}

fn set_value(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let global = state(agent, this)?;
    if !global.ty.mutable {
        return agent.type_error("cannot set an immutable WebAssembly.Global");
    }
    // Conversion may run JS that accesses the same global.
    let value = to_wasm(agent, arg(args, 0), global.ty.value_type)?;
    match &global.storage {
        Storage::Local(slot) => slot.set(value),
        Storage::Exported(index) => {
            let instance = owner(agent, this)?;
            let updated = instance.set_global_value(*index, value);
            debug_assert!(updated, "checked global type and mutability");
        }
    }
    Ok(Value::UNDEFINED)
}
