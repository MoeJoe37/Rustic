//! Function properties of the global object (§19.2) and the
//! constructors of the primitive wrappers (`Boolean`, `Number`,
//! `String`, `Symbol`), with only what their prototypes already define.

use super::{Method, arg, install_constructor, install_methods, method, prototype_from_new_target};
use crate::lexer::radix_to_f64;
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::number::is_str_whitespace;
use crate::runtime::object::{Class, Object, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{Kind, ObjectId, Value};

const GLOBAL_FUNCTIONS: &[Method] = &[
    method("isFinite", is_finite, 1),
    method("isNaN", is_nan, 1),
    method("parseFloat", parse_float, 1),
    method("parseInt", parse_int, 2),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let i = agent.intrinsics;
    install_methods(agent, i.global, GLOBAL_FUNCTIONS)?;
    let eval_key = agent.heap.key("eval")?;
    let eval = agent.new_native_function(eval, "eval", 1)?;
    agent.define_property_or_throw(
        i.global,
        eval_key,
        PropertyDescriptor::data_with(Value::object(eval), true, false, true),
    )?;
    agent.intrinsics.eval = eval;
    install_constructor(
        agent,
        "Boolean",
        boolean_constructor,
        1,
        i.boolean_prototype,
    )?;
    install_constructor(agent, "Number", number_constructor, 1, i.number_prototype)?;
    install_constructor(agent, "String", string_constructor, 1, i.string_prototype)?;
    let symbol = install_constructor(agent, "Symbol", symbol_constructor, 0, i.symbol_prototype)?;
    install_symbol_statics(agent, symbol)
}

/// `eval(x)` (§19.2.1) reached as a function value: an indirect eval.
fn eval(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    agent.indirect_eval(arg(args, 0))
}

const SYMBOL_STATICS: &[Method] = &[method("for", symbol_for, 1), method("keyFor", key_for, 1)];

/// The well-known symbols as properties of `Symbol` (§20.4.2), `for`,
/// `keyFor` and `Symbol.prototype.description` (§20.4.3.2).
fn install_symbol_statics(agent: &mut Agent, symbol: ObjectId) -> JsResult<()> {
    install_methods(agent, symbol, SYMBOL_STATICS)?;
    let s = agent.symbols;
    for (name, id) in [
        ("asyncIterator", s.async_iterator),
        ("hasInstance", s.has_instance),
        ("isConcatSpreadable", s.is_concat_spreadable),
        ("iterator", s.iterator),
        ("match", s.match_),
        ("matchAll", s.match_all),
        ("replace", s.replace),
        ("search", s.search),
        ("species", s.species),
        ("split", s.split),
        ("toPrimitive", s.to_primitive),
        ("toStringTag", s.to_string_tag),
        ("unscopables", s.unscopables),
    ] {
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            symbol,
            key,
            PropertyDescriptor::data_with(Value::symbol(id), false, false, false),
        )?;
    }
    let getter = agent.new_native_function(symbol_description, "get description", 0)?;
    let key = agent.heap.key("description")?;
    let prototype = agent.intrinsics.symbol_prototype;
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )?;
    let tag = agent.string("Symbol")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )
}

/// `Symbol.for(key)` (§20.4.2.2).
fn symbol_for(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let key = agent.to_string(arg(args, 0))?;
    let registry = agent.intrinsics.symbol_registry;
    let property = agent.to_property_key(Value::string(key))?;
    let found = agent.get(registry, property)?;
    if found != Value::UNDEFINED {
        return Ok(found);
    }
    let symbol = Value::symbol(agent.heap.alloc_registered_symbol(key)?);
    agent.create_data_property_or_throw(registry, property, symbol)?;
    Ok(symbol)
}

/// `Symbol.keyFor(sym)` (§20.4.2.6).
fn key_for(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Kind::Symbol(symbol) = arg(args, 0).kind() else {
        return agent.type_error("Symbol.keyFor: argument is not a symbol");
    };
    let record = agent.heap.symbol(symbol);
    match (record.registered, record.description) {
        (true, Some(key)) => Ok(Value::string(key)),
        _ => Ok(Value::UNDEFINED),
    }
}

/// `get Symbol.prototype.description` (§20.4.3.2).
fn symbol_description(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let symbol = match this.kind() {
        Kind::Symbol(s) => Some(s),
        Kind::Object(o) if agent.class(o) == Class::Symbol => {
            agent.primitive_of(o).and_then(|p| match p.kind() {
                Kind::Symbol(s) => Some(s),
                _ => None,
            })
        }
        _ => None,
    };
    let Some(symbol) = symbol else {
        return agent.type_error("Symbol.prototype.description called on a non-symbol");
    };
    Ok(agent
        .heap
        .symbol(symbol)
        .description
        .map_or(Value::UNDEFINED, Value::string))
}

fn is_finite(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = agent.to_number(arg(args, 0))?;
    Ok(Value::boolean(n.is_finite()))
}

fn is_nan(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = agent.to_number(arg(args, 0))?;
    Ok(Value::boolean(n.is_nan()))
}

/// The string's code units without leading StrWhiteSpaceChar.
fn trimmed_start(agent: &mut Agent, value: Value) -> JsResult<Vec<u16>> {
    let s = agent.to_string(value)?;
    let units = agent.heap.string(s).to_units();
    let start = units
        .iter()
        .position(|&u| !is_str_whitespace(u))
        .unwrap_or(units.len());
    Ok(units[start..].to_vec())
}

