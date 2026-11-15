//! BigInt (§21.2) and the conversions to it: `ToBigInt`, `ToBigInt64`,
//! `ToBigUint64` and `ToIndex` (§7.1.13–§7.1.22), plus the operators of
//! §6.1.6.2 as the interpreter calls them.

use super::{arg, install_constructor, install_methods, method};
use crate::runtime::agent::{Agent, ErrorKind, JsResult};
use crate::runtime::bigint::{BigInt, TooBig};
use crate::runtime::number::to_integer_or_infinity;
use crate::runtime::object::{Class, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::value::{Kind, Value};
use crate::runtime::{Numeric, PreferredType, string_to_bigint};

/// The largest value `ToIndex` accepts, 2⁵³ - 1.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let prototype = agent.intrinsics.bigint_prototype;
    let constructor = install_constructor(agent, "BigInt", bigint_constructor, 1, prototype)?;
    install_methods(
        agent,
        constructor,
        &[
            method("asIntN", bigint_as_int_n, 2),
            method("asUintN", bigint_as_uint_n, 2),
        ],
    )?;
    install_methods(
        agent,
        prototype,
        &[
            method("toString", bigint_to_string, 0),
            method("toLocaleString", bigint_to_locale_string, 0),
            method("valueOf", bigint_value_of, 0),
        ],
    )?;
    let tag = agent.string("BigInt")?;
    agent.define_property_or_throw(
        prototype,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )
}

impl Agent {
    /// A BigInt value holding `b`.
    pub fn bigint_value(&mut self, b: BigInt) -> JsResult<Value> {
        Ok(Value::bigint(self.heap.alloc_bigint(b)?))
    }

    /// The result of an operation that may exceed the size limit.
    pub(crate) fn bigint_result(&mut self, result: Result<BigInt, TooBig>) -> JsResult<Value> {
        match result {
            Ok(b) => self.bigint_value(b),
            Err(TooBig) => Err(self.throw(ErrorKind::RangeError, "Maximum BigInt size exceeded")),
        }
    }

    /// `ToBigInt(value)` (§7.1.13).
    pub fn to_bigint(&mut self, value: Value) -> JsResult<BigInt> {
        let primitive = self.to_primitive(value, Some(PreferredType::Number))?;
        match primitive.kind() {
            Kind::BigInt(b) => Ok(self.heap.bigint(b).clone()),
            Kind::Boolean(b) => Ok(BigInt::from_u64(u64::from(b))),
            Kind::String(s) => match string_to_bigint(self.heap.string(s)) {
                Some(b) => Ok(b),
                None => Err(self.throw(
                    ErrorKind::SyntaxError,
                    "cannot convert the string to a BigInt",
                )),
            },
            Kind::Number(_) => self.type_error("cannot convert a Number to a BigInt"),
            Kind::Symbol(_) => self.type_error("cannot convert a Symbol to a BigInt"),
            _ => self.type_error("cannot convert undefined or null to a BigInt"),
        }
    }

    /// `ToBigInt64(value)` (§7.1.15).
    pub fn to_bigint64(&mut self, value: Value) -> JsResult<i64> {
        Ok(self.to_bigint(value)?.to_i64_wrapping())
    }

    /// `ToBigUint64(value)` (§7.1.16).
    pub fn to_biguint64(&mut self, value: Value) -> JsResult<u64> {
        Ok(self.to_bigint(value)?.to_u64_wrapping())
    }

    /// `ToIndex(value)` (§7.1.22): an integer from 0 to 2⁵³ - 1, or a
    /// RangeError.
    pub fn to_index(&mut self, value: Value) -> JsResult<u64> {
        let number = self.to_number(value)?;
        let integer = to_integer_or_infinity(number);
        if !(0.0..=MAX_SAFE_INTEGER).contains(&integer) {
            return Err(self.throw(ErrorKind::RangeError, "index is out of range"));
        }
        // An integer in the checked range converts exactly.
        Ok(integer as u64)
    }

    /// `NumberToBigInt(number)` (§21.2.1.1.1).
    pub(crate) fn number_to_bigint(&mut self, number: f64) -> JsResult<BigInt> {
        match BigInt::from_f64(number) {
            Some(b) => Ok(b),
            None => Err(self.throw(
                ErrorKind::RangeError,
                "the number is not an integer and cannot become a BigInt",
            )),
        }
    }

    /// A unary operator on a BigInt (§6.1.6.2.1, .2 and `++`/`--`).
    pub(crate) fn bigint_unary(&mut self, op: BigIntUnary, x: &BigInt) -> JsResult<Value> {
        let one = BigInt::from_u64(1);
        let result = match op {
            BigIntUnary::Neg => Ok(x.neg()),
            BigIntUnary::Not => Ok(x.bit_not()),
            BigIntUnary::Inc => x.add(&one),
            BigIntUnary::Dec => x.sub(&one),
        };
        self.bigint_result(result)
    }

