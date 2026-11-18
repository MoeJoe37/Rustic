//! URI handling functions over the UTF-16 JavaScript string model.

use super::arg;
use crate::runtime::OutOfMemory;
use crate::runtime::agent::{Abrupt, Agent, ErrorKind, JsResult};
use crate::runtime::string::JsStr;
use crate::runtime::value::Value;

const RESERVED: &[u8] = b";/?:@&=+$,#";
const UNESCAPED: &[u8] = b"-_.!~*'()";
const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn malformed(agent: &mut Agent) -> Abrupt {
    agent.throw(ErrorKind::UriError, "malformed URI sequence")
}

pub(super) fn encode<const COMPONENT: bool>(
    agent: &mut Agent,
    _: Value,
    args: &[Value],
) -> JsResult<Value> {
    let input = agent.to_string(arg(args, 0))?;
    let mut index = 0;
    let mut output = Vec::new();
    while let Some(unit) = agent.heap.string(input).unit(index) {
        agent.check_native_work()?;
        index += 1;
        if let Ok(ascii) = u8::try_from(unit) {
            if ascii.is_ascii_alphanumeric()
                || UNESCAPED.contains(&ascii)
                || (!COMPONENT && RESERVED.contains(&ascii))
            {
                reserve_encoded(agent, &mut output, 1)?;
                output.push(ascii);
                continue;
            }
        }

        let mut utf16 = [unit, 0];
        let length = if (0xd800..=0xdbff).contains(&unit) {
            utf16[1] = agent
                .heap
                .string(input)
                .unit(index)
                .ok_or_else(|| malformed(agent))?;
            index += 1;
            2
        } else {
            1
        };
        let Some(Ok(scalar)) = char::decode_utf16(utf16[..length].iter().copied()).next() else {
            return Err(malformed(agent));
        };
        let mut utf8 = [0; 4];
        let bytes = scalar.encode_utf8(&mut utf8).as_bytes();
        reserve_encoded(agent, &mut output, bytes.len() * 3)?;
        for &byte in bytes {
            output.extend_from_slice(&[
                b'%',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0x0f)],
            ]);
        }
    }
    agent.string_from(JsStr::Latin1(output.into_boxed_slice()))
}

fn reserve_encoded(agent: &Agent, output: &mut Vec<u8>, additional: usize) -> JsResult<()> {
    if additional > agent.heap.byte_limit().saturating_sub(output.len()) {
        return Err(Abrupt::OutOfMemory);
    }
    output.try_reserve(additional).map_err(|_| OutOfMemory)?;
    Ok(())
}

pub(super) fn decode<const COMPONENT: bool>(
    agent: &mut Agent,
    _: Value,
    args: &[Value],
) -> JsResult<Value> {
    let input = agent.to_string(arg(args, 0))?;
    let mut index = 0;
    // Each output unit consumes at least one input unit, bounding this buffer
    // by the logical length of the input.
    let mut output = Vec::new();
    while let Some(unit) = agent.heap.string(input).unit(index) {
        agent.check_native_work()?;
        if unit != u16::from(b'%') {
            output.try_reserve(1).map_err(|_| OutOfMemory)?;
            output.push(unit);
            index += 1;
            continue;
        }
        let start = index;
        let source = agent.heap.string(input);
        let first = percent_byte(source, index).ok_or_else(|| malformed(agent))?;
        index += 3;
        if first.is_ascii() {
            if !COMPONENT && RESERVED.contains(&first) {
                output.try_reserve(3).map_err(|_| OutOfMemory)?;
                output.extend((start..index).map(|position| {
                    agent
                        .heap
                        .string(input)
                        .unit(position)
                        .expect("validated percent escape")
                }));
            } else {
                output.try_reserve(1).map_err(|_| OutOfMemory)?;
                output.push(u16::from(first));
            }
            continue;
        }

        let width = match first {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return Err(malformed(agent)),
        };
        let mut bytes = [first, 0, 0, 0];
        for next in &mut bytes[1..width] {
            *next =
                percent_byte(agent.heap.string(input), index).ok_or_else(|| malformed(agent))?;
            index += 3;
        }
        let scalar = std::str::from_utf8(&bytes[..width])
            .ok()
            .and_then(|text| text.chars().next())
            .ok_or_else(|| malformed(agent))?;
        let mut units = [0; 2];
        let encoded = scalar.encode_utf16(&mut units);
        output.try_reserve(encoded.len()).map_err(|_| OutOfMemory)?;
        output.extend_from_slice(encoded);
    }

    let contents = if output.iter().all(|&unit| unit <= u16::from(u8::MAX)) {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(output.len())
            .map_err(|_| OutOfMemory)?;
        bytes.extend(output.iter().map(|&unit| unit as u8));
        JsStr::Latin1(bytes.into_boxed_slice())
    } else {
        JsStr::Utf16(output.into_boxed_slice())
    };
    agent.string_from(contents)
}

fn percent_byte(source: &JsStr, start: usize) -> Option<u8> {
    if source.unit(start)? != u16::from(b'%') {
        return None;
    }
    let digit = |index| {
        let unit = source.unit(index)?;
        let value = char::from_u32(u32::from(unit))?.to_digit(16)?;
        u8::try_from(value).ok()
    };
    Some(digit(start.checked_add(1)?)? * 16 + digit(start.checked_add(2)?)?)
}
