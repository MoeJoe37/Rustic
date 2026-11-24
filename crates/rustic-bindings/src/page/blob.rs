//! Immutable byte data for the Window Blob interface and Fetch body mixin.

use std::rc::Rc;

use rustic_js::runtime::{Abrupt, Agent, ErrorKind, JsResult, ObjectId, Value};

use super::{
    events::{PrivateArray, define_method, with_root},
    fetch::settle,
    install_interface, prototype,
};
use crate::webidl::to_usv_string;

const BLOB: &str = "Blob";
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_PARTS: u32 = 65_536;
const TEXT: u8 = 0;
const ARRAY_BUFFER: u8 = 1;
const BYTES: u8 = 2;

pub(super) struct Data {
    pub(super) bytes: Vec<u8>,
    pub(super) media_type: String,
}

enum Part {
    Text(String),
    Buffer(ObjectId),
    Blob(Rc<Data>),
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        BLOB,
        None,
        construct,
        0,
        &[("size", "get size", size), ("type", "get type", media_type)],
        |agent, proto| {
            define_method(agent, proto, "slice", slice, 0)?;
            define_method(agent, proto, "text", read::<TEXT>, 0)?;
            define_method(agent, proto, "arrayBuffer", read::<ARRAY_BUFFER>, 0)?;
            define_method(agent, proto, "bytes", read::<BYTES>, 0)
        },
    )
}

pub(super) fn find(agent: &Agent, value: Value) -> Option<Rc<Data>> {
    agent.host_state(value.as_object()?, BLOB)
}

fn state(agent: &mut Agent, value: Value) -> JsResult<Rc<Data>> {
    match find(agent, value) {
        Some(data) => Ok(data),
        None => agent.type_error("invalid Blob receiver"),
    }
}

pub(super) fn from_bytes(agent: &mut Agent, bytes: Vec<u8>, media_type: String) -> JsResult<Value> {
    let prototype = prototype(agent, BLOB)?;
    store(agent, prototype, bytes, media_type)
}

fn store(
    agent: &mut Agent,
    prototype: ObjectId,
    bytes: Vec<u8>,
    media_type: String,
) -> JsResult<Value> {
    if bytes.len() > MAX_BYTES {
        return Err(agent.throw(ErrorKind::RangeError, "Blob exceeds byte limit"));
    }
    let footprint = bytes.capacity() + media_type.capacity();
    Ok(Value::object(agent.new_host_state(
        BLOB,
        prototype,
        Data { bytes, media_type },
        footprint,
    )?))
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("Blob requires construction");
    };
    let input = args.first().copied().unwrap_or(Value::UNDEFINED);
    let options = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    let retained = PrivateArray::new(agent, &[])?;
    with_root(agent, retained.value(), |agent| {
        let mut parts = Vec::new();
        let mut text_bytes = 0;
        if input != Value::UNDEFINED {
            agent.iterate_values(input, |agent, value| {
                if parts.len() >= MAX_PARTS as usize {
                    return Err(agent.throw(ErrorKind::RangeError, "Blob exceeds part limit"));
                }
                let part = if let Some(blob) = find(agent, value) {
                    retained.set(agent, parts.len() as u32, value)?;
                    Part::Blob(blob)
                } else if agent.is_web_buffer_source(value) {
                    let source = agent.web_unshared_buffer_source(value)?;
                    retained.set(agent, parts.len() as u32, value)?;
                    Part::Buffer(source)
                } else {
                    let text = to_usv_string(agent, value)?;
                    text_bytes += text.len();
                    check_size(agent, text_bytes)?;
                    Part::Text(text)
                };
                parts.try_reserve(1).map_err(|_| Abrupt::OutOfMemory)?;
                parts.push(part);
                Ok(true)
            })?;
        }
        let (native, content_type) = read_options(agent, options)?;
        let prototype = agent.host_prototype_from_new_target(BLOB, target)?;
        let bytes = collect(agent, parts, native)?;
        store(agent, prototype, bytes, content_type)
    })
}

fn read_options(agent: &mut Agent, options: Value) -> JsResult<(bool, String)> {
    if options.is_nullish() {
        return Ok((false, String::new()));
    }
    let Some(options) = options.as_object() else {
        return agent.type_error("Blob options must be a dictionary");
    };
    let endings = agent.heap.key("endings")?;
    let endings = agent.get(options, endings)?;
    let native = if endings == Value::UNDEFINED {
        false
    } else {
        let endings = to_usv_string(agent, endings)?;
        match endings.as_str() {
            "transparent" => false,
            "native" => true,
            _ => return agent.type_error("invalid Blob endings"),
        }
    };
    let content_type = agent.heap.key("type")?;
    let content_type = agent.get(options, content_type)?;
    Ok((native, normalize_type(agent, content_type)?))
}

