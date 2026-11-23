//! ECMAScript Uint8Array base64 and hex conversions.

use super::ElementKind;
use super::{View, create_same_type, length_of, view_of};
use crate::runtime::builtins::{arg, install_methods, method};
use crate::runtime::{Abrupt, Agent, ErrorKind, JsResult, JsStr, ObjectId, StringId, Value};

const HEX: &[u8; 16] = b"0123456789abcdef";
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let index = ElementKind::Uint8.index();
    let constructor = agent.intrinsics.typed_array_constructors[index];
    let prototype = agent.intrinsics.typed_array_prototypes[index];
    install_methods(
        agent,
        constructor,
        &[
            method("fromBase64", from_base64, 1),
            method("fromHex", from_hex, 1),
        ],
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("setFromBase64", set_from_base64, 1),
            method("setFromHex", set_from_hex, 1),
            method("toBase64", to_base64, 0),
            method("toHex", to_hex, 0),
        ],
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

#[derive(Clone, Copy)]
enum Format {
    Hex,
    Base64 { url: bool, last_chunk: LastChunk },
}

fn options_object(agent: &mut Agent, value: Value) -> JsResult<Option<ObjectId>> {
    if value == Value::UNDEFINED {
        return Ok(None);
    }
    match value.as_object() {
        Some(object) => Ok(Some(object)),
        None => agent.type_error("Uint8Array encoding options must be an object"),
    }
}

fn option(agent: &mut Agent, object: Option<ObjectId>, name: &str) -> JsResult<Value> {
    let Some(object) = object else {
        return Ok(Value::UNDEFINED);
    };
    let key = agent.heap.key(name)?;
    agent.get(object, key)
}

fn is_option(agent: &Agent, value: Value, name: &str) -> bool {
    value.as_string().is_some_and(|string| {
        agent
            .heap
            .string(string)
            .units()
            .eq(name.bytes().map(u16::from))
    })
}

fn url_alphabet(agent: &mut Agent, options: Option<ObjectId>) -> JsResult<bool> {
    let value = option(agent, options, "alphabet")?;
    if value == Value::UNDEFINED || is_option(agent, value, "base64") {
        Ok(false)
    } else if is_option(agent, value, "base64url") {
        Ok(true)
    } else {
        agent.type_error("Invalid base64 alphabet")
    }
}

fn base64_format(agent: &mut Agent, options: Value) -> JsResult<Format> {
    let options = options_object(agent, options)?;
    let url = url_alphabet(agent, options)?;
    let value = option(agent, options, "lastChunkHandling")?;
    let last_chunk = if value == Value::UNDEFINED || is_option(agent, value, "loose") {
        LastChunk::Loose
    } else if is_option(agent, value, "strict") {
        LastChunk::Strict
    } else if is_option(agent, value, "stop-before-partial") {
        LastChunk::StopBeforePartial
    } else {
        return agent.type_error("Invalid base64 lastChunkHandling");
    };
    Ok(Format::Base64 { url, last_chunk })
}

fn string_arg(agent: &mut Agent, value: Value) -> JsResult<StringId> {
    match value.as_string() {
        Some(string) => Ok(string),
        None => agent.type_error("Uint8Array decoding requires a string"),
    }
}

fn uint8_view(agent: &mut Agent, value: Value) -> JsResult<View> {
    match value.as_object().and_then(|object| view_of(agent, object)) {
        Some(view) if view.kind == ElementKind::Uint8 => Ok(view),
        _ => agent.type_error("Uint8Array method called on an incompatible receiver"),
    }
}

fn available_length(agent: &mut Agent, view: View) -> JsResult<usize> {
    match length_of(agent, &view) {
        Some(length) => Ok(length),
        None => agent.type_error("Uint8Array is detached or out of bounds"),
    }
}

fn from_hex(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let input = string_arg(agent, arg(args, 0))?;
    decode_new(agent, input, Format::Hex)
}

fn from_base64(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let input = string_arg(agent, arg(args, 0))?;
    let format = base64_format(agent, arg(args, 1))?;
    decode_new(agent, input, format)
}

fn set_from_hex(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let view = uint8_view(agent, this)?;
    agent.require_mutable_buffer(view.buffer)?;
    let input = string_arg(agent, arg(args, 0))?;
    decode_into(agent, input, Format::Hex, view)
}

fn set_from_base64(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let view = uint8_view(agent, this)?;
    agent.require_mutable_buffer(view.buffer)?;
    let input = string_arg(agent, arg(args, 0))?;
    let format = base64_format(agent, arg(args, 1))?;
    decode_into(agent, input, format, view)
}

