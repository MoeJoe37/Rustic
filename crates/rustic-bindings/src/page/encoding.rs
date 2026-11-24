//! The Encoding Standard's synchronous Web IDL interfaces.

use std::cell::RefCell;
use std::rc::Rc;

use rustic_encoding::{Decoder, DecoderErrorMode, Encoding};
use rustic_js::runtime::{
    Abrupt, Agent, ErrorKind, JsResult, ObjectId, PropertyDescriptor, StringId, Value,
};

use super::{events::define_method, install_interface, required_arg};
use crate::webidl::coerce_string;

const ENCODER: &str = "TextEncoder";
const DECODER: &str = "TextDecoder";
const WORK_CHUNK: usize = 4096;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        ENCODER,
        None,
        encoder_constructor,
        0,
        &[("encoding", "get encoding", encoder_encoding)],
        |agent, prototype| {
            define_method(agent, prototype, "encode", encode, 0)?;
            define_method(agent, prototype, "encodeInto", encode_into, 2)
        },
    )?;
    install_interface(
        agent,
        DECODER,
        None,
        decoder_constructor,
        0,
        &[
            ("encoding", "get encoding", decoder_encoding),
            ("fatal", "get fatal", decoder_fatal),
            ("ignoreBOM", "get ignoreBOM", decoder_ignore_bom),
        ],
        |agent, prototype| define_method(agent, prototype, "decode", decode, 0),
    )
}

fn encoder_constructor(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("TextEncoder requires construction");
    };
    let prototype = agent.host_prototype_from_new_target(ENCODER, target)?;
    Ok(Value::object(agent.new_host_state(
        ENCODER,
        prototype,
        (),
        0,
    )?))
}

fn encoder_object(agent: &mut Agent, this: Value) -> JsResult<ObjectId> {
    if let Some(object) = this.as_object() {
        if agent.host_state::<()>(object, ENCODER).is_some() {
            return Ok(object);
        }
    }
    agent.type_error("invalid TextEncoder receiver")
}

fn encoder_encoding(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    encoder_object(agent, this)?;
    agent.string("utf-8")
}

fn encode(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let owner = encoder_object(agent, this)?;
    let input = args.first().copied().unwrap_or(Value::UNDEFINED);
    if input == Value::UNDEFINED {
        return agent.new_web_uint8_array(owner, &[]);
    }
    let input = flattened_string(agent, input)?;
    let (read, written) = utf8_prefix(agent, input, usize::MAX)?;
    with_utf8(agent, input, read, written, |agent, bytes| {
        agent.new_web_uint8_array(owner, bytes)
    })
}

fn encode_into(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    encoder_object(agent, this)?;
    let destination = required_arg(agent, args, 1)?;
    let input = flattened_string(agent, args[0])?;
    let destination = agent.web_uint8_array(destination)?;
    let capacity = agent.web_buffer_bytes(destination).len();
    let (read, written) = utf8_prefix(agent, input, capacity)?;
    with_utf8(agent, input, read, written, |agent, bytes| {
        agent.write_web_uint8_array(destination, bytes)
    })?;

    let result = agent.new_object()?;
    for (name, count) in [("read", read), ("written", written)] {
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            result,
            key,
            PropertyDescriptor::data(Value::number(count as f64)),
        )?;
    }
    Ok(Value::object(result))
}

fn flattened_string(agent: &mut Agent, value: Value) -> JsResult<StringId> {
    let id = coerce_string(agent, value)?;
    agent.heap.flatten(id)?;
    Ok(id)
}

/// Count complete scalar values that fit, in UTF-16 units and UTF-8 bytes.
fn utf8_prefix(agent: &mut Agent, input: StringId, capacity: usize) -> JsResult<(usize, usize)> {
    // One byte per code unit is the minimum. At most one extra scalar
    // (two units) needs to be examined when the destination is full.
    let work = agent
        .heap
        .string(input)
        .len()
        .min(capacity.saturating_add(2));
    for _ in (0..work).step_by(WORK_CHUNK) {
        agent.check_native_work()?;
    }
    let mut read = 0;
    let mut written = 0_usize;
    for ch in char::decode_utf16(agent.heap.string(input).units()) {
        let ch = ch.unwrap_or(char::REPLACEMENT_CHARACTER);
        let next = written
            .checked_add(ch.len_utf8())
            .ok_or(Abrupt::OutOfMemory)?;
        if next > capacity {
            break;
        }
        written = next;
        read += ch.len_utf16();
    }
    Ok((read, written))
}

fn with_utf8<T>(
    agent: &mut Agent,
    input: StringId,
    read: usize,
    written: usize,
    finish: impl FnOnce(&mut Agent, &[u8]) -> JsResult<T>,
) -> JsResult<T> {
    agent.with_host_memory(written, |agent| {
        let mut text = String::new();
        text.try_reserve_exact(written)
            .map_err(|_| Abrupt::OutOfMemory)?;
        text.extend(
            char::decode_utf16(agent.heap.string(input).units().take(read))
                .map(|ch| ch.unwrap_or(char::REPLACEMENT_CHARACTER)),
        );
        finish(agent, text.as_bytes())
    })
}

#[derive(Clone)]
struct DecoderState {
    encoding: Encoding,
    mode: DecoderErrorMode,
    ignore_bom: bool,
    seen_bom: bool,
    decoder: Decoder,
}

