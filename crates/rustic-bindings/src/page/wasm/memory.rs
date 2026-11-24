use rustic_js::runtime::ErrorKind;
use rustic_wasm::{LinearMemory, MEMORY_PAGE_BYTES, MEMORY32_MAX_PAGES};

use super::*;

const MEMORY: &str = "WebAssembly.Memory";
pub(super) const MEMORY_LIMIT: usize = 8 * 1024 * 1024;

pub(super) fn install(agent: &mut Agent, namespace: ObjectId) -> JsResult<()> {
    let proto = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, proto, MEMORY)?;
    define_method(agent, proto, "grow", grow, 1)?;
    define_method(agent, proto, "toFixedLengthBuffer", buffer, 0)?;
    let getter = agent.new_native_function(buffer, "get buffer", 0)?;
    let key = agent.heap.key("buffer")?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
    )?;
    agent.install_host_interface_on(namespace, MEMORY, construct, 1, proto, |_, _| Ok(()))
}

fn index(agent: &mut Agent, value: Value) -> JsResult<u32> {
    let number = agent.to_number(value)?.trunc();
    if !(0.0..=f64::from(u32::MAX)).contains(&number) {
        return agent.type_error("invalid WebAssembly memory page count");
    }
    Ok(number as u32)
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("WebAssembly.Memory requires new");
    };
    let Some(descriptor) = arg(args, 0).as_object() else {
        return agent.type_error("WebAssembly.Memory requires a descriptor");
    };
    let key = agent.heap.key("address")?;
    let address = agent.get(descriptor, key)?;
    if address != Value::UNDEFINED {
        let name = agent.to_string(address)?;
        if agent.heap.string(name).len() != 3 || agent.heap.string(name).to_text() != "i32" {
            return agent.type_error("only 32-bit WebAssembly memory is supported");
        }
    }
    let key = agent.heap.key("initial")?;
    let initial = agent.get(descriptor, key)?;
    let initial = index(agent, initial)?;
    let key = agent.heap.key("maximum")?;
    let maximum = agent.get(descriptor, key)?;
    let maximum = if maximum == Value::UNDEFINED {
        None
    } else {
        Some(index(agent, maximum)?)
    };
    let key = agent.heap.key("shared")?;
    let shared = agent.get(descriptor, key)?;
    if agent.to_boolean(shared) {
        return agent.type_error("shared WebAssembly memory is unavailable");
    }
    let maximum_pages = maximum.unwrap_or(MEMORY32_MAX_PAGES);
    let admitted = maximum_pages.min((MEMORY_LIMIT / MEMORY_PAGE_BYTES) as u32);
    if initial > admitted || maximum_pages > MEMORY32_MAX_PAGES {
        return Err(agent.throw(ErrorKind::RangeError, "invalid WebAssembly memory limits"));
    }
    let byte_limit = admitted as usize * MEMORY_PAGE_BYTES;
    let allocation = LinearMemory::storage_bytes(byte_limit);
    let proto = agent.host_prototype_from_new_target(MEMORY, target)?;
    agent.spend_native_work(u64::from(initial) * (MEMORY_PAGE_BYTES as u64 / 64))?;
    let allocated = agent.with_host_memory(allocation, |_| {
        Ok(LinearMemory::new(initial, maximum, byte_limit))
    });
    let memory = match allocated {
        Ok(Some(memory)) => memory,
        Ok(None) | Err(Abrupt::OutOfMemory) => {
            return Err(agent.throw(ErrorKind::RangeError, "cannot allocate WebAssembly memory"));
        }
        Err(error) => return Err(error),
    };
    Ok(Value::object(
        agent.new_host_state(MEMORY, proto, memory, allocation)?,
    ))
}

pub(super) fn find(agent: &Agent, value: Value) -> Option<Rc<LinearMemory>> {
    agent.host_state(value.as_object()?, MEMORY)
}

fn state(agent: &mut Agent, this: Value) -> JsResult<Rc<LinearMemory>> {
    match find(agent, this) {
        Some(memory) => Ok(memory),
        None => agent.type_error("invalid WebAssembly.Memory receiver"),
    }
}

pub(super) fn export(agent: &mut Agent, instance: Value, memory: LinearMemory) -> JsResult<Value> {
    let proto = prototype(agent, MEMORY)?;
    let wrapper = Value::object(agent.new_host_state(MEMORY, proto, memory, 0)?);
    with_root(agent, wrapper, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_links, wrapper, instance)
        })
    })?;
    Ok(wrapper)
}

fn buffer(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let memory = state(agent, this)?;
    with_page(agent, |agent, page| {
        let cached = agent.weak_map_get(page.wasm_calls, this);
        if cached
            .as_object()
            .is_some_and(|id| agent.array_buffer_bytes(id).is_some())
        {
            return Ok(cached);
        }
        let owner = this.as_object().expect("branded Memory");
        let buffer = agent.new_wasm_array_buffer(owner, memory.buffer())?;
        with_root(agent, buffer, |agent| {
            agent.weak_map_set(page.wasm_calls, this, buffer)
        })?;
        Ok(buffer)
    })
}

fn grow(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let memory = state(agent, this)?;
    let pages = index(agent, arg(args, 0))?;
    let old = memory.grow(pages, |work| {
        agent.spend_native_work((work as u64).div_ceil(64))
    })?;
    match old {
        Some(pages) => Ok(Value::number(f64::from(pages))),
        None => Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly memory")),
    }
}
