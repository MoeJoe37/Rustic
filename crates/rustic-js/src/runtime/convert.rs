//! Type conversion and comparison (§7.1, §7.2).

use std::cmp::Ordering;

use super::agent::{Agent, JsResult};
use super::bigint::BigInt;
use super::number::{number_to_string, string_to_number};
use super::object::{Class, Object, PropertyDescriptor};
use super::shape::PropertyKey;
use super::string::JsStr;
use super::value::{Kind, ObjectId, StringId, Value};
use crate::unicode::{is_line_terminator, is_whitespace};

/// The `hint` of `ToPrimitive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferredType {
    /// `"string"`.
    String,
    /// `"number"`.
    Number,
}

/// A Number or a BigInt (`ToNumeric`).
#[derive(Clone, Debug, PartialEq)]
pub enum Numeric {
    /// A Number.
    Number(f64),
    /// A BigInt.
    BigInt(BigInt),
}

/// `StringToBigInt` (§7.1.14): `None` where the grammar does not match.
pub fn string_to_bigint(s: &JsStr) -> Option<BigInt> {
    let text: String = s
        .units()
        .map(|u| char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}'))
        .collect();
    let trimmed = text
        .trim_matches(|c: char| is_whitespace(u32::from(c)) || is_line_terminator(u32::from(c)));
    if trimmed.is_empty() {
        return Some(BigInt::zero());
    }
    let lower = trimmed.get(..2).map(str::to_ascii_lowercase);
    let radix = match lower.as_deref() {
        Some("0x") => 16,
        Some("0o") => 8,
        Some("0b") => 2,
        _ => 10,
    };
    if radix != 10 {
        let digits = &trimmed[2..];
        return (!digits.is_empty())
            .then(|| BigInt::from_digits(digits, radix, false))
            .flatten();
    }
    let (negative, digits) = match trimmed.as_bytes()[0] {
        b'-' => (true, &trimmed[1..]),
        b'+' => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    BigInt::from_digits(digits, 10, negative)
}

impl Agent {
    /// `RequireObjectCoercible`.
    pub fn require_object_coercible(&mut self, value: Value) -> JsResult<Value> {
        if value.is_nullish() {
            return self.type_error("cannot convert undefined or null to object");
        }
        Ok(value)
    }

    /// `ToPrimitive(input, preferredType)`.
    pub fn to_primitive(
        &mut self,
        input: Value,
        preferred: Option<PreferredType>,
    ) -> JsResult<Value> {
        let Some(_) = input.as_object() else {
            return Ok(input);
        };
        let key = PropertyKey::Symbol(self.symbols.to_primitive);
        if let Some(exotic) = self.get_method(input, key)? {
            let hint = match preferred {
                None => "default",
                Some(PreferredType::String) => "string",
                Some(PreferredType::Number) => "number",
            };
            let hint = self.string(hint)?;
            let result = self.call(exotic, input, &[hint])?;
            if result.as_object().is_some() {
                return self.type_error("Symbol.toPrimitive returned an object");
            }
            return Ok(result);
        }
        self.ordinary_to_primitive(input, preferred.unwrap_or(PreferredType::Number))
    }

    /// `OrdinaryToPrimitive(O, hint)`.
    pub fn ordinary_to_primitive(&mut self, o: Value, hint: PreferredType) -> JsResult<Value> {
        let (first, second) = (self.names.value_of, self.names.to_string);
        let order = match hint {
            PreferredType::String => [second, first],
            PreferredType::Number => [first, second],
        };
        for name in order {
            let method = self.get_v(o, PropertyKey::String(name))?;
            if self.is_callable(method) {
                let result = self.call(method, o, &[])?;
                if result.as_object().is_none() {
                    return Ok(result);
                }
            }
        }
        self.type_error("cannot convert object to primitive value")
    }

    /// `ToBoolean`.
    pub fn to_boolean(&self, value: Value) -> bool {
        match value.kind() {
            Kind::Undefined | Kind::Null | Kind::Uninitialized => false,
            Kind::Boolean(b) => b,
            Kind::Number(n) => !(n == 0.0 || n.is_nan()),
            Kind::String(s) => self.heap.string_len(s) != 0,
            Kind::Symbol(_) | Kind::Object(_) => true,
            Kind::BigInt(b) => !self.heap.bigint(b).is_zero(),
        }
    }

    /// `ToNumeric`.
    pub fn to_numeric(&mut self, value: Value) -> JsResult<Numeric> {
        let primitive = self.to_primitive(value, Some(PreferredType::Number))?;
        if let Kind::BigInt(b) = primitive.kind() {
            return Ok(Numeric::BigInt(self.heap.bigint(b).clone()));
        }
        self.to_number(primitive).map(Numeric::Number)
    }

    /// `ToNumber`.
    pub fn to_number(&mut self, value: Value) -> JsResult<f64> {
        match value.kind() {
            Kind::Undefined | Kind::Uninitialized => Ok(f64::NAN),
            Kind::Null => Ok(0.0),
            Kind::Boolean(b) => Ok(if b { 1.0 } else { 0.0 }),
            Kind::Number(n) => Ok(n),
            Kind::String(s) => {
                self.heap.flatten(s)?;
                Ok(string_to_number(self.heap.string(s)))
            }
            Kind::Symbol(_) => self.type_error("cannot convert a Symbol to a number"),
            Kind::BigInt(_) => self.type_error("cannot convert a BigInt to a number"),
            Kind::Object(_) => {
                let primitive = self.to_primitive(value, Some(PreferredType::Number))?;
                self.to_number(primitive)
            }
        }
    }

    /// `ToString`, as a heap string.
    pub fn to_string(&mut self, value: Value) -> JsResult<StringId> {
        let text = match value.kind() {
            Kind::String(s) => return Ok(s),
            Kind::Undefined | Kind::Uninitialized => return Ok(self.names.undefined),
            Kind::Null => "null".to_owned(),
            Kind::Boolean(true) => "true".to_owned(),
            Kind::Boolean(false) => "false".to_owned(),
            Kind::Number(n) => number_to_string(n),
            Kind::BigInt(b) => self.heap.bigint(b).to_string_radix10(),
            Kind::Symbol(_) => return self.type_error("cannot convert a Symbol to a string"),
            Kind::Object(_) => {
                // A string from script (`toString`) is flattened here, so
                // built-ins converting objects read it flat (TD-116). A
                // string value itself is returned as it is: `+` and
                // template literals append to ropes without copying them.
                let primitive = self.to_primitive(value, Some(PreferredType::String))?;
                let s = self.to_string(primitive)?;
                self.heap.flatten(s)?;
                return Ok(s);
            }
        };
        Ok(self.heap.alloc_str(&text)?)
    }

    /// `ToObject`: wrap a primitive, or a TypeError for undefined/null.
    pub fn to_object(&mut self, value: Value) -> JsResult<ObjectId> {
        let (class, proto) = match value.kind() {
            Kind::Object(o) => return Ok(o),
            Kind::Undefined | Kind::Null | Kind::Uninitialized => {
                return self.type_error("cannot convert undefined or null to object");
            }
            Kind::Boolean(_) => (Class::Boolean, self.intrinsics.boolean_prototype),
            Kind::Number(_) => (Class::Number, self.intrinsics.number_prototype),
            Kind::String(_) => (Class::String, self.intrinsics.string_prototype),
            Kind::Symbol(_) => (Class::Symbol, self.intrinsics.symbol_prototype),
            Kind::BigInt(_) => (Class::BigInt, self.intrinsics.bigint_prototype),
        };
        let mut object = Object::new(class, Some(proto));
        object.extra_mut().primitive = Some(value);
        let o = self.heap.alloc_object(object)?;
        if let Some(s) = value.as_string() {
            let length = self.heap.string_len(s) as f64;
            let key = PropertyKey::String(self.names.length);
            self.define_property_or_throw(
                o,
                key,
                PropertyDescriptor::data_with(Value::number(length), false, false, false),
            )?;
        }
        Ok(o)
    }

    /// `ToPropertyKey`.
    pub fn to_property_key(&mut self, value: Value) -> JsResult<PropertyKey> {
        if let Some(n) = value.as_number() {
            // An array index needs no string.
            let index = n as u32;
            if f64::from(index) == n && index != u32::MAX && !(n == 0.0 && n.is_sign_negative()) {
                return Ok(PropertyKey::Index(index));
            }
        }
        if let Some(s) = value.as_string() {
            return Ok(self.heap.key_for_string(s)?);
        }
        let key = self.to_primitive(value, Some(PreferredType::String))?;
        if let Kind::Symbol(s) = key.kind() {
            return Ok(PropertyKey::Symbol(s));
        }
        let s = self.to_string(key)?;
        Ok(self.heap.key_for_string(s)?)
    }

    /// The value of a property key (`"0"` for index 0).
    pub fn key_to_value(&mut self, key: PropertyKey) -> JsResult<Value> {
        Ok(match key {
            PropertyKey::Index(i) => Value::string(self.heap.intern_str(&i.to_string())?),
            PropertyKey::String(s) => Value::string(s),
            PropertyKey::Symbol(s) => Value::symbol(s),
        })
    }

    fn strings_equal(&self, a: StringId, b: StringId) -> bool {
        self.heap.strings_equal(a, b)
    }

    /// Flatten `value` if it is a rope, charging the copy (TD-116).
    pub(crate) fn flatten_value(&mut self, value: Value) -> JsResult<()> {
        if let Some(s) = value.as_string() {
            self.heap.flatten(s)?;
        }
        Ok(())
    }

    /// `SameValue`.
    pub fn same_value(&self, x: Value, y: Value) -> bool {
        match (x.kind(), y.kind()) {
            // Values compare by bits: NaN is canonical, and +0/-0 differ.
            (Kind::Number(_), Kind::Number(_)) => x.bits() == y.bits(),
            _ => self.same_value_non_number(x, y),
        }
    }

    /// `SameValueZero`.
    pub fn same_value_zero(&self, x: Value, y: Value) -> bool {
        match (x.kind(), y.kind()) {
            (Kind::Number(a), Kind::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
            _ => self.same_value_non_number(x, y),
        }
    }

    fn same_value_non_number(&self, x: Value, y: Value) -> bool {
        match (x.kind(), y.kind()) {
            (Kind::String(a), Kind::String(b)) => self.strings_equal(a, b),
            (Kind::BigInt(a), Kind::BigInt(b)) => self.heap.bigint(a) == self.heap.bigint(b),
            _ => x == y,
        }
    }

    /// `IsStrictlyEqual` (`===`).
    pub fn is_strictly_equal(&self, x: Value, y: Value) -> bool {
        match (x.kind(), y.kind()) {
            (Kind::Number(a), Kind::Number(b)) => a == b,
            _ => self.same_value_non_number(x, y),
        }
    }

    /// `IsLooselyEqual` (`==`).
    pub fn is_loosely_equal(&mut self, x: Value, y: Value) -> JsResult<bool> {
        if x.type_of() == y.type_of() {
            return Ok(self.is_strictly_equal(x, y));
        }
        self.flatten_value(x)?;
        self.flatten_value(y)?;
        Ok(match (x.kind(), y.kind()) {
            (Kind::Null | Kind::Undefined, Kind::Null | Kind::Undefined) => true,
            (Kind::Number(n), Kind::String(s)) | (Kind::String(s), Kind::Number(n)) => {
                n == string_to_number(self.heap.string(s))
            }
            (Kind::BigInt(b), Kind::String(s)) | (Kind::String(s), Kind::BigInt(b)) => {
                match string_to_bigint(self.heap.string(s)) {
                    Some(n) => &n == self.heap.bigint(b),
                    None => false,
                }
            }
            (Kind::Boolean(b), _) => {
                return self.is_loosely_equal(Value::number(if b { 1.0 } else { 0.0 }), y);
            }
            (_, Kind::Boolean(b)) => {
                return self.is_loosely_equal(x, Value::number(if b { 1.0 } else { 0.0 }));
            }
            (
                Kind::Number(_) | Kind::String(_) | Kind::BigInt(_) | Kind::Symbol(_),
                Kind::Object(_),
            ) => {
                let y = self.to_primitive(y, None)?;
                return self.is_loosely_equal(x, y);
            }
            (
                Kind::Object(_),
                Kind::Number(_) | Kind::String(_) | Kind::BigInt(_) | Kind::Symbol(_),
            ) => {
                let x = self.to_primitive(x, None)?;
                return self.is_loosely_equal(x, y);
            }
            (Kind::BigInt(b), Kind::Number(n)) | (Kind::Number(n), Kind::BigInt(b)) => {
                self.heap.bigint(b).cmp_f64(n) == Some(Ordering::Equal)
            }
            _ => false,
        })
    }

    /// `IsLessThan(x, y, LeftFirst)`: `Some(true)`, `Some(false)` or `None`
    /// (undefined, from NaN).
    pub fn is_less_than(&mut self, x: Value, y: Value, left_first: bool) -> JsResult<Option<bool>> {
        let (px, py) = if left_first {
            let px = self.to_primitive(x, Some(PreferredType::Number))?;
            (px, self.to_primitive(y, Some(PreferredType::Number))?)
        } else {
            let py = self.to_primitive(y, Some(PreferredType::Number))?;
            (self.to_primitive(x, Some(PreferredType::Number))?, py)
        };
        self.flatten_value(px)?;
        self.flatten_value(py)?;
        if let (Kind::String(a), Kind::String(b)) = (px.kind(), py.kind()) {
            return Ok(Some(self.heap.string(a) < self.heap.string(b)));
        }
        if let (Kind::BigInt(a), Kind::String(s)) = (px.kind(), py.kind()) {
            return Ok(string_to_bigint(self.heap.string(s)).map(|b| self.heap.bigint(a) < &b));
        }
        if let (Kind::String(s), Kind::BigInt(b)) = (px.kind(), py.kind()) {
            return Ok(string_to_bigint(self.heap.string(s)).map(|a| &a < self.heap.bigint(b)));
        }
        let nx = self.to_numeric(px)?;
        let ny = self.to_numeric(py)?;
        Ok(match (nx, ny) {
            (Numeric::Number(a), Numeric::Number(b)) => {
                a.partial_cmp(&b).map(|o| o == Ordering::Less)
            }
            (Numeric::BigInt(a), Numeric::BigInt(b)) => Some(a < b),
            (Numeric::BigInt(a), Numeric::Number(b)) => a.cmp_f64(b).map(|o| o == Ordering::Less),
            (Numeric::Number(a), Numeric::BigInt(b)) => {
                b.cmp_f64(a).map(|o| o == Ordering::Greater)
            }
        })
    }

    /// The result of the `typeof` operator.
    pub fn type_of(&self, value: Value) -> &'static str {
        match value.kind() {
            Kind::Undefined | Kind::Uninitialized => "undefined",
            Kind::Null => "object",
            Kind::Boolean(_) => "boolean",
            Kind::Number(_) => "number",
            Kind::String(_) => "string",
            Kind::Symbol(_) => "symbol",
            Kind::BigInt(_) => "bigint",
            Kind::Object(_) if self.is_callable(value) => "function",
            Kind::Object(_) => "object",
        }
    }
}