impl DecoderState {
    fn start(&mut self) {
        self.seen_bom = false;
        self.decoder = Decoder::new(self.encoding, self.mode);
    }

    fn strip_bom<'a>(&mut self, text: &'a str) -> &'a str {
        if self.ignore_bom
            || self.seen_bom
            || text.is_empty()
            || !matches!(
                self.encoding,
                Encoding::Utf8 | Encoding::Utf16Le | Encoding::Utf16Be
            )
        {
            return text;
        }
        self.seen_bom = true;
        text.strip_prefix('\u{feff}').unwrap_or(text)
    }
}

fn decoder_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("TextDecoder requires construction");
    };
    let label = args.first().copied().unwrap_or(Value::UNDEFINED);
    let encoding = if label == Value::UNDEFINED {
        Some(Encoding::Utf8)
    } else {
        let id = flattened_string(agent, label)?;
        let (read, written) = utf8_prefix(agent, id, usize::MAX)?;
        with_utf8(agent, id, read, written, |_, bytes| {
            Ok(Encoding::for_label_bytes(bytes))
        })?
    };
    let options = options(agent, args)?;
    let fatal = boolean_member(agent, options, "fatal")?;
    let ignore_bom = boolean_member(agent, options, "ignoreBOM")?;
    let Some(encoding) = encoding.filter(|encoding| *encoding != Encoding::Replacement) else {
        return Err(agent.throw(ErrorKind::RangeError, "unsupported text encoding label"));
    };
    let prototype = agent.host_prototype_from_new_target(DECODER, target)?;
    let mode = if fatal {
        DecoderErrorMode::Fatal
    } else {
        DecoderErrorMode::Replacement
    };
    let state = DecoderState {
        encoding,
        mode,
        ignore_bom,
        seen_bom: false,
        decoder: Decoder::new(encoding, mode),
    };
    // Decoder reconsumes at most a few bytes after malformed input.
    let object = agent.new_host_state(DECODER, prototype, RefCell::new(state), 16)?;
    Ok(Value::object(object))
}

fn options(agent: &mut Agent, args: &[Value]) -> JsResult<Option<ObjectId>> {
    let input = args.get(1).copied().unwrap_or(Value::UNDEFINED);
    if input.is_nullish() {
        return Ok(None);
    }
    match input.as_object() {
        Some(object) => Ok(Some(object)),
        None => agent.type_error("encoding options must be a dictionary"),
    }
}

fn boolean_member(agent: &mut Agent, options: Option<ObjectId>, name: &str) -> JsResult<bool> {
    let Some(options) = options else {
        return Ok(false);
    };
    let key = agent.heap.key(name)?;
    let value = agent.get(options, key)?;
    Ok(agent.to_boolean(value))
}

fn decoder_state(agent: &mut Agent, this: Value) -> JsResult<Rc<RefCell<DecoderState>>> {
    match this
        .as_object()
        .and_then(|object| agent.host_state(object, DECODER))
    {
        Some(state) => Ok(state),
        None => agent.type_error("invalid TextDecoder receiver"),
    }
}

fn decoder_encoding(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = decoder_state(agent, this)?;
    agent.string(&state.borrow().encoding.name().to_ascii_lowercase())
}

fn decoder_fatal(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = decoder_state(agent, this)?;
    Ok(Value::boolean(matches!(
        state.borrow().mode,
        DecoderErrorMode::Fatal
    )))
}

fn decoder_ignore_bom(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = decoder_state(agent, this)?;
    Ok(Value::boolean(state.borrow().ignore_bom))
}

fn decode(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let state = decoder_state(agent, this)?;
    let input = args.first().copied().unwrap_or(Value::UNDEFINED);
    let source = if input == Value::UNDEFINED {
        None
    } else {
        Some(agent.web_buffer_source(input)?)
    };
    let options = options(agent, args)?;
    let stream = boolean_member(agent, options, "stream")?;

    // Take state after dictionary getters, which may re-enter decode().
    let mut next = state.borrow().clone();
    let length = source.map_or(0, |source| agent.web_buffer_bytes(source).len());
    // Each input byte expands to at most three UTF-8 bytes, with at most
    // four extra bytes of input buffered by a previous streaming call.
    let capacity = length
        .checked_add(4)
        .and_then(|length| length.checked_mul(3))
        .ok_or(Abrupt::OutOfMemory)?;
    agent.with_host_memory(capacity, |agent| {
        let mut output = String::new();
        output
            .try_reserve_exact(capacity)
            .map_err(|_| Abrupt::OutOfMemory)?;
        if let Some(source) = source {
            for start in (0..length).step_by(WORK_CHUNK) {
                agent.check_native_work()?;
                let decoded = {
                    let bytes = agent.web_buffer_bytes(source);
                    let end = (start + WORK_CHUNK).min(length);
                    next.decoder.decode(&bytes[start..end], false, &mut output)
                };
                if decoded.is_err() {
                    state.borrow_mut().start();
                    return agent.type_error("malformed encoded text");
                }
            }
        }
        if !stream && next.decoder.decode(&[], true, &mut output).is_err() {
            state.borrow_mut().start();
            return agent.type_error("incomplete encoded text");
        }
        let result = agent.string(next.strip_bom(&output))?;
        if !stream {
            next.start();
        }
        *state.borrow_mut() = next;
        Ok(result)
    })
}
