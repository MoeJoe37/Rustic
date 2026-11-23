//! The `Math` object (§21.3).
//!
//! The transcendental functions use the platform's `libm` through
//! `f64`, which the specification allows to be approximate; results on
//! different platforms may differ in the last bit.

mod exact_sum;

use super::{Method, arg, install_methods, iterate, method};
use crate::runtime::agent::{Agent, ErrorKind, JsResult};
use crate::runtime::number::to_uint32;
use crate::runtime::object::PropertyDescriptor;
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::Value;

/// A `Math` function of one Number.
macro_rules! unary {
    ($($name:ident => $f:expr;)*) => {
        $(
            fn $name(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
                let x = agent.to_number(arg(args, 0))?;
                let f: fn(f64) -> f64 = $f;
                Ok(Value::number(f(x)))
            }
        )*
    };
}

unary! {
    abs => f64::abs;
    acos => f64::acos;
    acosh => f64::acosh;
    asin => f64::asin;
    asinh => |x| if x == 0.0 { x } else { x.asinh() };
    atan => f64::atan;
    atanh => |x| if x == 0.0 { x } else { x.atanh() };
    cbrt => f64::cbrt;
    ceil => f64::ceil;
    cos => f64::cos;
    cosh => f64::cosh;
    exp => f64::exp;
    expm1 => |x| if x == 0.0 { x } else { x.exp_m1() };
    floor => f64::floor;
    fround => |x| f64::from(x as f32);
    f16round => |x| {
        super::buffer::f16_bits_to_f64(super::buffer::f64_to_f16_bits(x))
    };
    log => f64::ln;
    log1p => |x| if x == 0.0 { x } else { x.ln_1p() };
    log10 => f64::log10;
    log2 => f64::log2;
    round => round_half_up;
    sign => |x| if x.is_nan() || x == 0.0 { x } else { x.signum() };
    sin => f64::sin;
    sinh => |x| if x == 0.0 { x } else { x.sinh() };
    sqrt => f64::sqrt;
    tan => f64::tan;
    tanh => |x| if x == 0.0 { x } else { x.tanh() };
    trunc => f64::trunc;
}

const FUNCTIONS: &[Method] = &[
    method("abs", abs, 1),
    method("acos", acos, 1),
    method("acosh", acosh, 1),
    method("asin", asin, 1),
    method("asinh", asinh, 1),
    method("atan", atan, 1),
    method("atanh", atanh, 1),
    method("atan2", atan2, 2),
    method("cbrt", cbrt, 1),
    method("ceil", ceil, 1),
    method("clz32", clz32, 1),
    method("cos", cos, 1),
    method("cosh", cosh, 1),
    method("exp", exp, 1),
    method("expm1", expm1, 1),
    method("floor", floor, 1),
    method("fround", fround, 1),
    method("f16round", f16round, 1),
    method("hypot", hypot, 2),
    method("imul", imul, 2),
    method("log", log, 1),
    method("log1p", log1p, 1),
    method("log10", log10, 1),
    method("log2", log2, 1),
    method("max", max, 2),
    method("min", min, 2),
    method("pow", pow, 2),
    method("random", random, 0),
    method("round", round, 1),
    method("sign", sign, 1),
    method("sin", sin, 1),
    method("sinh", sinh, 1),
    method("sqrt", sqrt, 1),
    method("sumPrecise", sum_precise, 1),
    method("tan", tan, 1),
    method("tanh", tanh, 1),
    method("trunc", trunc, 1),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let math = agent.new_object_with(Some(object_prototype))?;
    install_methods(agent, math, FUNCTIONS)?;
    for (name, value) in [
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("PI", std::f64::consts::PI),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            math,
            key,
            PropertyDescriptor::data_with(Value::number(value), false, false, false),
        )?;
    }
    let tag = agent.string("Math")?;
    agent.define_property_or_throw(
        math,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )?;
    let global = agent.intrinsics.global;
    let key = agent.heap.key("Math")?;
    agent.define_property_or_throw(
        global,
        key,
        PropertyDescriptor::data_with(Value::object(math), true, false, true),
    )
}

