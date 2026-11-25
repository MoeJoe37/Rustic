use rustic_js::runtime::{ErrorKind, WasmBufferResize};
use rustic_wasm::{AddressType, Limits, LinearMemory, MEMORY_PAGE_BYTES};

use super::*;

const MEMORY: &str = "WebAssembly.Memory";
pub(super) const MEMORY_LIMIT: usize = 8 * 1024 * 1024;

pub(super) fn install(agent: &mut Agent, namespace: ObjectId) -> JsResult<()> {
    let proto = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, proto, MEMORY)?;
    define_method(agent, proto, "grow", grow, 1)?;
    define_method(agent, proto, "toFixedLengthBuffer", fixed_buffer, 0)?;
    define_method(agent, proto, "toResizableBuffer", resizable_buffer, 0)?;
    let getter = agent.new_native_function(buffer, "get buffer", 0)?;
    let key = agent.heap.key("buffer")?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
    )?;
    agent.install_host_interface_on(namespace, MEMORY, construct, 1, proto, |_, _| Ok(()))
}

fn index(agent: &mut Agent, value: Value, address: AddressType) -> JsResult<u64> {
    address_index(
        agent,
        value,
        address,
        "invalid WebAssembly memory page count",
    )
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("WebAssembly.Memory requires new");
    };
    let Some(descriptor) = arg(args, 0).as_object() else {
        return agent.type_error("WebAssembly.Memory requires a descriptor");
    };
    let address_type = address_type(agent, descriptor, "invalid WebAssembly memory address type")?;
    let key = agent.heap.key("initial")?;
    let initial = agent.get(descriptor, key)?;
    let initial = index(agent, initial, address_type)?;
    let key = agent.heap.key("maximum")?;
    let maximum = agent.get(descriptor, key)?;
    let maximum = if maximum == Value::UNDEFINED {
        None
    } else {
        Some(index(agent, maximum, address_type)?)
    };
    let key = agent.heap.key("shared")?;
    let shared = agent.get(descriptor, key)?;
    if agent.to_boolean(shared) {
        return agent.type_error("shared WebAssembly memory is unavailable");
    }
    let address_limit = address_type.max_memory_pages();
    let maximum_pages = maximum.unwrap_or(address_limit);
    let admitted = maximum_pages.min((MEMORY_LIMIT / MEMORY_PAGE_BYTES) as u64);
    if initial > admitted || maximum_pages > address_limit {
        return Err(agent.throw(ErrorKind::RangeError, "invalid WebAssembly memory limits"));
    }
    let byte_limit = admitted as usize * MEMORY_PAGE_BYTES;
    let allocation = LinearMemory::storage_bytes(byte_limit);
    let proto = agent.host_prototype_from_new_target(MEMORY, target)?;
    agent.spend_native_work(initial * (MEMORY_PAGE_BYTES as u64 / 64))?;
    let allocated = agent.with_host_memory(allocation, |_| {
        let limits = Limits {
            address_type,
            min: initial,
            max: maximum,
        };
        Ok(LinearMemory::with_limits(limits, byte_limit))
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
    buffer_with_mode(agent, this, None)
}

fn fixed_buffer(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    buffer_with_mode(agent, this, Some(false))
}

fn resizable_buffer(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    buffer_with_mode(agent, this, Some(true))
}

fn buffer_with_mode(agent: &mut Agent, this: Value, mode: Option<bool>) -> JsResult<Value> {
    let memory = state(agent, this)?;
    let current = memory.buffer();
    let resizable = mode.unwrap_or(current.is_resizable());
    let resize = if resizable {
        let Some(maximum) = memory.maximum() else {
            return agent.type_error("resizable WebAssembly memory requires a maximum");
        };
        Some(WasmBufferResize {
            maximum: maximum as f64 * MEMORY_PAGE_BYTES as f64,
            grow: resize_buffer,
        })
    } else {
        None
    };
    with_page(agent, |agent, page| {
        let cached = agent.weak_map_get(page.wasm_calls, this);
        let same_mode = resizable == current.is_resizable();
        if same_mode
            && cached
                .as_object()
                .is_some_and(|id| agent.array_buffer_bytes(id).is_some())
        {
            return Ok(cached);
        }
        let owner = this.as_object().expect("branded Memory");
        let mut install = |bytes| {
            let buffer = agent.new_wasm_array_buffer(owner, bytes, resize)?;
            with_root(agent, buffer, |agent| {
                agent.weak_map_set(page.wasm_calls, this, buffer)
            })?;
            Ok(buffer)
        };
        if same_mode {
            install(current)
        } else {
            memory.replace_buffer(resizable, install)
        }
    })
}

fn resize_buffer(agent: &mut Agent, owner: ObjectId, bytes: usize) -> JsResult<()> {
    let memory = state(agent, Value::object(owner))?;
    let pages = (bytes / MEMORY_PAGE_BYTES) as u64;
    let Some(delta) = pages
        .checked_sub(memory.pages())
        .filter(|_| bytes % MEMORY_PAGE_BYTES == 0)
    else {
        return Err(agent.throw(
            ErrorKind::RangeError,
            "WebAssembly memory resize must grow by whole pages",
        ));
    };
    grow_memory(agent, &memory, delta)?;
    Ok(())
}

fn grow_memory(agent: &mut Agent, memory: &LinearMemory, pages: u64) -> JsResult<u64> {
    memory
        .grow(pages, |work| {
            agent.spend_native_work((work as u64).div_ceil(64))
        })?
        .ok_or_else(|| agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly memory"))
}

fn grow(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let memory = state(agent, this)?;
    let address = memory.address_type();
    let pages = index(agent, arg(args, 0), address)?;
    let old = grow_memory(agent, &memory, pages)?;
    match address {
        AddressType::I32 => Ok(Value::number(old as f64)),
        AddressType::I64 => agent.bigint_value(BigInt::from_u64(old)),
    }
}
