use rustic_js::runtime::ErrorKind;
use rustic_wasm::{AddressType, Limits, LinearMemory, MEMORY_PAGE_BYTES};

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

fn index(agent: &mut Agent, value: Value, address: AddressType) -> JsResult<u64> {
    match address {
        AddressType::I32 => {
            let number = agent.to_number(value)?.trunc();
            if (0.0..=f64::from(u32::MAX)).contains(&number) {
                return Ok(number as u64);
            }
        }
        AddressType::I64 => {
            let number = agent.to_bigint(value)?;
            if !number.is_negative() && number.bit_length() <= 64 {
                return Ok(number.to_u64_wrapping());
            }
        }
    }
    agent.type_error("invalid WebAssembly memory page count")
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
    let address_type = if address == Value::UNDEFINED {
        AddressType::I32
    } else {
        let name = agent.to_string(address)?;
        let name = agent.heap.string(name);
        if name.len() != 3 {
            return agent.type_error("invalid WebAssembly memory address type");
        }
        match name.to_text().as_str() {
            "i32" => AddressType::I32,
            "i64" => AddressType::I64,
            _ => return agent.type_error("invalid WebAssembly memory address type"),
        }
    };
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
    let address = memory.address_type();
    let pages = index(agent, arg(args, 0), address)?;
    let old = memory.grow(pages, |work| {
        agent.spend_native_work((work as u64).div_ceil(64))
    })?;
    match (old, address) {
        (Some(pages), AddressType::I32) => Ok(Value::number(pages as f64)),
        (Some(pages), AddressType::I64) => agent.bigint_value(BigInt::from_u64(pages)),
        (None, _) => Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly memory")),
    }
}
