//! Standalone Window tables with entries traced by the JavaScript heap.

use rustic_js::runtime::ErrorKind;

use super::super::events::PrivateArray;
use super::*;

const TABLE: &str = "WebAssembly.Table";
const TABLE_LIMIT: u32 = 65_536;

struct Table {
    address: AddressType,
    functions: bool,
    capacity: u32,
}

pub(super) fn install(agent: &mut Agent, namespace: ObjectId) -> JsResult<()> {
    let proto = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, proto, TABLE)?;
    define_method(agent, proto, "grow", grow, 1)?;
    define_method(agent, proto, "get", get, 1)?;
    define_method(agent, proto, "set", set, 1)?;
    let getter = agent.new_native_function(length, "get length", 0)?;
    let key = agent.heap.key("length")?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
    )?;
    agent.install_host_interface_on(namespace, TABLE, construct, 1, proto, |_, _| Ok(()))
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("WebAssembly.Table requires new");
    };
    let Some(descriptor) = arg(args, 0).as_object() else {
        return agent.type_error("WebAssembly.Table requires a descriptor");
    };
    let key = agent.heap.key("element")?;
    let element = agent.get(descriptor, key)?;
    let element = agent.to_string(element)?;
    let element = agent.heap.string(element);
    let functions = match element.len() {
        7 if element.to_text() == "anyfunc" => true,
        9 if element.to_text() == "externref" => false,
        _ => return agent.type_error("unsupported WebAssembly.Table element type"),
    };
    let address = address_type(agent, descriptor, "invalid WebAssembly table address type")?;
    let key = agent.heap.key("initial")?;
    let initial = agent.get(descriptor, key)?;
    let initial = index(agent, initial, address)?;
    let key = agent.heap.key("maximum")?;
    let maximum = agent.get(descriptor, key)?;
    let maximum = if maximum == Value::UNDEFINED {
        address.max_table_elements()
    } else {
        index(agent, maximum, address)?
    };
    if initial > maximum {
        return Err(agent.throw(ErrorKind::RangeError, "invalid WebAssembly table limits"));
    }
    let value = entry(agent, functions, args.get(1).copied())?;
    if initial > u64::from(TABLE_LIMIT) {
        return Err(agent.throw(ErrorKind::RangeError, "cannot allocate WebAssembly table"));
    }
    let proto = agent.host_prototype_from_new_target(TABLE, target)?;
    let state = Table {
        address,
        functions,
        capacity: maximum.min(u64::from(TABLE_LIMIT)) as u32,
    };
    let object = Value::object(agent.new_host_state(TABLE, proto, state, 0)?);
    with_root(agent, object, |agent| {
        let entries = PrivateArray::new(agent, &[])?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_links, object, entries.value())
        })?;
        append(agent, entries, initial as u32, value)?;
        Ok(object)
    })
}

fn state(agent: &mut Agent, this: Value) -> JsResult<(Rc<Table>, PrivateArray)> {
    let Some(table) = this.as_object().and_then(|id| agent.host_state(id, TABLE)) else {
        return agent.type_error("invalid WebAssembly.Table receiver");
    };
    let entries = with_page(agent, |agent, page| {
        let entries = agent.weak_map_get(page.wasm_links, this);
        Ok(PrivateArray::from_value(entries).expect("branded table entries"))
    })?;
    Ok((table, entries))
}

fn index(agent: &mut Agent, value: Value, address: AddressType) -> JsResult<u64> {
    address_index(
        agent,
        value,
        address,
        "invalid WebAssembly table index or count",
    )
}

fn size_value(agent: &mut Agent, length: u32, address: AddressType) -> JsResult<Value> {
    match address {
        AddressType::I32 => Ok(Value::number(f64::from(length))),
        AddressType::I64 => agent.bigint_value(BigInt::from_u64(u64::from(length))),
    }
}

fn entry(agent: &mut Agent, functions: bool, value: Option<Value>) -> JsResult<Value> {
    let value = value.unwrap_or(if functions {
        Value::NULL
    } else {
        Value::UNDEFINED
    });
    if functions && value != Value::NULL && host::function_record(agent, value)?.is_none() {
        return agent
            .type_error("WebAssembly function table entries must be Wasm functions or null");
    }
    Ok(value)
}

fn append(agent: &mut Agent, entries: PrivateArray, count: u32, value: Value) -> JsResult<()> {
    match entries.append_repeated(agent, count, value) {
        Err(Abrupt::OutOfMemory) => Err(agent.throw(
            ErrorKind::RangeError,
            "cannot allocate WebAssembly table entries",
        )),
        result => result,
    }
}

fn check_index(agent: &mut Agent, entries: PrivateArray, index: u64) -> JsResult<u32> {
    if index >= u64::from(entries.len(agent)?) {
        return Err(agent.throw(
            ErrorKind::RangeError,
            "WebAssembly table index out of bounds",
        ));
    }
    Ok(index as u32)
}

fn length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let length = entries.len(agent)?;
    size_value(agent, length, table.address)
}

fn get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let index = index(agent, arg(args, 0), table.address)?;
    let index = check_index(agent, entries, index)?;
    entries.get(agent, index)
}

fn set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let index = index(agent, arg(args, 0), table.address)?;
    let value = entry(agent, table.functions, args.get(1).copied())?;
    let index = check_index(agent, entries, index)?;
    entries.set(agent, index, value)?;
    Ok(Value::UNDEFINED)
}

fn grow(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let count = index(agent, arg(args, 0), table.address)?;
    let value = entry(agent, table.functions, args.get(1).copied())?;
    // Converting the count can run script and grow this same table.
    let old = entries.len(agent)?;
    if count > u64::from(table.capacity - old) {
        return Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly table"));
    }
    let result = size_value(agent, old, table.address)?;
    append(agent, entries, count as u32, value)?;
    Ok(result)
}