fn decode_new(agent: &mut Agent, input: StringId, format: Format) -> JsResult<Value> {
    // Validate and count first so allocating the result needs no temporary
    // byte vector and malformed input wins over result allocation failure.
    let result = decode(agent, input, format, usize::MAX, |_, _, _| Ok(()))?.finish(agent)?;
    let array = create_same_type(agent, ElementKind::Uint8, result.written)?;
    let view = view_of(agent, array).expect("a new Uint8Array");
    decode(
        agent,
        input,
        format,
        result.written,
        |agent, offset, bytes| write_bytes(agent, view, offset, bytes),
    )?
    .finish(agent)?;
    Ok(Value::object(array))
}

fn decode_into(agent: &mut Agent, input: StringId, format: Format, view: View) -> JsResult<Value> {
    // Option accessors may have detached or resized the backing buffer.
    let length = available_length(agent, view)?;
    let result = decode(agent, input, format, length, |agent, offset, bytes| {
        write_bytes(agent, view, offset, bytes)
    })?
    .finish(agent)?;
    let object = agent.new_object()?;
    for (name, count) in [("read", result.read), ("written", result.written)] {
        let key = agent.heap.key(name)?;
        agent.create_data_property_or_throw(object, key, Value::number(count as f64))?;
    }
    Ok(Value::object(object))
}

fn write_bytes(agent: &mut Agent, view: View, offset: usize, bytes: &[u8]) -> JsResult<()> {
    let mut bits = [0; 8];
    bits[..bytes.len()].copy_from_slice(bytes);
    agent.buffer_write(
        view.buffer,
        view.offset + offset,
        bytes.len(),
        u64::from_le_bytes(bits),
    )
}

#[derive(Default)]
struct Decoded {
    read: usize,
    written: usize,
    malformed: bool,
}

impl Decoded {
    fn invalid(mut self) -> Self {
        self.malformed = true;
        self
    }

    fn finish(self, agent: &mut Agent) -> JsResult<Self> {
        if self.malformed {
            Err(agent.throw(ErrorKind::SyntaxError, "Invalid Uint8Array encoded input"))
        } else {
            Ok(self)
        }
    }
}

fn decode(
    agent: &mut Agent,
    input: StringId,
    format: Format,
    capacity: usize,
    write: impl FnMut(&mut Agent, usize, &[u8]) -> JsResult<()>,
) -> JsResult<Decoded> {
    match format {
        Format::Hex => decode_hex(agent, input, capacity, write),
        Format::Base64 { url, last_chunk } => {
            decode_base64(agent, input, capacity, url, last_chunk, write)
        }
    }
}

fn decode_hex(
    agent: &mut Agent,
    input: StringId,
    capacity: usize,
    mut write: impl FnMut(&mut Agent, usize, &[u8]) -> JsResult<()>,
) -> JsResult<Decoded> {
    let length = agent.heap.string(input).len();
    let mut result = Decoded::default();
    if length % 2 != 0 {
        return Ok(result.invalid());
    }
    let digit = |unit: Option<u16>| {
        char::from(u8::try_from(unit?).ok()?)
            .to_digit(16)
            .map(|value| value as u8)
    };
    for offset in 0..capacity.min(length / 2) {
        agent.check_native_work()?;
        let string = agent.heap.string(input);
        let (Some(high), Some(low)) = (
            digit(string.unit(result.read)),
            digit(string.unit(result.read + 1)),
        ) else {
            return Ok(result.invalid());
        };
        write(agent, offset, &[high << 4 | low])?;
        result.read += 2;
        result.written += 1;
    }
    Ok(result)
}

fn next_non_space(agent: &mut Agent, input: StringId, index: &mut usize) -> JsResult<Option<u16>> {
    while let Some(unit) = agent.heap.string(input).unit(*index) {
        agent.check_native_work()?;
        *index += 1;
        if !matches!(unit, 0x09 | 0x0a | 0x0c | 0x0d | 0x20) {
            return Ok(Some(unit));
        }
    }
    Ok(None)
}

fn sextet(unit: u16, url: bool) -> Option<u8> {
    Some(match u8::try_from(unit).ok()? {
        c @ b'A'..=b'Z' => c - b'A',
        c @ b'a'..=b'z' => c - b'a' + 26,
        c @ b'0'..=b'9' => c - b'0' + 52,
        b'-' if url => 62,
        b'_' if url => 63,
        b'+' if !url => 62,
        b'/' if !url => 63,
        _ => return None,
    })
}

