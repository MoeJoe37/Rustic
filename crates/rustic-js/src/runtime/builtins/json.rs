//! The `JSON` object (§25.5): `parse` and `stringify`.
//!
//! Parsing keeps its own stack, so nesting depth is bounded by the heap
//! budget rather than the native stack; the reviver walk does the same.
//! `stringify` recurses on the data and stops with a RangeError when the
//! native stack budget of the run is spent.

use std::collections::HashMap;

use super::{Method, arg, install_methods, method};
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::number::{number_to_string, to_integer_or_infinity};
use crate::runtime::object::{Class, Extra, Internal, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{Kind, ObjectId, Value};
use crate::runtime::{ErrorKind, JsStr, OutOfMemory, StringId};

const FUNCTIONS: &[Method] = &[
    method("isRawJSON", is_raw_json, 1),
    method("parse", parse, 2),
    method("rawJSON", raw_json, 1),
    method("stringify", stringify, 3),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let json = agent.new_object_with(Some(object_prototype))?;
    install_methods(agent, json, FUNCTIONS)?;
    let tag = agent.string("JSON")?;
    agent.define_property_or_throw(
        json,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    let global = agent.intrinsics.global;
    let key = agent.heap.key("JSON")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(Value::object(json), true, false, true),
    )
}

fn syntax_error<T>(agent: &mut Agent, message: &str, at: usize) -> JsResult<T> {
    Err(agent.throw(
        ErrorKind::SyntaxError,
        &format!("JSON.parse: {message} at position {at}"),
    ))
}

/// The key of a property as a string value, for reviver and replacer
/// calls.
fn key_value(agent: &mut Agent, key: PropertyKey) -> JsResult<Value> {
    match key {
        PropertyKey::String(s) => Ok(Value::string(s)),
        PropertyKey::Index(i) => agent.string(&i.to_string()),
        PropertyKey::Symbol(_) => unreachable!("JSON only walks string keys"),
    }
}

/// `EnumerableOwnProperties(O, key)` (§7.3.23): string keys only.
fn enumerable_keys(agent: &mut Agent, o: ObjectId) -> JsResult<Vec<PropertyKey>> {
    let mut keys = Vec::new();
    for key in agent.own_property_keys(o)? {
        if matches!(key, PropertyKey::Symbol(_)) {
            continue;
        }
        if let Some(desc) = agent.get_own_property(o, key)? {
            if desc.enumerable == Some(true) {
                agent.heap.charge(8)?;
                keys.push(key);
            }
        }
    }
    Ok(keys)
}

// ---- parse ------------------------------------------------------------------

/// A container being filled while parsing.
enum Open {
    Array(Vec<Value>, Option<Vec<Option<SourceRange>>>),
    Object(ObjectId, Option<PropertyKey>),
}

type SourceRange = (usize, usize);

#[derive(Clone, Copy)]
struct ParsedValue {
    value: Value,
    source: Option<SourceRange>,
}

impl ParsedValue {
    fn container(object: ObjectId) -> Self {
        Self {
            value: Value::object(object),
            source: None,
        }
    }
}

type ParseRecords = HashMap<(ObjectId, PropertyKey), ParsedValue>;

fn record(
    agent: &mut Agent,
    records: &mut ParseRecords,
    holder: ObjectId,
    key: PropertyKey,
    parsed: ParsedValue,
) -> JsResult<()> {
    agent.check_native_work()?;
    // Duplicate object members retain the final value and its own token.
    if !records.contains_key(&(holder, key)) {
        agent
            .heap
            .charge(2 * std::mem::size_of::<((ObjectId, PropertyKey), ParsedValue)>())?;
        records.try_reserve(1).map_err(|_| OutOfMemory)?;
    }
    records.insert((holder, key), parsed);
    Ok(())
}

struct Parser<'a> {
    text: &'a [u16],
    at: usize,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while let Some(&u) = self.text.get(self.at) {
            if matches!(u, 0x09 | 0x0A | 0x0D | 0x20) {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u16> {
        self.text.get(self.at).copied()
    }

    fn eat(&mut self, unit: u8) -> bool {
        if self.peek() == Some(u16::from(unit)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn literal(&mut self, word: &str) -> bool {
        let units: Vec<u16> = word.encode_utf16().collect();
        if self.text[self.at..].starts_with(&units) {
            self.at += units.len();
            true
        } else {
            false
        }
    }

    /// A string after its opening quote, as code units.
    fn string(&mut self, agent: &mut Agent) -> JsResult<Vec<u16>> {
        let mut out = Vec::new();
        loop {
            let Some(u) = self.peek() else {
                return syntax_error(agent, "unterminated string", self.at);
            };
            self.at += 1;
            match u {
                0x22 => return Ok(out),
                0x5C => {
                    let Some(e) = self.peek() else {
                        return syntax_error(agent, "unterminated string", self.at);
                    };
                    self.at += 1;
                    let unit = match u8::try_from(e).unwrap_or(0) {
                        b'"' => 0x22,
                        b'\\' => 0x5C,
                        b'/' => 0x2F,
                        b'b' => 0x08,
                        b'f' => 0x0C,
                        b'n' => 0x0A,
                        b'r' => 0x0D,
                        b't' => 0x09,
                        b'u' => {
                            let hex = self.text.get(self.at..self.at + 4);
                            let value = hex.and_then(|h| {
                                h.iter().try_fold(0u16, |acc, &d| {
                                    char::from_u32(u32::from(d))
                                        .and_then(|c| c.to_digit(16))
                                        .map(|v| acc * 16 + v as u16)
                                })
                            });
                            let Some(value) = value else {
                                return syntax_error(agent, "bad unicode escape", self.at);
                            };
                            self.at += 4;
                            value
                        }
                        _ => return syntax_error(agent, "bad escape", self.at - 1),
                    };
                    out.push(unit);
                }
                u if u < 0x20 => {
                    return syntax_error(agent, "control character in string", self.at - 1);
                }
                u => out.push(u),
            }
            if out.len() % 4096 == 0 {
                agent.heap.charge(8192)?;
            }
        }
    }

    fn number(&mut self, agent: &mut Agent) -> JsResult<f64> {
        let start = self.at;
        self.eat(b'-');
        let digit = |p: &Self| p.peek().is_some_and(|u| (0x30..=0x39).contains(&u));
        if !self.eat(b'0') {
            if !digit(self) {
                return syntax_error(agent, "bad number", self.at);
            }
            while digit(self) {
                self.at += 1;
            }
        }
        if self.eat(b'.') {
            if !digit(self) {
                return syntax_error(agent, "bad number", self.at);
            }
            while digit(self) {
                self.at += 1;
            }
        }
        if self.eat(b'e') || self.eat(b'E') {
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if !digit(self) {
                return syntax_error(agent, "bad number", self.at);
            }
            while digit(self) {
                self.at += 1;
            }
        }
        let text: String = self.text[start..self.at]
            .iter()
            .map(|&u| char::from(u as u8))
            .collect();
        Ok(text.parse::<f64>().unwrap_or(f64::NAN))
    }

    fn primitive(&mut self, agent: &mut Agent) -> JsResult<Value> {
        Ok(match self.peek().and_then(|u| u8::try_from(u).ok()) {
            Some(b'"') => {
                self.at += 1;
                let units = self.string(agent)?;
                agent.string_from(JsStr::from_units(&units))?
            }
            Some(b'-' | b'0'..=b'9') => Value::number(self.number(agent)?),
            Some(b't') if self.literal("true") => Value::TRUE,
            Some(b'f') if self.literal("false") => Value::FALSE,
            Some(b'n') if self.literal("null") => Value::NULL,
            _ => return syntax_error(agent, "unexpected token", self.at),
        })
    }

    /// A whole JSON text. Source records are needed only for callable revivers.
    fn parse(
        &mut self,
        agent: &mut Agent,
        mut records: Option<&mut ParseRecords>,
    ) -> JsResult<ParsedValue> {
        let mut stack: Vec<Open> = Vec::new();
        loop {
            // A value.
            self.skip_space();
            let mut parsed = match self.peek().and_then(|u| u8::try_from(u).ok()) {
                Some(b'{') => {
                    self.at += 1;
                    let o = agent.new_object()?;
                    self.skip_space();
                    if self.eat(b'}') {
                        ParsedValue::container(o)
                    } else {
                        let key = self.member_key(agent)?;
                        agent.heap.charge(std::mem::size_of::<Open>())?;
                        stack.push(Open::Object(o, Some(key)));
                        continue;
                    }
                }
                Some(b'[') => {
                    self.at += 1;
                    self.skip_space();
                    if self.eat(b']') {
                        ParsedValue::container(agent.create_array_from_list(&[])?)
                    } else {
                        agent.heap.charge(std::mem::size_of::<Open>())?;
                        stack.push(Open::Array(Vec::new(), records.is_some().then(Vec::new)));
                        continue;
                    }
                }
                _ => {
                    let start = self.at;
                    let value = self.primitive(agent)?;
                    ParsedValue {
                        value,
                        source: records.is_some().then_some((start, self.at)),
                    }
                }
            };
            // Close containers the value completes.
            loop {
                match stack.last_mut() {
                    None => {
                        self.skip_space();
                        if self.at != self.text.len() {
                            return syntax_error(agent, "unexpected text after JSON", self.at);
                        }
                        return Ok(parsed);
                    }
                    Some(Open::Array(items, sources)) => {
                        agent.heap.charge(8)?;
                        items.push(parsed.value);
                        if let Some(sources) = sources {
                            agent
                                .heap
                                .charge(std::mem::size_of::<Option<SourceRange>>())?;
                            sources.push(parsed.source);
                        }
                        self.skip_space();
                        if self.eat(b',') {
                            break;
                        }
                        if !self.eat(b']') {
                            return syntax_error(agent, "expected , or ]", self.at);
                        }
                        let Some(Open::Array(items, sources)) = stack.pop() else {
                            unreachable!("the top is an array");
                        };
                        let array = agent.create_array_from_list(&items)?;
                        if let (Some(sources), Some(records)) = (sources, records.as_deref_mut()) {
                            for (index, (value, source)) in
                                items.into_iter().zip(sources).enumerate()
                            {
                                let index = u32::try_from(index).map_err(|_| OutOfMemory)?;
                                record(
                                    agent,
                                    records,
                                    array,
                                    PropertyKey::Index(index),
                                    ParsedValue { value, source },
                                )?;
                            }
                        }
                        parsed = ParsedValue::container(array);
                    }
                    Some(Open::Object(o, key)) => {
                        let (o, k) = (*o, key.take().expect("a pending member key"));
                        agent.create_data_property_or_throw(o, k, parsed.value)?;
                        if let Some(records) = records.as_deref_mut() {
                            record(agent, records, o, k, parsed)?;
                        }
                        self.skip_space();
                        if self.eat(b',') {
                            let next = self.member_key(agent)?;
                            if let Some(Open::Object(_, key)) = stack.last_mut() {
                                *key = Some(next);
                            }
                            break;
                        }
                        if !self.eat(b'}') {
                            return syntax_error(agent, "expected , or }", self.at);
                        }
                        stack.pop();
                        parsed = ParsedValue::container(o);
                    }
                }
            }
        }
    }

    /// `"key" :` of an object member.
    fn member_key(&mut self, agent: &mut Agent) -> JsResult<PropertyKey> {
        self.skip_space();
        if !self.eat(b'"') {
            return syntax_error(agent, "expected a property name", self.at);
        }
        let units = self.string(agent)?;
        self.skip_space();
        if !self.eat(b':') {
            return syntax_error(agent, "expected :", self.at);
        }
        Ok(agent.heap.key_for(&JsStr::from_units(&units))?)
    }
}

/// `JSON.parse(text, reviver)` (§25.5.1).
fn parse(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let text = agent.to_string(arg(args, 0))?;
    let reviver = arg(args, 1);
    if !agent.is_callable(reviver) {
        agent.heap.flatten(text)?;
        let units = agent.heap.string(text).to_units();
        return parse_json_units(agent, &units);
    }
    let units = source_units(agent, text)?;
    let mut records = ParseRecords::new();
    let parsed = Parser {
        text: &units,
        at: 0,
    }
    .parse(agent, Some(&mut records))?;
    let root = agent.new_object()?;
    let empty = PropertyKey::String(agent.names.empty);
    agent.create_data_property_or_throw(root, empty, parsed.value)?;
    record(agent, &mut records, root, empty, parsed)?;
    internalize(agent, root, empty, reviver, &records, &units)
}

pub(crate) fn parse_json_units(agent: &mut Agent, text: &[u16]) -> JsResult<Value> {
    Ok(Parser { text, at: 0 }.parse(agent, None)?.value)
}

fn source_units(agent: &mut Agent, text: StringId) -> JsResult<Vec<u16>> {
    agent.heap.flatten(text)?;
    let length = agent.heap.string(text).len();
    for _ in 0..length.div_ceil(1024) {
        agent.check_native_work()?;
    }
    agent
        .heap
        .charge(length.checked_mul(2).ok_or(OutOfMemory)?)?;
    let mut units = Vec::new();
    units.try_reserve_exact(length).map_err(|_| OutOfMemory)?;
    units.extend(agent.heap.string(text).units());
    Ok(units)
}

/// `InternalizeJSONProperty(holder, name, reviver)` (§25.5.1.1), walked
/// with an explicit stack.
fn internalize(
    agent: &mut Agent,
    root: ObjectId,
    name: PropertyKey,
    reviver: Value,
    records: &ParseRecords,
    source_text: &[u16],
) -> JsResult<Value> {
    struct Frame {
        holder: ObjectId,
        name: PropertyKey,
        value: Value,
        children: Vec<PropertyKey>,
        next: usize,
        original: Option<ParsedValue>,
    }
    let open =
        |agent: &mut Agent, holder: ObjectId, name: PropertyKey, track: bool| -> JsResult<Frame> {
            let value = agent.get(holder, name)?;
            let original = track
                .then(|| records.get(&(holder, name)))
                .flatten()
                .copied()
                .filter(|parsed| agent.same_value(parsed.value, value));
            let is_array = super::array::is_array(agent, value)?;
            let children = match value.as_object() {
                Some(o) if is_array => {
                    let length = agent.length_of_array_like(o)?;
                    let length = u32::try_from(length as u64).map_err(|_| OutOfMemory)?;
                    let bytes = (length as usize)
                        .checked_mul(std::mem::size_of::<PropertyKey>())
                        .ok_or(OutOfMemory)?;
                    agent.heap.charge(bytes)?;
                    (0..length).map(PropertyKey::Index).collect()
                }
                Some(o) => enumerable_keys(agent, o)?,
                None => Vec::new(),
            };
            agent.heap.charge(std::mem::size_of::<Frame>())?;
            Ok(Frame {
                holder,
                name,
                value,
                children,
                next: 0,
                original,
            })
        };
    let mut stack = vec![open(agent, root, name, true)?];
    loop {
        let top = stack
            .last_mut()
            .expect("the walk ends when the stack empties");
        if top.next < top.children.len() {
            let key = top.children[top.next];
            top.next += 1;
            let holder = top.value.as_object().expect("only objects have children");
            let frame = open(agent, holder, key, top.original.is_some())?;
            stack.push(frame);
            continue;
        }
        let frame = stack.pop().expect("the stack is not empty");
        let name = key_value(agent, frame.name)?;
        let context = agent.new_object()?;
        if let Some((start, end)) = frame.original.and_then(|parsed| parsed.source) {
            let source = agent.string_from(JsStr::from_units(&source_text[start..end]))?;
            let key = agent.heap.key("source")?;
            agent.create_data_property_or_throw(context, key, source)?;
        }
        let result = agent.call(
            reviver,
            Value::object(frame.holder),
            &[name, frame.value, Value::object(context)],
        )?;
        if stack.is_empty() {
            return Ok(result);
        }
        if result == Value::UNDEFINED {
            agent.delete(frame.holder, frame.name)?;
        } else {
            agent.create_data_property(frame.holder, frame.name, result)?;
        }
    }
}

fn raw_json(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let text = agent.to_string(arg(args, 0))?;
    let units = source_units(agent, text)?;
    let mut parser = Parser {
        text: &units,
        at: 0,
    };
    // Starting directly at the primitive rejects leading space and containers.
    parser.primitive(agent)?;
    if parser.at != units.len() {
        return syntax_error(agent, "unexpected text after raw JSON", parser.at);
    }
    let wrapper = agent.new_object_with(None)?;
    agent.heap.charge(std::mem::size_of::<Extra>())?;
    agent.heap.object_mut(wrapper).extra_mut().internal = Some(Internal::RawJson);
    let key = agent.heap.key("rawJSON")?;
    agent.define_property_or_throw(
        wrapper,
        key,
        PropertyDescriptor::data_with(Value::string(text), false, true, false),
    )?;
    agent.prevent_extensions(wrapper)?;
    Ok(Value::object(wrapper))
}

fn is_raw(agent: &Agent, object: ObjectId) -> bool {
    matches!(
        agent.heap.object(object).internal(),
        Some(Internal::RawJson)
    )
}

fn is_raw_json(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::boolean(
        arg(args, 0)
            .as_object()
            .is_some_and(|object| is_raw(agent, object)),
    ))
}

// ---- stringify ----------------------------------------------------------------

struct State {
    replacer: Option<Value>,
    property_list: Option<Vec<PropertyKey>>,
    stack: Vec<ObjectId>,
    indent: Vec<u16>,
    gap: Vec<u16>,
    out: Vec<u16>,
}

impl Agent {
    /// Serialize with the intrinsic JSON algorithm, without replacer or spacing.
    pub fn stringify_json(&mut self, value: Value) -> JsResult<Value> {
        stringify(self, Value::UNDEFINED, &[value])
    }
}

/// `JSON.stringify(value, replacer, space)` (§25.5.2).
fn stringify(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let (value, replacer, space) = (arg(args, 0), arg(args, 1), arg(args, 2));
    let mut state = State {
        replacer: None,
        property_list: None,
        stack: Vec::new(),
        indent: Vec::new(),
        gap: Vec::new(),
        out: Vec::new(),
    };
    if let Some(r) = replacer.as_object() {
        if agent.is_callable(replacer) {
            state.replacer = Some(replacer);
        } else if super::array::is_array(agent, replacer)? {
            let length = agent.length_of_array_like(r)?;
            let mut list: Vec<PropertyKey> = Vec::new();
            let mut k = 0.0;
            while k < length {
                // Array lengths are below 2³².
                let v = agent.get(r, PropertyKey::Index(k as u32))?;
                let item = match v.kind() {
                    Kind::String(_) | Kind::Number(_) => Some(agent.to_string(v)?),
                    Kind::Object(o) if matches!(agent.class(o), Class::String | Class::Number) => {
                        Some(agent.to_string(v)?)
                    }
                    _ => None,
                };
                if let Some(s) = item {
                    let key = agent.to_property_key(Value::string(s))?;
                    if !list.contains(&key) {
                        agent.heap.charge(8)?;
                        list.push(key);
                    }
                }
                k += 1.0;
            }
            state.property_list = Some(list);
        }
    }
    let mut space = space;
    if let Some(o) = space.as_object() {
        match agent.class(o) {
            Class::Number => space = Value::number(agent.to_number(space)?),
            Class::String => space = Value::string(agent.to_string(space)?),
            _ => {}
        }
    }
    if let Some(n) = space.as_number() {
        let n = to_integer_or_infinity(n).clamp(0.0, 10.0);
        // At most ten spaces.
        state.gap = vec![u16::from(b' '); n as usize];
    } else if let Some(s) = space.as_string() {
        agent.heap.flatten(s)?;
        let units = agent.heap.string(s).to_units();
        state.gap = units[..units.len().min(10)].to_vec();
    }
    let wrapper = agent.new_object()?;
    let empty = PropertyKey::String(agent.names.empty);
    agent.create_data_property_or_throw(wrapper, empty, value)?;
    if serialize_property(agent, &mut state, empty, wrapper)? {
        agent.string_from(JsStr::from_units(&state.out))
    } else {
        Ok(Value::UNDEFINED)
    }
}

fn push(agent: &mut Agent, state: &mut State, units: &[u16]) -> JsResult<()> {
    let before = state.out.capacity();
    state.out.extend_from_slice(units);
    let grown = state.out.capacity() - before;
    if grown > 0 {
        agent.heap.charge(2 * grown)?;
    }
    Ok(())
}

fn push_str(agent: &mut Agent, state: &mut State, text: &str) -> JsResult<()> {
    let units: Vec<u16> = text.encode_utf16().collect();
    push(agent, state, &units)
}

/// `QuoteJSONString(value)` (§25.5.2.3): lone surrogates are escaped.
fn quote(agent: &mut Agent, state: &mut State, units: &[u16]) -> JsResult<()> {
    let mut out: Vec<u16> = Vec::with_capacity(units.len() + 2);
    out.push(u16::from(b'"'));
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        let escape = |out: &mut Vec<u16>, text: &str| out.extend(text.encode_utf16());
        match u {
            0x08 => escape(&mut out, "\\b"),
            0x09 => escape(&mut out, "\\t"),
            0x0A => escape(&mut out, "\\n"),
            0x0C => escape(&mut out, "\\f"),
            0x0D => escape(&mut out, "\\r"),
            0x22 => escape(&mut out, "\\\""),
            0x5C => escape(&mut out, "\\\\"),
            u if u < 0x20 => escape(&mut out, &format!("\\u{u:04x}")),
            0xD800..=0xDBFF
                if units
                    .get(i + 1)
                    .is_some_and(|n| (0xDC00..0xE000).contains(n)) =>
            {
                out.push(u);
                out.push(units[i + 1]);
                i += 1;
            }
            0xD800..=0xDFFF => escape(&mut out, &format!("\\u{u:04x}")),
            u => out.push(u),
        }
        i += 1;
    }
    out.push(u16::from(b'"'));
    push(agent, state, &out)
}

/// `SerializeJSONProperty(state, key, holder)` (§25.5.2.2): appends the
/// text and returns whether there was any (undefined, functions and
/// symbols produce none).
fn serialize_property(
    agent: &mut Agent,
    state: &mut State,
    key: PropertyKey,
    holder: ObjectId,
) -> JsResult<bool> {
    let mut value = agent.get(holder, key)?;
    if value.as_object().is_some() || matches!(value.kind(), Kind::BigInt(_)) {
        let to_json_key = agent.heap.key("toJSON")?;
        let to_json = agent.get_v(value, to_json_key)?;
        if agent.is_callable(to_json) {
            let name = key_value(agent, key)?;
            value = agent.call(to_json, value, &[name])?;
        }
    }
    if let Some(replacer) = state.replacer {
        let name = key_value(agent, key)?;
        value = agent.call(replacer, Value::object(holder), &[name, value])?;
    }
    if let Some(o) = value.as_object() {
        match agent.class(o) {
            Class::Number => value = Value::number(agent.to_number(value)?),
            Class::String => value = Value::string(agent.to_string(value)?),
            Class::Boolean | Class::BigInt => {
                value = agent.primitive_of(o).unwrap_or(value);
            }
            _ => {}
        }
    }
    match value.kind() {
        Kind::Null => push_str(agent, state, "null")?,
        Kind::Boolean(b) => push_str(agent, state, if b { "true" } else { "false" })?,
        Kind::String(s) => {
            agent.heap.flatten(s)?;
            let units = agent.heap.string(s).to_units();
            quote(agent, state, &units)?;
        }
        Kind::Number(n) => {
            let text = if n.is_finite() {
                number_to_string(n)
            } else {
                "null".into()
            };
            push_str(agent, state, &text)?;
        }
        Kind::BigInt(_) => return agent.type_error("JSON.stringify cannot serialize a BigInt"),
        Kind::Object(o) if is_raw(agent, o) => {
            let key = agent.heap.key("rawJSON")?;
            let text = agent
                .get(o, key)?
                .as_string()
                .expect("frozen rawJSON property");
            let units = source_units(agent, text)?;
            push(agent, state, &units)?;
        }
        Kind::Object(o) if !agent.is_callable(value) => {
            if super::array::is_array(agent, value)? {
                serialize_array(agent, state, o)?;
            } else {
                serialize_object(agent, state, o)?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Enters `o` for serialization: the cycle check, the native stack check
/// and the deeper indent.
fn enter(agent: &mut Agent, state: &mut State, o: ObjectId) -> JsResult<Vec<u16>> {
    if state.stack.contains(&o) {
        return agent.type_error("JSON.stringify cannot serialize a cyclic structure");
    }
    agent.check_native_stack()?;
    agent.heap.charge(8 + 2 * state.gap.len())?;
    state.stack.push(o);
    let stepback = state.indent.clone();
    state.indent.extend_from_slice(&state.gap.clone());
    Ok(stepback)
}

fn leave(state: &mut State, stepback: Vec<u16>) {
    state.stack.pop();
    state.indent = stepback;
}

/// The separator before a member: `,` after the first, then a newline
/// and the indent when there is a gap.
fn member_separator(agent: &mut Agent, state: &mut State, first: bool) -> JsResult<()> {
    if !first {
        push_str(agent, state, ",")?;
    }
    if !state.gap.is_empty() {
        push_str(agent, state, "\n")?;
        let indent = state.indent.clone();
        push(agent, state, &indent)?;
    }
    Ok(())
}

fn close(
    agent: &mut Agent,
    state: &mut State,
    stepback: &[u16],
    any: bool,
    bracket: &str,
) -> JsResult<()> {
    if any && !state.gap.is_empty() {
        push_str(agent, state, "\n")?;
        push(agent, state, stepback)?;
    }
    push_str(agent, state, bracket)
}

/// `SerializeJSONObject(state, value)` (§25.5.2.5).
fn serialize_object(agent: &mut Agent, state: &mut State, o: ObjectId) -> JsResult<()> {
    let stepback = enter(agent, state, o)?;
    let keys = match &state.property_list {
        Some(list) => list.clone(),
        None => enumerable_keys(agent, o)?,
    };
    push_str(agent, state, "{")?;
    let mut any = false;
    for key in keys {
        // Members whose value has no text are left out entirely, so the
        // text is built aside and appended only when there is some.
        let mark = state.out.len();
        member_separator(agent, state, !any)?;
        let name = key_value(agent, key)?;
        let name = agent
            .heap
            .string(name.as_string().expect("a string key"))
            .to_units();
        quote(agent, state, &name)?;
        push_str(agent, state, if state.gap.is_empty() { ":" } else { ": " })?;
        if serialize_property(agent, state, key, o)? {
            any = true;
        } else {
            state.out.truncate(mark);
        }
    }
    close(agent, state, &stepback, any, "}")?;
    leave(state, stepback);
    Ok(())
}

/// `SerializeJSONArray(state, value)` (§25.5.2.6).
fn serialize_array(agent: &mut Agent, state: &mut State, o: ObjectId) -> JsResult<()> {
    let stepback = enter(agent, state, o)?;
    let length = agent.length_of_array_like(o)?;
    push_str(agent, state, "[")?;
    let mut index = 0.0;
    while index < length {
        member_separator(agent, state, index == 0.0)?;
        // Array lengths are below 2³².
        if !serialize_property(agent, state, PropertyKey::Index(index as u32), o)? {
            push_str(agent, state, "null")?;
        }
        index += 1.0;
    }
    close(agent, state, &stepback, length > 0.0, "]")?;
    leave(state, stepback);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) -> String {
        let mut agent = Agent::new().unwrap();
        let source = format!("JSON.stringify(JSON.parse({text:?}))");
        let value = agent.evaluate(&source).unwrap();
        agent.display(value)
    }

    #[test]
    fn parse_and_stringify_round_trip() {
        assert_eq!(
            round_trip(r#" {"a": [1, 2.5e3, -0, true, null, "xA\n"], "b": {}} "#),
            r#"{"a":[1,2500,0,true,null,"xA\n"],"b":{}}"#
        );
        assert_eq!(round_trip("[[[[[[]]]]]]"), "[[[[[[]]]]]]");
    }
}