fn normalize_type(agent: &mut Agent, input: Value) -> JsResult<String> {
    if input == Value::UNDEFINED {
        return Ok(String::new());
    }
    let mut text = to_usv_string(agent, input)?;
    if text.bytes().any(|byte| !(b' '..=b'~').contains(&byte)) {
        text.clear();
    }
    text.make_ascii_lowercase();
    Ok(text)
}

fn check_size(agent: &mut Agent, size: usize) -> JsResult<()> {
    if size > MAX_BYTES {
        return Err(agent.throw(ErrorKind::RangeError, "Blob exceeds byte limit"));
    }
    Ok(())
}

fn collect(agent: &mut Agent, parts: Vec<Part>, native: bool) -> JsResult<Vec<u8>> {
    let mut total = 0_usize;
    for part in &parts {
        agent.check_native_work()?;
        let length = match part {
            Part::Blob(data) => data.bytes.len(),
            Part::Buffer(source) => agent.web_buffer_bytes(*source).len(),
            Part::Text(text) if native => native_bytes(text.as_bytes()).count(),
            Part::Text(text) => text.len(),
        };
        total = total.checked_add(length).ok_or(Abrupt::OutOfMemory)?;
        check_size(agent, total)?;
    }
    agent.with_host_memory(total, |agent| {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(total)
            .map_err(|_| Abrupt::OutOfMemory)?;
        for part in parts {
            match part {
                Part::Blob(data) => bytes.extend_from_slice(&data.bytes),
                Part::Buffer(source) => bytes.extend_from_slice(&agent.web_buffer_bytes(source)),
                Part::Text(text) if native => bytes.extend(native_bytes(text.as_bytes())),
                Part::Text(text) => bytes.extend(text.into_bytes()),
            }
        }
        Ok(bytes)
    })
}

fn native_bytes(bytes: &[u8]) -> impl Iterator<Item = u8> + '_ {
    const NEWLINE: &[u8] = if cfg!(windows) { b"\r\n" } else { b"\n" };
    bytes
        .iter()
        .enumerate()
        .flat_map(|(index, byte)| match *byte {
            b'\n' if index != 0 && bytes[index - 1] == b'\r' => &[],
            b'\r' | b'\n' => NEWLINE,
            _ => std::slice::from_ref(byte),
        })
        .copied()
}

fn size(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(state(agent, this)?.bytes.len() as f64))
}

fn media_type(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let data = state(agent, this)?;
    agent.string(&data.media_type)
}

fn slice_index(agent: &mut Agent, input: Value, length: usize, default: usize) -> JsResult<usize> {
    if input == Value::UNDEFINED {
        return Ok(default);
    }
    let number = agent.to_number(input)?;
    // [Clamp] rounds ties to even. Bounds outside the Blob range also
    // cover the long long precision limit and infinities.
    let mut number = if number.is_nan() {
        0.0
    } else {
        number.round_ties_even()
    };
    if number < 0.0 {
        number += length as f64;
    }
    Ok(number.clamp(0.0, length as f64) as usize)
}

fn slice(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let data = state(agent, this)?;
    let arg = |index| args.get(index).copied().unwrap_or(Value::UNDEFINED);
    let start = slice_index(agent, arg(0), data.bytes.len(), 0)?;
    let end = slice_index(agent, arg(1), data.bytes.len(), data.bytes.len())?.max(start);
    let content_type = normalize_type(agent, arg(2))?;
    let bytes = agent.with_host_memory(end - start, |_| {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(end - start)
            .map_err(|_| Abrupt::OutOfMemory)?;
        bytes.extend_from_slice(&data.bytes[start..end]);
        Ok(bytes)
    })?;
    from_bytes(agent, bytes, content_type)
}

fn read<const FORMAT: u8>(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let promise = agent.new_host_promise()?;
    let outcome = (|| {
        let data = state(agent, this)?;
        let owner = this.as_object().expect("Blob receiver");
        match FORMAT {
            TEXT => agent.string(&rustic_encoding::utf8_decode(&data.bytes)),
            ARRAY_BUFFER => agent.new_web_array_buffer(owner, &data.bytes),
            _ => agent.new_web_uint8_array(owner, &data.bytes),
        }
    })();
    settle(agent, promise, outcome)?;
    Ok(Value::object(promise))
}