fn decode_base64(
    agent: &mut Agent,
    input: StringId,
    capacity: usize,
    url: bool,
    last_chunk: LastChunk,
    mut write: impl FnMut(&mut Agent, usize, &[u8]) -> JsResult<()>,
) -> JsResult<Decoded> {
    let mut result = Decoded::default();
    let mut index = 0;
    while result.written < capacity {
        let remaining = capacity - result.written;
        let mut chunk = [0; 4];
        let mut digits = 0;
        while digits < chunk.len() {
            match next_non_space(agent, input, &mut index)? {
                None => {
                    if digits == 0 {
                        result.read = index;
                        return Ok(result);
                    }
                    match last_chunk {
                        LastChunk::StopBeforePartial => return Ok(result),
                        LastChunk::Loose if digits >= 2 => break,
                        _ => return Ok(result.invalid()),
                    }
                }
                Some(0x3d) => {
                    if digits < 2 {
                        return Ok(result.invalid());
                    }
                    let mut next = next_non_space(agent, input, &mut index)?;
                    if digits == 2 {
                        if next.is_none() && last_chunk == LastChunk::StopBeforePartial {
                            return Ok(result);
                        }
                        if next != Some(0x3d) {
                            return Ok(result.invalid());
                        }
                        next = next_non_space(agent, input, &mut index)?;
                    }
                    let unused_mask = if digits == 2 { 0x0f } else { 0x03 };
                    if next.is_some()
                        || (last_chunk == LastChunk::Strict && chunk[digits - 1] & unused_mask != 0)
                    {
                        return Ok(result.invalid());
                    }
                    break;
                }
                Some(unit) => {
                    let Some(digit) = sextet(unit, url) else {
                        return Ok(result.invalid());
                    };
                    if digits == remaining.saturating_add(1) {
                        return Ok(result);
                    }
                    chunk[digits] = digit;
                    digits += 1;
                }
            }
        }
        let [a, b, c, d] = chunk;
        let bytes = [a << 2 | b >> 4, b << 4 | c >> 2, c << 6 | d];
        let written = digits - 1;
        write(agent, result.written, &bytes[..written])?;
        result.written += written;
        result.read = index;
        if digits != 4 {
            break;
        }
    }
    Ok(result)
}

fn output_bytes(agent: &Agent, length: Option<usize>) -> JsResult<Vec<u8>> {
    let free = agent
        .heap
        .byte_limit()
        .saturating_sub(agent.heap.bytes_used());
    let length = length
        .filter(|&length| length <= free)
        .ok_or(Abrupt::OutOfMemory)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| Abrupt::OutOfMemory)?;
    Ok(output)
}

fn to_hex(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let view = uint8_view(agent, this)?;
    let length = available_length(agent, view)?;
    let mut output = output_bytes(agent, length.checked_mul(2))?;
    for offset in 0..length {
        agent.check_native_work()?;
        let [byte, ..] = agent
            .buffer_read(view.buffer, view.offset + offset, 1)
            .to_le_bytes();
        output.extend_from_slice(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0x0f)]]);
    }
    agent.string_from(JsStr::Latin1(output.into_boxed_slice()))
}

fn to_base64(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let view = uint8_view(agent, this)?;
    let options = options_object(agent, arg(args, 0))?;
    let alphabet = if url_alphabet(agent, options)? {
        BASE64_URL
    } else {
        BASE64
    };
    let padding = option(agent, options, "omitPadding")?;
    let omit_padding = agent.to_boolean(padding);
    let length = available_length(agent, view)?;
    let tail = length % 3;
    let extra = match tail {
        0 => 0,
        _ if omit_padding => tail + 1,
        _ => 4,
    };
    let mut output = output_bytes(
        agent,
        (length / 3)
            .checked_mul(4)
            .and_then(|n| n.checked_add(extra)),
    )?;
    for offset in (0..length).step_by(3) {
        agent.check_native_work()?;
        let size = (length - offset).min(3);
        let [a, b, c, ..] = agent
            .buffer_read(view.buffer, view.offset + offset, size)
            .to_le_bytes();
        let digits = [
            a >> 2,
            (a & 3) << 4 | b >> 4,
            (b & 15) << 2 | c >> 6,
            c & 63,
        ];
        output.extend(
            digits[..size + 1]
                .iter()
                .map(|&digit| alphabet[usize::from(digit)]),
        );
        if !omit_padding {
            output.extend_from_slice(&b"=="[..3 - size]);
        }
    }
    agent.string_from(JsStr::Latin1(output.into_boxed_slice()))
}