/// `parseFloat(string)` (§19.2.4): the longest prefix that is a
/// StrDecimalLiteral.
fn parse_float(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let units = trimmed_start(agent, arg(args, 0))?;
    let text: String = units
        .iter()
        .map_while(|&u| u8::try_from(u).ok().filter(u8::is_ascii).map(char::from))
        .collect();
    let b = text.as_bytes();
    let mut i = 0;
    let mut sign = 1.0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        if b[i] == b'-' {
            sign = -1.0;
        }
        i += 1;
    }
    if text[i..].starts_with("Infinity") {
        return Ok(Value::number(sign * f64::INFINITY));
    }
    let start = i;
    let int_digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        frac_digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if int_digits > 0 || frac_digits > 0 {
            i += 1 + frac_digits;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return Ok(Value::NAN);
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_digits = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp_digits > 0 {
            i = j + exp_digits;
        }
    }
    let n = text[start..i].parse::<f64>().unwrap_or(f64::NAN);
    Ok(Value::number(sign * n))
}

/// `parseInt(string, radix)` (§19.2.5).
fn parse_int(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let units = trimmed_start(agent, arg(args, 0))?;
    let mut s: &[u16] = &units;
    let mut sign = 1.0;
    if let Some(&first) = s.first() {
        if first == u16::from(b'-') {
            sign = -1.0;
        }
        if first == u16::from(b'-') || first == u16::from(b'+') {
            s = &s[1..];
        }
    }
    let r = agent.to_number(arg(args, 1))?;
    // ToInt32 (§7.1.6).
    let mut r = if r.is_finite() {
        (r.trunc().rem_euclid(4_294_967_296.0) as u32) as i32
    } else {
        0
    };
    let mut strip_prefix = true;
    if r != 0 {
        if !(2..=36).contains(&r) {
            return Ok(Value::NAN);
        }
        if r != 16 {
            strip_prefix = false;
        }
    } else {
        r = 10;
    }
    if strip_prefix
        && s.len() >= 2
        && s[0] == u16::from(b'0')
        && (s[1] == u16::from(b'x') || s[1] == u16::from(b'X'))
    {
        s = &s[2..];
        r = 16;
    }
    let radix = r as u32;
    let digits: String = s
        .iter()
        .map_while(|&u| char::from_u32(u32::from(u)).filter(|c| c.is_ascii() && c.is_digit(radix)))
        .collect();
    if digits.is_empty() {
        return Ok(Value::NAN);
    }
    let n = match radix {
        // Exact decimal rounding, as implementations do for radix 10.
        10 => digits.parse::<f64>().unwrap_or(f64::NAN),
        2 | 8 | 16 => radix_to_f64(&digits, radix),
        // Other radixes may be approximated past 20 digits (§19.2.5
        // step 16); accumulation is exact below 2^53.
        _ => digits.chars().fold(0.0, |n, c| {
            n * f64::from(radix) + f64::from(c.to_digit(radix).expect("filtered digits"))
        }),
    };
    Ok(Value::number(sign * n))
}

/// A wrapper object of `class` for `primitive` whose prototype comes from
/// the new target.
fn wrapper(
    agent: &mut Agent,
    class: Class,
    primitive: Value,
    default: ObjectId,
) -> JsResult<Value> {
    let prototype = prototype_from_new_target(agent, default)?;
    let mut object = Object::new(class, Some(prototype));
    object.extra_mut().primitive = Some(primitive);
    let o = agent.heap.alloc_object(object)?;
    if class == Class::String {
        let length = agent
            .heap
            .string(primitive.as_string().expect("a string primitive"))
            .len();
        let key = crate::runtime::PropertyKey::String(agent.names.length);
        agent.define_property_or_throw(
            o,
            key,
            crate::runtime::PropertyDescriptor::data_with(
                Value::number(length as f64),
                false,
                false,
                false,
            ),
        )?;
    }
    Ok(Value::object(o))
}

/// `Boolean(value)` (§20.3.1.1).
fn boolean_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let b = Value::boolean(agent.to_boolean(arg(args, 0)));
    if agent.new_target().is_none() {
        return Ok(b);
    }
    let default = agent.intrinsics.boolean_prototype;
    wrapper(agent, Class::Boolean, b, default)
}

/// `Number(value)` (§21.1.1.1). A BigInt converts to its value.
fn number_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = match args.first() {
        None => 0.0,
        Some(&v) => {
            let prim = agent.to_numeric(v)?;
            match prim {
                crate::runtime::Numeric::Number(n) => n,
                crate::runtime::Numeric::BigInt(b) => b.to_f64(),
            }
        }
    };
    let n = Value::number(n);
    if agent.new_target().is_none() {
        return Ok(n);
    }
    let default = agent.intrinsics.number_prototype;
    wrapper(agent, Class::Number, n, default)
}

/// `String(value)` (§22.1.1.1). A Symbol prints as its descriptive
/// string when called.
fn string_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let s = match args.first() {
        None => Value::string(agent.names.empty),
        Some(&v) => {
            if agent.new_target().is_none() {
                if let Kind::Symbol(symbol) = v.kind() {
                    let description = agent.heap.symbol(symbol).description;
                    let text = description
                        .map(|d| agent.heap.string(d).to_text())
                        .unwrap_or_default();
                    return agent.string(&format!("Symbol({text})"));
                }
            }
            Value::string(agent.to_string(v)?)
        }
    };
    if agent.new_target().is_none() {
        return Ok(s);
    }
    let default = agent.intrinsics.string_prototype;
    wrapper(agent, Class::String, s, default)
}

/// `Symbol(description)` (§20.4.1.1). Not a constructor for `new`.
fn symbol_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_some() {
        return agent.type_error("Symbol is not a constructor");
    }
    let description = arg(args, 0);
    let description = if description == Value::UNDEFINED {
        None
    } else {
        Some(agent.to_string(description)?)
    };
    Ok(Value::symbol(agent.heap.alloc_symbol(description)?))
}
