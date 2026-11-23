//! `RegExp` (§22.2.4–§22.2.9): the constructor, `RegExp.prototype` with
//! `exec` and the `Symbol.match`/`matchAll`/`replace`/`search`/`split`
//! protocol, `%RegExpStringIteratorPrototype%` and Annex B `compile`.
//!
//! Patterns are compiled by [`crate::regexp`]; a small cache on the agent
//! keeps the programs of recent patterns, so a literal in a loop or a
//! repeated `new RegExp(text)` compiles once.

use std::rc::Rc;

use super::{Method, arg, install_methods, iterator_result, method, prototype_from_new_target};
use crate::regexp::{ExecError, Flags, Regex, UNSET};
use crate::runtime::agent::{Abrupt, Agent, JsResult, NativeFn};
use crate::runtime::number::{to_integer_or_infinity, to_length, to_uint32};
use crate::runtime::object::{Internal, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{ObjectId, Value};
use crate::runtime::{ErrorKind, JsStr};

const PROTOTYPE_METHODS: &[Method] = &[
    method("compile", compile, 2),
    method("exec", exec, 1),
    method("test", test, 1),
    method("toString", to_string, 0),
];

const STATIC_METHODS: &[Method] = &[method("escape", escape, 1)];

/// The flag getters, in the order the `flags` getter reads them.
const FLAG_GETTERS: &[(&str, u16)] = &[
    ("hasIndices", b'd' as u16),
    ("global", b'g' as u16),
    ("ignoreCase", b'i' as u16),
    ("multiline", b'm' as u16),
    ("dotAll", b's' as u16),
    ("unicode", b'u' as u16),
    ("unicodeSets", b'v' as u16),
    ("sticky", b'y' as u16),
];

/// Compiled patterns kept for reuse.
const CACHE_SIZE: usize = 32;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let prototype = agent.new_object()?;
    let constructor =
        super::install_constructor(agent, "RegExp", regexp_constructor, 2, prototype)?;
    install_methods(agent, constructor, STATIC_METHODS)?;
    install_methods(agent, prototype, PROTOTYPE_METHODS)?;
    let species = agent.new_native_function(species_getter, "get [Symbol.species]", 0)?;
    let key = PropertyKey::Symbol(agent.symbols.species);
    agent.define_property_or_throw(
        constructor,
        key,
        PropertyDescriptor::accessor(Value::object(species), Value::UNDEFINED, false, true),
    )?;
    for &(name, _) in FLAG_GETTERS {
        getter(agent, prototype, name, flag_getter)?;
    }
    getter(agent, prototype, "flags", flags_getter)?;
    getter(agent, prototype, "source", source_getter)?;
    let symbols = [
        (
            agent.symbols.match_,
            "[Symbol.match]",
            symbol_match as NativeFn,
            1,
        ),
        (
            agent.symbols.match_all,
            "[Symbol.matchAll]",
            symbol_match_all,
            1,
        ),
        (agent.symbols.replace, "[Symbol.replace]", symbol_replace, 2),
        (agent.symbols.search, "[Symbol.search]", symbol_search, 1),
        (agent.symbols.split, "[Symbol.split]", symbol_split, 2),
    ];
    for (symbol, name, behavior, length) in symbols {
        agent.define_method(
            prototype,
            PropertyKey::Symbol(symbol),
            behavior,
            name,
            length,
        )?;
    }

    // %RegExpStringIteratorPrototype% (§22.2.9.2).
    let iterator_prototype = agent
        .get_prototype_of(agent.intrinsics.array_iterator_prototype)?
        .expect("%ArrayIteratorPrototype% inherits from %IteratorPrototype%");
    let string_iterator = agent.new_object_with(Some(iterator_prototype))?;
    install_methods(
        agent,
        string_iterator,
        &[method("next", string_iterator_next, 0)],
    )?;
    let tag_key = PropertyKey::Symbol(agent.symbols.to_string_tag);
    let tag = agent.string("RegExp String Iterator")?;
    agent.define_property_or_throw(
        string_iterator,
        tag_key,
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;

    agent.intrinsics.regexp = constructor;
    agent.intrinsics.regexp_prototype = prototype;
    agent.intrinsics.regexp_string_iterator_prototype = string_iterator;
    Ok(())
}

fn getter(
    agent: &mut Agent,
    target: ObjectId,
    name: &'static str,
    behavior: NativeFn,
) -> JsResult<()> {
    let f = agent.new_native_function(behavior, &format!("get {name}"), 0)?;
    let key = agent.heap.key(name)?;
    agent.define_property_or_throw(
        target,
        key,
        PropertyDescriptor::accessor(Value::object(f), Value::UNDEFINED, false, true),
    )
}

fn species_getter(_: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

// ---- helpers ------------------------------------------------------------------

fn units_of(agent: &mut Agent, value: Value) -> JsResult<Vec<u16>> {
    let s = agent.to_string(value)?;
    agent.heap.flatten(s)?;
    Ok(agent.heap.string(s).to_units())
}

fn make(agent: &mut Agent, units: &[u16]) -> JsResult<Value> {
    agent.string_from(JsStr::from_units(units))
}

fn key_of(agent: &mut Agent, name: &str) -> JsResult<PropertyKey> {
    Ok(agent.heap.key(name)?)
}

fn index_key(i: usize) -> PropertyKey {
    // Captures and array positions stay far below `u32::MAX`.
    PropertyKey::Index(i as u32)
}

fn last_index_key(agent: &mut Agent) -> JsResult<PropertyKey> {
    key_of(agent, "lastIndex")
}

fn set_last_index(agent: &mut Agent, o: ObjectId, value: f64) -> JsResult<()> {
    let key = last_index_key(agent)?;
    agent.set(o, key, Value::number(value), true)
}

fn get_last_index(agent: &mut Agent, o: ObjectId) -> JsResult<f64> {
    let key = last_index_key(agent)?;
    let v = agent.get(o, key)?;
    let n = agent.to_number(v)?;
    Ok(to_length(n))
}

/// The receiver as an object, or a TypeError naming the method.
fn this_object(agent: &mut Agent, this: Value, name: &str) -> JsResult<ObjectId> {
    match this.as_object() {
        Some(o) => Ok(o),
        None => agent.type_error(&format!("RegExp.prototype.{name} called on a non-object")),
    }
}

/// The matcher and flags of a RegExp object, if `o` is one.
/// Whether `o` has `[[RegExpMatcher]]`.
pub(crate) fn is_regexp_object(agent: &Agent, o: ObjectId) -> bool {
    matches!(
        agent.heap.object(o).internal(),
        Some(Internal::RegExp { .. })
    )
}

fn regexp_data(agent: &Agent, o: ObjectId) -> Option<(Rc<Regex>, Value, Value)> {
    match agent.heap.object(o).internal() {
        Some(Internal::RegExp {
            matcher,
            source,
            flags,
        }) => Some((Rc::clone(matcher), *source, *flags)),
        _ => None,
    }
}

/// `AdvanceStringIndex(S, index, unicode)` (§22.2.7.3).
fn advance(s: &[u16], index: usize, unicode: bool) -> usize {
    if unicode
        && index + 1 < s.len()
        && (0xD800..0xDC00).contains(&s[index])
        && (0xDC00..0xE000).contains(&s[index + 1])
    {
        index + 2
    } else {
        index + 1
    }
}

fn contains_flag(flags: &[u16], c: u8) -> bool {
    flags.contains(&u16::from(c))
}

// ---- creation -----------------------------------------------------------------

/// Compile `pattern` with `flags`, reusing a recent compilation.
fn compiled(agent: &mut Agent, pattern: &[u16], flags: Flags) -> JsResult<Rc<Regex>> {
    if let Some(i) = agent
        .regexp_cache
        .iter()
        .position(|(p, f, _)| *f == flags && p[..] == *pattern)
    {
        let entry = agent.regexp_cache.remove(i);
        let matcher = Rc::clone(&entry.2);
        agent.regexp_cache.push(entry);
        return Ok(matcher);
    }
    let matcher = match Regex::new(pattern, flags) {
        Ok(m) => Rc::new(m),
        Err(e) => {
            return Err(agent.throw(
                ErrorKind::SyntaxError,
                &format!("invalid regular expression: {}", e.message),
            ));
        }
    };
    agent.heap.charge(matcher.footprint())?;
    if agent.regexp_cache.len() == CACHE_SIZE {
        agent.regexp_cache.remove(0);
    }
    agent
        .regexp_cache
        .push((pattern.into(), flags, Rc::clone(&matcher)));
    Ok(matcher)
}

/// `RegExpAlloc(newTarget)` (§22.2.3.2): an object with `lastIndex`.
fn regexp_alloc(agent: &mut Agent, prototype: ObjectId) -> JsResult<ObjectId> {
    let o = agent.new_object_with(Some(prototype))?;
    let key = last_index_key(agent)?;
    agent.define_property_or_throw(
        o,
        key,
        PropertyDescriptor::data_with(Value::number(0.0), true, false, false),
    )?;
    Ok(o)
}

/// `RegExpInitialize(obj, pattern, flags)` (§22.2.3.3).
fn regexp_initialize(
    agent: &mut Agent,
    o: ObjectId,
    pattern: Value,
    flags: Value,
) -> JsResult<Value> {
    let p = if pattern == Value::UNDEFINED {
        agent.string("")?
    } else {
        let s = agent.to_string(pattern)?;
        Value::string(s)
    };
    let f = if flags == Value::UNDEFINED {
        agent.string("")?
    } else {
        let s = agent.to_string(flags)?;
        Value::string(s)
    };
    let flag_units = units_of(agent, f)?;
    let Some(parsed) = Flags::parse(flag_units.iter().copied()) else {
        return Err(agent.throw(ErrorKind::SyntaxError, "invalid regular expression flags"));
    };
    let pattern_units = units_of(agent, p)?;
    let matcher = compiled(agent, &pattern_units, parsed)?;
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>())?;
    agent.heap.object_mut(o).extra_mut().internal = Some(Internal::RegExp {
        matcher,
        source: p,
        flags: f,
    });
    set_last_index(agent, o, 0.0)?;
    Ok(Value::object(o))
}

/// `RegExpCreate(P, F)` (§22.2.3.1).
fn regexp_create(agent: &mut Agent, pattern: Value, flags: Value) -> JsResult<Value> {
    let prototype = agent.intrinsics.regexp_prototype;
    let o = regexp_alloc(agent, prototype)?;
    regexp_initialize(agent, o, pattern, flags)
}

/// A regular expression literal's value.
pub(crate) fn regexp_create_literal(
    agent: &mut Agent,
    pattern: Value,
    flags: Value,
) -> JsResult<Value> {
    regexp_create(agent, pattern, flags)
}

/// `IsRegExp(argument)` (§7.2.6).
pub(crate) fn is_regexp(agent: &mut Agent, value: Value) -> JsResult<bool> {
    let Some(o) = value.as_object() else {
        return Ok(false);
    };
    let key = PropertyKey::Symbol(agent.symbols.match_);
    let matcher = agent.get(o, key)?;
    if matcher != Value::UNDEFINED {
        return Ok(agent.to_boolean(matcher));
    }
    Ok(regexp_data(agent, o).is_some())
}

/// `RegExp(pattern, flags)` (§22.2.4.1).
fn regexp_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let (pattern, flags) = (arg(args, 0), arg(args, 1));
    let pattern_is_regexp = is_regexp(agent, pattern)?;
    if agent.new_target().is_none() && pattern_is_regexp && flags == Value::UNDEFINED {
        let o = pattern.as_object().expect("a RegExp is an object");
        let key = PropertyKey::String(agent.names.constructor);
        let constructor = agent.get(o, key)?;
        if agent.active_function().map(Value::object) == Some(constructor) {
            return Ok(pattern);
        }
    }
    let (p, f) = match pattern.as_object().and_then(|o| regexp_data(agent, o)) {
        Some((_, source, original_flags)) => (
            source,
            if flags == Value::UNDEFINED {
                original_flags
            } else {
                flags
            },
        ),
        None if pattern_is_regexp => {
            let o = pattern.as_object().expect("a RegExp is an object");
            let key = key_of(agent, "source")?;
            let source = agent.get(o, key)?;
            let f = if flags == Value::UNDEFINED {
                let key = key_of(agent, "flags")?;
                agent.get(o, key)?
            } else {
                flags
            };
            (source, f)
        }
        None => (pattern, flags),
    };
    let default = agent.intrinsics.regexp_prototype;
    let prototype = prototype_from_new_target(agent, default)?;
    let o = regexp_alloc(agent, prototype)?;
    regexp_initialize(agent, o, p, f)
}