/// `Math.round` (§21.3.2.28): halves round up, and results in [-0.5, 0)
/// are -0. Computed without `x + 0.5`, which rounds wrongly just below
/// one half.
fn round_half_up(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    if (-0.5..0.0).contains(&x) {
        return -0.0;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// The Numbers of all arguments, converted in order.
fn numbers(agent: &mut Agent, args: &[Value]) -> JsResult<Vec<f64>> {
    let mut out = Vec::with_capacity(args.len());
    for &a in args {
        out.push(agent.to_number(a)?);
    }
    Ok(out)
}

fn atan2(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let y = agent.to_number(arg(args, 0))?;
    let x = agent.to_number(arg(args, 1))?;
    Ok(Value::number(y.atan2(x)))
}

fn clz32(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let n = agent.to_number(arg(args, 0))?;
    Ok(Value::number(f64::from(to_uint32(n).leading_zeros())))
}

fn imul(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let a = agent.to_number(arg(args, 0))?;
    let b = agent.to_number(arg(args, 1))?;
    let product = to_uint32(a).wrapping_mul(to_uint32(b)) as i32;
    Ok(Value::number(f64::from(product)))
}

/// `Math.hypot` (§21.3.2.18): scaled, so large and small arguments do
/// not overflow or underflow.
fn hypot(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let values = numbers(agent, args)?;
    if values.iter().any(|v| v.is_infinite()) {
        return Ok(Value::number(f64::INFINITY));
    }
    if values.iter().any(|v| v.is_nan()) {
        return Ok(Value::NAN);
    }
    let largest = values.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if largest == 0.0 {
        return Ok(Value::number(0.0));
    }
    let sum: f64 = values.iter().map(|v| (v / largest) * (v / largest)).sum();
    Ok(Value::number(largest * sum.sqrt()))
}

/// `Math.sumPrecise` (§21.3.2.34).
fn sum_precise(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let items = agent.require_object_coercible(arg(args, 0))?;
    let key = PropertyKey::Symbol(agent.symbols.iterator);
    let Some(method) = agent.get_method(items, key)? else {
        return agent.type_error("Math.sumPrecise requires an iterable");
    };
    let mut count = 0;
    let mut sum = exact_sum::ExactSum::new();
    let mut non_finite = 0.0_f64;
    let mut all_negative_zero = true;
    iterate(agent, items, method, |agent, value| {
        agent.check_native_work()?;
        if count == exact_sum::MAX_TERMS {
            return Err(agent.throw(ErrorKind::RangeError, "Math.sumPrecise has too many values"));
        }
        count += 1;
        let Some(number) = value.as_number() else {
            return agent.type_error("Math.sumPrecise requires Number values");
        };
        // Keep iterating even after a non-finite input. The remaining iterator
        // steps and type checks are observable.
        if !number.is_finite() {
            non_finite += number;
        } else if non_finite.is_finite() && number.to_bits() != (-0.0_f64).to_bits() {
            all_negative_zero = false;
            sum.add(number);
        }
        Ok(true)
    })?;
    let result = if !non_finite.is_finite() {
        non_finite
    } else if all_negative_zero {
        -0.0
    } else {
        sum.finish()
    };
    Ok(Value::number(result))
}

/// `Math.max` and `Math.min` (§21.3.2.24, §21.3.2.25): NaN wins, and
/// +0 is larger than -0.
fn extreme(agent: &mut Agent, args: &[Value], larger: bool) -> JsResult<Value> {
    let values = numbers(agent, args)?;
    let mut best = if larger {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    };
    for v in values {
        if v.is_nan() {
            return Ok(Value::NAN);
        }
        // Zeros compare equal, so the sign decides between them.
        let better = if larger {
            v > best || (v == 0.0 && best == 0.0 && best.is_sign_negative())
        } else {
            v < best || (v == 0.0 && best == 0.0 && v.is_sign_negative())
        };
        if better {
            best = v;
        }
    }
    Ok(Value::number(best))
}

fn max(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    extreme(agent, args, true)
}

fn min(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    extreme(agent, args, false)
}

fn pow(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let base = agent.to_number(arg(args, 0))?;
    let exponent = agent.to_number(arg(args, 1))?;
    Ok(Value::number(crate::vm::exponentiate(base, exponent)))
}

/// `Math.random` (§21.3.2.27): xorshift64* with 53 random bits. It is
/// not cryptographic, as in every browser; `crypto.getRandomValues` is
/// the host's.
fn random(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let mut x = agent.vm.random_state;
    if x == 0 {
        x = clock_seed() | 1;
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    agent.vm.random_state = x;
    let bits = x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
    Ok(Value::number(bits as f64 / (1u64 << 53) as f64))
}

/// A seed from the clock and an address, mixed (splitmix64).
fn clock_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let marker = 0u8;
    let address = std::ptr::from_ref(&marker) as usize as u64;
    let mut z = nanos ^ address.rotate_left(32);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