    /// A binary operator on two BigInts (§6.1.6.2.3–.20).
    pub(crate) fn bigint_binary(
        &mut self,
        op: BigIntBinary,
        x: &BigInt,
        y: &BigInt,
    ) -> JsResult<Value> {
        let result = match op {
            BigIntBinary::Add => x.add(y),
            BigIntBinary::Sub => x.sub(y),
            BigIntBinary::Mul => x.mul(y),
            BigIntBinary::Div | BigIntBinary::Rem => match x.div_rem(y) {
                Some((q, r)) => Ok(if op == BigIntBinary::Div { q } else { r }),
                None => return Err(self.throw(ErrorKind::RangeError, "BigInt division by zero")),
            },
            BigIntBinary::Exp => match x.pow(y) {
                Some(result) => result,
                None => {
                    return Err(self.throw(
                        ErrorKind::RangeError,
                        "BigInt exponent must not be negative",
                    ));
                }
            },
            BigIntBinary::Shl => x.shl(y),
            BigIntBinary::Sar => x.shr(y),
            BigIntBinary::Shr => {
                return self.type_error("BigInts have no unsigned right shift, use >> instead");
            }
            BigIntBinary::And => Ok(x.bit_and(y)),
            BigIntBinary::Or => Ok(x.bit_or(y)),
            BigIntBinary::Xor => Ok(x.bit_xor(y)),
        };
        self.bigint_result(result)
    }
}

/// The unary operators BigInts support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BigIntUnary {
    Neg,
    Not,
    Inc,
    Dec,
}

/// The binary operators BigInts support (`>>>` always throws).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BigIntBinary {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Exp,
    Shl,
    Sar,
    Shr,
    And,
    Or,
    Xor,
}

/// `BigInt(value)` (§21.2.1.1). Not usable with `new`.
fn bigint_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    if agent.new_target().is_some() {
        return agent.type_error("BigInt is not a constructor");
    }
    let primitive = agent.to_primitive(arg(args, 0), Some(PreferredType::Number))?;
    let big = match primitive.as_number() {
        Some(number) => agent.number_to_bigint(number)?,
        None => agent.to_bigint(primitive)?,
    };
    agent.bigint_value(big)
}

/// `thisBigIntValue` (§21.2.3.4.1).
fn this_bigint(agent: &mut Agent, this: Value) -> JsResult<Value> {
    if matches!(this.kind(), Kind::BigInt(_)) {
        return Ok(this);
    }
    if let Some(o) = this.as_object() {
        if agent.class(o) == Class::BigInt {
            if let Some(p) = agent.primitive_of(o) {
                return Ok(p);
            }
        }
    }
    agent.type_error("method called on an incompatible receiver")
}

/// `BigInt.prototype.valueOf` (§21.2.3.4).
fn bigint_value_of(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    this_bigint(agent, this)
}

/// `BigInt.prototype.toLocaleString(locales, options)` (ECMA-402 §19.3.1).
fn bigint_to_locale_string(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let value = this_bigint(agent, this)?;
    super::intl::number_to_locale_string(agent, value, arg(args, 0), arg(args, 1))
}

/// `BigInt.prototype.toString(radix)` (§21.2.3.3).
fn bigint_to_string(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let value = this_bigint(agent, this)?;
    let radix = arg(args, 0);
    let radix = if radix == Value::UNDEFINED {
        10.0
    } else {
        let n = agent.to_number(radix)?;
        to_integer_or_infinity(n)
    };
    if !(2.0..=36.0).contains(&radix) {
        return Err(agent.throw(ErrorKind::RangeError, "radix must be between 2 and 36"));
    }
    let Kind::BigInt(id) = value.kind() else {
        unreachable!("thisBigIntValue returns a BigInt");
    };
    // The radix was just checked to be a small integer.
    let text = agent.heap.bigint(id).to_string_radix(radix as u32);
    agent.string(&text)
}

/// `BigInt.asIntN(bits, bigint)` (§21.2.2.1).
fn bigint_as_int_n(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let bits = agent.to_index(arg(args, 0))?;
    let big = agent.to_bigint(arg(args, 1))?;
    let result = big.as_int_n(bits);
    agent.bigint_result(result)
}

/// `BigInt.asUintN(bits, bigint)` (§21.2.2.2).
fn bigint_as_uint_n(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let bits = agent.to_index(arg(args, 0))?;
    let big = agent.to_bigint(arg(args, 1))?;
    let result = big.as_uint_n(bits);
    agent.bigint_result(result)
}

/// The operands of a numeric binary operator after `ToNumeric`: both
/// Numbers, both BigInts, or a TypeError for a mix (§13.15.3 step 5).
pub(crate) fn same_numeric_types(
    agent: &mut Agent,
    x: Numeric,
    y: Numeric,
) -> JsResult<SameNumeric> {
    match (x, y) {
        (Numeric::Number(a), Numeric::Number(b)) => Ok(SameNumeric::Numbers(a, b)),
        (Numeric::BigInt(a), Numeric::BigInt(b)) => Ok(SameNumeric::BigInts(a, b)),
        _ => agent.type_error("cannot mix BigInt and other types, use explicit conversions"),
    }
}

/// Two operands of one numeric type.
pub(crate) enum SameNumeric {
    Numbers(f64, f64),
    BigInts(BigInt, BigInt),
}