/// `RegExp.prototype.compile(pattern, flags)` (§B.2.4.1).
fn compile(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = this_object(agent, this, "compile")?;
    if regexp_data(agent, o).is_none() {
        return agent.type_error("RegExp.prototype.compile called on an incompatible receiver");
    }
    let (pattern, flags) = (arg(args, 0), arg(args, 1));
    let (p, f) = match pattern.as_object().and_then(|p| regexp_data(agent, p)) {
        Some((_, source, original_flags)) => {
            if flags != Value::UNDEFINED {
                return agent.type_error("flags must be undefined when compiling from a RegExp");
            }
            (source, original_flags)
        }
        None => (pattern, flags),
    };
    regexp_initialize(agent, o, p, f)
}

/// `RegExp.escape(S)` (§22.2.5.1).
fn escape(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(s) = arg(args, 0).as_string() else {
        return agent.type_error("RegExp.escape expects a string");
    };
    agent.heap.flatten(s)?;
    let units = agent.heap.string(s).to_units();
    let mut out = Vec::with_capacity(units.len() * 2);
    let hex = |out: &mut Vec<u16>, c: u16| {
        let text = format!("\\x{c:02x}");
        out.extend(text.encode_utf16());
    };
    for (i, &c) in units.iter().enumerate() {
        let is_alnum = c < 0x80 && (c as u8).is_ascii_alphanumeric();
        if i == 0 && is_alnum {
            hex(&mut out, c);
        } else if c < 0x80 && b"^$\\.*+?()[]{}|/".contains(&(c as u8)) {
            out.push(u16::from(b'\\'));
            out.push(c);
        } else if c < 0x80 && b",-=<>#&!%:;@~'`\"".contains(&(c as u8)) {
            hex(&mut out, c);
        } else if matches!(c, 0x09..=0x0D | 0x20) {
            match c {
                0x09 => out.extend("\\t".encode_utf16()),
                0x0A => out.extend("\\n".encode_utf16()),
                0x0B => out.extend("\\v".encode_utf16()),
                0x0C => out.extend("\\f".encode_utf16()),
                0x0D => out.extend("\\r".encode_utf16()),
                _ => hex(&mut out, c),
            }
        } else if crate::unicode::is_whitespace(u32::from(c))
            || crate::unicode::is_line_terminator(u32::from(c))
            || (0xD800..0xE000).contains(&c) && !paired(&units, i)
        {
            if c <= 0xFF {
                hex(&mut out, c);
            } else {
                let text = format!("\\u{c:04x}");
                out.extend(text.encode_utf16());
            }
        } else {
            out.push(c);
        }
    }
    agent.heap.charge(2 * out.len())?;
    make(agent, &out)
}

/// Whether the surrogate at `i` is half of a pair.
fn paired(units: &[u16], i: usize) -> bool {
    let c = units[i];
    if (0xD800..0xDC00).contains(&c) {
        units
            .get(i + 1)
            .is_some_and(|n| (0xDC00..0xE000).contains(n))
    } else {
        i > 0 && (0xD800..0xDC00).contains(&units[i - 1])
    }
}

// ---- accessors ----------------------------------------------------------------

/// The flag a getter reads: its name is in the running function's name.
fn flag_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let f = agent.active_function().expect("a getter is running");
    let name_key = PropertyKey::String(agent.names.name);
    let name = agent.get(f, name_key)?;
    let name = units_of(agent, name)?;
    let name = String::from_utf16_lossy(&name);
    let letter = FLAG_GETTERS
        .iter()
        .find(|(n, _)| name.strip_prefix("get ") == Some(n))
        .map_or(0, |&(_, c)| c);
    let Some(o) = this.as_object() else {
        return agent.type_error("a RegExp flag getter called on a non-object");
    };
    match regexp_data(agent, o) {
        Some((_, _, flags)) => {
            let flags = units_of(agent, flags)?;
            Ok(Value::boolean(flags.contains(&letter)))
        }
        None if o == agent.intrinsics.regexp_prototype => Ok(Value::UNDEFINED),
        None => agent.type_error("a RegExp flag getter called on an incompatible receiver"),
    }
}

/// `get RegExp.prototype.flags` (§22.2.6.4).
fn flags_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = this_object(agent, this, "flags")?;
    let mut out = Vec::new();
    for &(name, letter) in FLAG_GETTERS {
        let key = key_of(agent, name)?;
        let v = agent.get(o, key)?;
        if agent.to_boolean(v) {
            out.push(letter);
        }
    }
    make(agent, &out)
}

/// `get RegExp.prototype.source` (§22.2.6.13).
fn source_getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = this_object(agent, this, "source")?;
    let Some((_, source, _)) = regexp_data(agent, o) else {
        if o == agent.intrinsics.regexp_prototype {
            return agent.string("(?:)");
        }
        return agent.type_error("RegExp.prototype.source called on an incompatible receiver");
    };
    let units = units_of(agent, source)?;
    let escaped = escape_pattern(&units);
    make(agent, &escaped)
}

/// `EscapeRegExpPattern` (§22.2.6.13.1): a source that reads back as the
/// same pattern between slashes.
fn escape_pattern(units: &[u16]) -> Vec<u16> {
    if units.is_empty() {
        return "(?:)".encode_utf16().collect();
    }
    let mut out = Vec::with_capacity(units.len());
    let mut in_class = false;
    let mut i = 0;
    while i < units.len() {
        let c = units[i];
        match c {
            0x5C if i + 1 < units.len() => {
                out.push(c);
                i += 1;
                match units[i] {
                    0x0A => out.push(u16::from(b'n')),
                    0x0D => out.push(u16::from(b'r')),
                    0x2028 => out.extend("u2028".encode_utf16()),
                    0x2029 => out.extend("u2029".encode_utf16()),
                    next => out.push(next),
                }
            }
            0x2F if !in_class => out.extend("\\/".encode_utf16()),
            0x5B => {
                in_class = true;
                out.push(c);
            }
            0x5D => {
                in_class = false;
                out.push(c);
            }
            0x0A => out.extend("\\n".encode_utf16()),
            0x0D => out.extend("\\r".encode_utf16()),
            0x2028 => out.extend("\\u2028".encode_utf16()),
            0x2029 => out.extend("\\u2029".encode_utf16()),
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// `RegExp.prototype.toString()` (§22.2.6.17).
fn to_string(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = this_object(agent, this, "toString")?;
    let key = key_of(agent, "source")?;
    let source = agent.get(o, key)?;
    let source = units_of(agent, source)?;
    let key = key_of(agent, "flags")?;
    let flags = agent.get(o, key)?;
    let flags = units_of(agent, flags)?;
    let mut out = vec![u16::from(b'/')];
    out.extend_from_slice(&source);
    out.push(u16::from(b'/'));
    out.extend_from_slice(&flags);
    make(agent, &out)
}

// ---- matching -----------------------------------------------------------------

/// Run `matcher` over the string `s` from `start`, polling the agent's
/// cancellation token and step budget.
fn run_matcher(
    agent: &mut Agent,
    matcher: &Regex,
    s: crate::runtime::StringId,
    start: usize,
    sticky: bool,
) -> JsResult<Option<Vec<u32>>> {
    let (token, mut steps) = agent.poll_state();
    let result = {
        let mut poll = || {
            if token.as_ref().is_some_and(|t| t.is_cancelled()) {
                return true;
            }
            match &mut steps {
                Some(0) => true,
                Some(left) => {
                    *left -= 1;
                    false
                }
                None => false,
            }
        };
        match agent.heap.string(s) {
            JsStr::Latin1(bytes) => matcher.search(&bytes[..], start, sticky, &mut poll),
            JsStr::Utf16(units) => matcher.search(&units[..], start, sticky, &mut poll),
        }
    };
    agent.set_steps_left(steps);
    match result {
        Ok(m) => Ok(m),
        Err(ExecError::Cancelled) => Err(Abrupt::Cancelled),
        Err(ExecError::TooComplex) => Err(agent.throw(
            ErrorKind::RangeError,
            "regular expression too complex to match",
        )),
    }
}

/// `RegExpBuiltinExec(R, S)` (§22.2.7.2).
fn builtin_exec(agent: &mut Agent, r: ObjectId, s: Value) -> JsResult<Value> {
    let sid = s.as_string().expect("the caller converted it");
    agent.heap.flatten(sid)?;
    let length = agent.heap.string(sid).len();
    let mut last_index = get_last_index(agent, r)?;
    let (matcher, _, _) = regexp_data(agent, r).expect("the caller checked");
    let flags = matcher.flags;
    let (global, sticky) = (flags.global, flags.sticky);
    if !global && !sticky {
        last_index = 0.0;
    }
    if last_index > length as f64 {
        if global || sticky {
            set_last_index(agent, r, 0.0)?;
        }
        return Ok(Value::NULL);
    }
    // `last_index` is at most the length here.
    let Some(regs) = run_matcher(agent, &matcher, sid, last_index as usize, sticky)? else {
        if global || sticky {
            set_last_index(agent, r, 0.0)?;
        }
        return Ok(Value::NULL);
    };
    let (start, end) = (regs[0] as usize, regs[1] as usize);
    if global || sticky {
        set_last_index(agent, r, end as f64)?;
    }
    build_result(agent, &matcher, sid, &regs, start)
}

/// The match array of `exec` (§22.2.7.2 steps 20 on).
fn build_result(
    agent: &mut Agent,
    matcher: &Regex,
    sid: crate::runtime::StringId,
    regs: &[u32],
    start: usize,
) -> JsResult<Value> {
    let a = agent.new_array()?;
    let key = key_of(agent, "index")?;
    agent.create_data_property_or_throw(a, key, Value::number(start as f64))?;
    let key = key_of(agent, "input")?;
    agent.create_data_property_or_throw(a, key, Value::string(sid))?;
    let groups = matcher.groups as usize;
    let mut values = Vec::with_capacity(groups + 1);
    for g in 0..=groups {
        let (s, e) = (regs[2 * g], regs[2 * g + 1]);
        let value = if s == UNSET || e == UNSET {
            Value::UNDEFINED
        } else {
            let units: Vec<u16> = agent
                .heap
                .string(sid)
                .units()
                .skip(s as usize)
                .take((e - s) as usize)
                .collect();
            make(agent, &units)?
        };
        values.push(value);
    }
    for (i, &v) in values.iter().enumerate() {
        agent.create_data_property_or_throw(a, index_key(i), v)?;
    }
    let groups_object = if matcher.names.is_empty() {
        Value::UNDEFINED
    } else {
        let o = agent.new_object_with(None)?;
        for (name, indexes) in &matcher.names {
            let value = indexes
                .iter()
                .map(|&g| values[g as usize])
                .find(|v| *v != Value::UNDEFINED)
                .unwrap_or(Value::UNDEFINED);
            let name = make(agent, name)?;
            let key = agent.to_property_key(name)?;
            agent.create_data_property_or_throw(o, key, value)?;
        }
        Value::object(o)
    };
    let key = key_of(agent, "groups")?;
    agent.create_data_property_or_throw(a, key, groups_object)?;
    if matcher.flags.has_indices {
        let indices = agent.new_array()?;
        let group_indices = if matcher.names.is_empty() {
            None
        } else {
            Some(agent.new_object_with(None)?)
        };
        let mut pairs = Vec::with_capacity(groups + 1);
        for g in 0..=groups {
            let (s, e) = (regs[2 * g], regs[2 * g + 1]);
            let pair = if s == UNSET || e == UNSET {
                Value::UNDEFINED
            } else {
                let pair = agent.new_array()?;
                agent.create_data_property_or_throw(
                    pair,
                    index_key(0),
                    Value::number(f64::from(s)),
                )?;
                agent.create_data_property_or_throw(
                    pair,
                    index_key(1),
                    Value::number(f64::from(e)),
                )?;
                Value::object(pair)
            };
            agent.create_data_property_or_throw(indices, index_key(g), pair)?;
            pairs.push(pair);
        }
        if let Some(o) = group_indices {
            for (name, indexes) in &matcher.names {
                let value = indexes
                    .iter()
                    .map(|&g| pairs[g as usize])
                    .find(|v| *v != Value::UNDEFINED)
                    .unwrap_or(Value::UNDEFINED);
                let name = make(agent, name)?;
                let key = agent.to_property_key(name)?;
                agent.create_data_property_or_throw(o, key, value)?;
            }
        }
        let key = key_of(agent, "groups")?;
        agent.create_data_property_or_throw(
            indices,
            key,
            group_indices.map_or(Value::UNDEFINED, Value::object),
        )?;
        let key = key_of(agent, "indices")?;
        agent.create_data_property_or_throw(a, key, Value::object(indices))?;
    }
    Ok(Value::object(a))
}

/// `RegExpExec(R, S)` (§22.2.7.1).
fn regexp_exec(agent: &mut Agent, r: ObjectId, s: Value) -> JsResult<Value> {
    let key = key_of(agent, "exec")?;
    let exec = agent.get(r, key)?;
    if agent.is_callable(exec) {
        let result = agent.call(exec, Value::object(r), &[s])?;
        if result != Value::NULL && result.as_object().is_none() {
            return agent.type_error("exec returned neither an object nor null");
        }
        return Ok(result);
    }
    if regexp_data(agent, r).is_none() {
        return agent.type_error("RegExpExec on an object that is not a RegExp");
    }
    builtin_exec(agent, r, s)
}

/// `RegExp.prototype.exec(string)` (§22.2.6.2).
fn exec(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = this_object(agent, this, "exec")?;
    if regexp_data(agent, o).is_none() {
        return agent.type_error("RegExp.prototype.exec called on an incompatible receiver");
    }
    let s = agent.to_string(arg(args, 0))?;
    builtin_exec(agent, o, Value::string(s))
}

/// `RegExp.prototype.test(S)` (§22.2.6.16).
fn test(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = this_object(agent, this, "test")?;
    let s = agent.to_string(arg(args, 0))?;
    let result = regexp_exec(agent, o, Value::string(s))?;
    Ok(Value::boolean(result != Value::NULL))
}

fn flags_of(agent: &mut Agent, o: ObjectId) -> JsResult<Vec<u16>> {
    let key = key_of(agent, "flags")?;
    let flags = agent.get(o, key)?;
    units_of(agent, flags)
}

/// The string at index 0 of a match result.
fn match_string(agent: &mut Agent, result: Value) -> JsResult<Vec<u16>> {
    let o = result.as_object().expect("exec results are objects");
    let v = agent.get(o, index_key(0))?;
    units_of(agent, v)
}

/// After an empty match of a global search: step `lastIndex` past it.
fn step_past_empty(agent: &mut Agent, rx: ObjectId, s: &[u16], unicode: bool) -> JsResult<()> {
    let this_index = get_last_index(agent, rx)?;
    // `this_index` is a length, so at most 2^53 - 1.
    let next = if this_index as usize >= s.len() {
        this_index + 1.0
    } else {
        advance(s, this_index as usize, unicode) as f64
    };
    set_last_index(agent, rx, next)
}

/// `RegExp.prototype[Symbol.match](string)` (§22.2.6.8).
fn symbol_match(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let rx = this_object(agent, this, "[Symbol.match]")?;
    let sid = agent.to_string(arg(args, 0))?;
    let s = Value::string(sid);
    let flags = flags_of(agent, rx)?;
    if !contains_flag(&flags, b'g') {
        return regexp_exec(agent, rx, s);
    }
    let unicode = contains_flag(&flags, b'u') || contains_flag(&flags, b'v');
    set_last_index(agent, rx, 0.0)?;
    agent.heap.flatten(sid)?;
    let units = agent.heap.string(sid).to_units();
    let a = agent.new_array()?;
    let mut n = 0;
    loop {
        let result = regexp_exec(agent, rx, s)?;
        if result == Value::NULL {
            return Ok(if n == 0 {
                Value::NULL
            } else {
                Value::object(a)
            });
        }
        let matched = match_string(agent, result)?;
        let value = make(agent, &matched)?;
        agent.create_data_property_or_throw(a, index_key(n), value)?;
        n += 1;
        if matched.is_empty() {
            step_past_empty(agent, rx, &units, unicode)?;
        }
    }
}

/// `SpeciesConstructor(O, %RegExp%)`.
fn species_constructor(agent: &mut Agent, o: ObjectId) -> JsResult<Value> {
    let default = Value::object(agent.intrinsics.regexp);
    let c = agent.get(o, PropertyKey::String(agent.names.constructor))?;
    if c == Value::UNDEFINED {
        return Ok(default);
    }
    let Some(c) = c.as_object() else {
        return agent.type_error("the constructor property is not an object");
    };
    let s = agent.get(c, PropertyKey::Symbol(agent.symbols.species))?;
    if s.is_nullish() {
        return Ok(default);
    }
    if agent.is_constructor_value(s) {
        return Ok(s);
    }
    agent.type_error("the species is not a constructor")
}

/// `RegExp.prototype[Symbol.matchAll](string)` (§22.2.6.9).
fn symbol_match_all(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let r = this_object(agent, this, "[Symbol.matchAll]")?;
    let sid = agent.to_string(arg(args, 0))?;
    let c = species_constructor(agent, r)?;
    let flags = flags_of(agent, r)?;
    let flags_value = make(agent, &flags)?;
    let matcher = agent.construct(c, &[Value::object(r), flags_value])?;
    let last_index = get_last_index(agent, r)?;
    let m = matcher
        .as_object()
        .expect("a constructor returns an object");
    set_last_index(agent, m, last_index)?;
    let global = contains_flag(&flags, b'g');
    let unicode = contains_flag(&flags, b'u') || contains_flag(&flags, b'v');
    let prototype = agent.intrinsics.regexp_string_iterator_prototype;
    let it = agent.new_object_with(Some(prototype))?;
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::Extra>())?;
    agent.heap.object_mut(it).extra_mut().internal = Some(Internal::RegExpStringIterator {
        regexp: matcher,
        string: Value::string(sid),
        global,
        unicode,
        done: false,
    });
    Ok(Value::object(it))
}

/// `%RegExpStringIteratorPrototype%.next()` (§22.2.9.2.1).
fn string_iterator_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = this
        .as_object()
        .and_then(|o| match agent.heap.object(o).internal() {
            Some(Internal::RegExpStringIterator {
                regexp,
                string,
                global,
                unicode,
                done,
            }) => Some((o, *regexp, *string, *global, *unicode, *done)),
            _ => None,
        });
    let Some((o, regexp, string, global, unicode, done)) = state else {
        return agent.type_error("not a RegExp String Iterator");
    };
    if done {
        return iterator_result(agent, Value::UNDEFINED, true);
    }
    let finish = |agent: &mut Agent| {
        if let Some(Internal::RegExpStringIterator { done, .. }) =
            agent.heap.object_mut(o).internal_mut()
        {
            *done = true;
        }
    };
    let r = regexp.as_object().expect("the matcher is an object");
    let result = regexp_exec(agent, r, string)?;
    if result == Value::NULL {
        finish(agent);
        return iterator_result(agent, Value::UNDEFINED, true);
    }
    if !global {
        finish(agent);
        return iterator_result(agent, result, false);
    }
    let matched = match_string(agent, result)?;
    if matched.is_empty() {
        let units = units_of(agent, string)?;
        step_past_empty(agent, r, &units, unicode)?;
    }
    iterator_result(agent, result, false)
}

/// `RegExp.prototype[Symbol.search](string)` (§22.2.6.12).
fn symbol_search(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let rx = this_object(agent, this, "[Symbol.search]")?;
    let sid = agent.to_string(arg(args, 0))?;
    let key = last_index_key(agent)?;
    let previous = agent.get(rx, key)?;
    if !agent.same_value(previous, Value::number(0.0)) {
        agent.set(rx, key, Value::number(0.0), true)?;
    }
    let result = regexp_exec(agent, rx, Value::string(sid))?;
    let current = agent.get(rx, key)?;
    if !agent.same_value(current, previous) {
        agent.set(rx, key, previous, true)?;
    }
    match result.as_object() {
        None => Ok(Value::number(-1.0)),
        Some(o) => {
            let key = key_of(agent, "index")?;
            agent.get(o, key)
        }
    }
}

/// `RegExp.prototype[Symbol.split](string, limit)` (§22.2.6.14).
fn symbol_split(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let rx = this_object(agent, this, "[Symbol.split]")?;
    let sid = agent.to_string(arg(args, 0))?;
    let c = species_constructor(agent, rx)?;
    let flags = flags_of(agent, rx)?;
    let unicode = contains_flag(&flags, b'u') || contains_flag(&flags, b'v');
    let mut new_flags = flags.clone();
    if !contains_flag(&flags, b'y') {
        new_flags.push(u16::from(b'y'));
    }
    let new_flags = make(agent, &new_flags)?;
    let splitter = agent.construct(c, &[Value::object(rx), new_flags])?;
    let splitter = splitter
        .as_object()
        .expect("a constructor returns an object");
    let a = agent.new_array()?;
    let limit = arg(args, 1);
    let lim = if limit == Value::UNDEFINED {
        u32::MAX
    } else {
        let n = agent.to_number(limit)?;
        to_uint32(n)
    };
    if lim == 0 {
        return Ok(Value::object(a));
    }
    let s = Value::string(sid);
    agent.heap.flatten(sid)?;
    let units = agent.heap.string(sid).to_units();
    let size = units.len();
    if size == 0 {
        let z = regexp_exec(agent, splitter, s)?;
        if z == Value::NULL {
            agent.create_data_property_or_throw(a, index_key(0), s)?;
        }
        return Ok(Value::object(a));
    }
    let mut length_a: u32 = 0;
    let (mut p, mut q) = (0usize, 0usize);
    while q < size {
        set_last_index(agent, splitter, q as f64)?;
        let z = regexp_exec(agent, splitter, s)?;
        let Some(z) = z.as_object() else {
            q = advance(&units, q, unicode);
            continue;
        };
        let e = get_last_index(agent, splitter)?.min(size as f64) as usize;
        if e == p {
            q = advance(&units, q, unicode);
            continue;
        }
        let t = make(agent, &units[p..q])?;
        agent.create_data_property_or_throw(a, index_key(length_a as usize), t)?;
        length_a += 1;
        if length_a == lim {
            return Ok(Value::object(a));
        }
        p = e;
        let n = agent.length_of_array_like(z)?;
        let captures = (n - 1.0).max(0.0) as usize;
        for i in 1..=captures {
            let next = agent.get(z, index_key(i))?;
            agent.create_data_property_or_throw(a, index_key(length_a as usize), next)?;
            length_a += 1;
            if length_a == lim {
                return Ok(Value::object(a));
            }
        }
        q = p;
    }
    let t = make(agent, &units[p.min(size)..])?;
    agent.create_data_property_or_throw(a, index_key(length_a as usize), t)?;
    Ok(Value::object(a))
}

/// `GetSubstitution(matched, str, position, captures, namedCaptures,
/// replacementTemplate)` (§22.1.3.19.1).
pub(crate) fn get_substitution(
    agent: &mut Agent,
    matched: &[u16],
    s: &[u16],
    position: usize,
    captures: &[Option<Vec<u16>>],
    named: Option<ObjectId>,
    template: &[u16],
) -> JsResult<Vec<u16>> {
    let digit = |u: u16| (0x30..=0x39).contains(&u).then(|| usize::from(u - 0x30));
    let m = captures.len();
    let mut out = Vec::with_capacity(template.len());
    let mut i = 0;
    while i < template.len() {
        let c = template[i];
        if c != u16::from(b'$') || i + 1 >= template.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let next = template[i + 1];
        match u8::try_from(next).unwrap_or(0) {
            b'$' => {
                out.push(c);
                i += 2;
            }
            b'&' => {
                out.extend_from_slice(matched);
                i += 2;
            }
            b'`' => {
                out.extend_from_slice(&s[..position.min(s.len())]);
                i += 2;
            }
            b'\'' => {
                let tail = (position + matched.len()).min(s.len());
                out.extend_from_slice(&s[tail..]);
                i += 2;
            }
            b'0'..=b'9' => {
                let first = digit(next).expect("a digit");
                let second = template.get(i + 2).copied().and_then(digit);
                let (index, length) = match second {
                    Some(d) if (1..=m).contains(&(first * 10 + d)) => (first * 10 + d, 3),
                    _ => (first, 2),
                };
                if (1..=m).contains(&index) {
                    if let Some(capture) = &captures[index - 1] {
                        out.extend_from_slice(capture);
                    }
                    i += length;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            b'<' => {
                let close = template[i + 2..].iter().position(|&u| u == u16::from(b'>'));
                match (named, close) {
                    (Some(groups), Some(offset)) => {
                        let name = &template[i + 2..i + 2 + offset];
                        let name = make(agent, name)?;
                        let key = agent.to_property_key(name)?;
                        let capture = agent.get(groups, key)?;
                        if capture != Value::UNDEFINED {
                            out.extend(units_of(agent, capture)?);
                        }
                        i += 3 + offset;
                    }
                    _ => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// `RegExp.prototype[Symbol.replace](string, replaceValue)` (§22.2.6.11).
fn symbol_replace(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let rx = this_object(agent, this, "[Symbol.replace]")?;
    let sid = agent.to_string(arg(args, 0))?;
    let s = Value::string(sid);
    agent.heap.flatten(sid)?;
    let units = agent.heap.string(sid).to_units();
    let length_s = units.len();
    let mut replace_value = arg(args, 1);
    let functional = agent.is_callable(replace_value);
    let template = if functional {
        None
    } else {
        let t = agent.to_string(replace_value)?;
        replace_value = Value::string(t);
        Some(agent.heap.string(t).to_units())
    };
    let flags = flags_of(agent, rx)?;
    let global = contains_flag(&flags, b'g');
    let unicode = contains_flag(&flags, b'u') || contains_flag(&flags, b'v');
    if global {
        set_last_index(agent, rx, 0.0)?;
    }
    // The results stay reachable from this array while script runs.
    let results = agent.new_array()?;
    let mut count = 0;
    loop {
        let result = regexp_exec(agent, rx, s)?;
        if result == Value::NULL {
            break;
        }
        agent.create_data_property_or_throw(results, index_key(count), result)?;
        count += 1;
        if !global {
            break;
        }
        let matched = match_string(agent, result)?;
        if matched.is_empty() {
            step_past_empty(agent, rx, &units, unicode)?;
        }
    }
    let mut accumulated: Vec<u16> = Vec::new();
    let mut next_source = 0;
    for i in 0..count {
        let result = agent.get(results, index_key(i))?;
        let result = result.as_object().expect("exec results are objects");
        let n = agent.length_of_array_like(result)?;
        let n_captures = (n - 1.0).max(0.0) as usize;
        let matched_value = agent.get(result, index_key(0))?;
        let matched = units_of(agent, matched_value)?;
        let key = key_of(agent, "index")?;
        let position = agent.get(result, key)?;
        let position = agent.to_number(position)?;
        let position = to_integer_or_infinity(position).clamp(0.0, length_s as f64) as usize;
        let mut captures = Vec::with_capacity(n_captures);
        let mut capture_values = Vec::with_capacity(n_captures);
        for k in 1..=n_captures {
            let cap = agent.get(result, index_key(k))?;
            if cap == Value::UNDEFINED {
                captures.push(None);
                capture_values.push(Value::UNDEFINED);
            } else {
                let sid = agent.to_string(cap)?;
                captures.push(Some(agent.heap.string(sid).to_units()));
                capture_values.push(Value::string(sid));
            }
        }
        let key = key_of(agent, "groups")?;
        let named = agent.get(result, key)?;
        let replacement = match &template {
            None => {
                let matched_value = make(agent, &matched)?;
                let mut call_args = Vec::with_capacity(n_captures + 4);
                call_args.push(matched_value);
                call_args.extend_from_slice(&capture_values);
                call_args.push(Value::number(position as f64));
                call_args.push(s);
                if named != Value::UNDEFINED {
                    call_args.push(named);
                }
                let v = agent.call(replace_value, Value::UNDEFINED, &call_args)?;
                units_of(agent, v)?
            }
            Some(template) => {
                let named = if named == Value::UNDEFINED {
                    None
                } else {
                    Some(agent.to_object(named)?)
                };
                get_substitution(
                    agent, &matched, &units, position, &captures, named, template,
                )?
            }
        };
        if position >= next_source {
            accumulated.extend_from_slice(&units[next_source..position]);
            accumulated.extend_from_slice(&replacement);
            next_source = position + matched.len();
            agent.heap.charge(2 * replacement.len())?;
        }
    }
    if next_source < length_s {
        accumulated.extend_from_slice(&units[next_source..]);
    }
    make(agent, &accumulated)
}

/// `RegExpCreate(regexp, flags)` then `Invoke(rx, symbol, «s»)`, for the
/// String methods given a pattern that is not a RegExp.
pub(crate) fn create_and_invoke(
    agent: &mut Agent,
    pattern: Value,
    flags: Option<&str>,
    symbol: crate::runtime::SymbolId,
    s: Value,
) -> JsResult<Value> {
    let flags = match flags {
        Some(f) => agent.string(f)?,
        None => Value::UNDEFINED,
    };
    let rx = regexp_create(agent, pattern, flags)?;
    let o = rx.as_object().expect("a RegExp is an object");
    let method = agent.get(o, PropertyKey::Symbol(symbol))?;
    agent.call(method, rx, &[s])
}
