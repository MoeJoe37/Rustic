//! The 8-byte ECMAScript value (ECMA-262 §6.1), NaN-boxed in a `u64`
//! (ADR 0015).
//!
//! A Number is stored as its IEEE-754 bits, with every NaN canonicalized.
//! Other values sit in the negative quiet-NaN space, which no canonical
//! Number uses: the top 16 bits are a tag and the low 32 bits a payload (a
//! boolean or an arena index). Encoding uses only integer operations, so the
//! layout is the same on every target and needs no `unsafe`.

use std::fmt;

/// The bit pattern every NaN is stored as.
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;
/// Tags occupy the top 16 bits; all are negative quiet NaNs with a nonzero
/// tag field, which a canonical Number never has.
const TAG_SHIFT: u32 = 48;
const TAG_UNDEFINED: u64 = 0xFFF9;
const TAG_NULL: u64 = 0xFFFA;
const TAG_BOOLEAN: u64 = 0xFFFB;
const TAG_STRING: u64 = 0xFFFC;
const TAG_SYMBOL: u64 = 0xFFFD;
const TAG_OBJECT: u64 = 0xFFFE;
const TAG_OTHER: u64 = 0xFFFF;
/// Payloads of [`TAG_OTHER`].
const OTHER_UNINITIALIZED: u64 = 0;
const OTHER_BIGINT_BASE: u64 = 1 << 32;

macro_rules! handle {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub(crate) u32);

        impl $name {
            /// The arena index.
            pub fn index(self) -> u32 {
                self.0
            }
        }
    };
}

handle!(
    /// A string in a [`super::Heap`].
    StringId
);
handle!(
    /// A symbol in a [`super::Heap`].
    SymbolId
);
handle!(
    /// A BigInt in a [`super::Heap`].
    BigIntId
);
handle!(
    /// An object in a [`super::Heap`].
    ObjectId
);

/// What a [`Value`] is, decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// `undefined`.
    Undefined,
    /// `null`.
    Null,
    /// A Boolean.
    Boolean(bool),
    /// A Number.
    Number(f64),
    /// A String.
    String(StringId),
    /// A Symbol.
    Symbol(SymbolId),
    /// A BigInt.
    BigInt(BigIntId),
    /// An Object.
    Object(ObjectId),
    /// Not a language value: the state of a binding before its declaration
    /// runs (the temporal dead zone), or an array hole.
    Uninitialized,
}

/// The ECMAScript language type of a value (§6.1), as `typeof` sees it
/// apart from functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// Undefined.
    Undefined,
    /// Null.
    Null,
    /// Boolean.
    Boolean,
    /// Number.
    Number,
    /// String.
    String,
    /// Symbol.
    Symbol,
    /// BigInt.
    BigInt,
    /// Object.
    Object,
}

/// An ECMAScript value in 8 bytes. Handles are only meaningful with the
/// heap that made them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Value(u64);

impl Value {
    /// `undefined`.
    pub const UNDEFINED: Value = Value(TAG_UNDEFINED << TAG_SHIFT);
    /// `null`.
    pub const NULL: Value = Value(TAG_NULL << TAG_SHIFT);
    /// `true`.
    pub const TRUE: Value = Value(TAG_BOOLEAN << TAG_SHIFT | 1);
    /// `false`.
    pub const FALSE: Value = Value(TAG_BOOLEAN << TAG_SHIFT);
    /// The temporal-dead-zone marker (never visible to script).
    pub const UNINITIALIZED: Value = Value(TAG_OTHER << TAG_SHIFT | OTHER_UNINITIALIZED);
    /// `NaN`.
    pub const NAN: Value = Value(CANONICAL_NAN);

    /// A Number; every NaN becomes the canonical NaN.
    #[inline]
    pub fn number(n: f64) -> Value {
        if n.is_nan() {
            Value::NAN
        } else {
            Value(n.to_bits())
        }
    }

    /// A Boolean.
    #[inline]
    pub fn boolean(b: bool) -> Value {
        if b { Value::TRUE } else { Value::FALSE }
    }

    /// A String.
    pub fn string(id: StringId) -> Value {
        Value(TAG_STRING << TAG_SHIFT | u64::from(id.0))
    }

    /// A Symbol.
    pub fn symbol(id: SymbolId) -> Value {
        Value(TAG_SYMBOL << TAG_SHIFT | u64::from(id.0))
    }

    /// A BigInt.
    pub fn bigint(id: BigIntId) -> Value {
        Value(TAG_OTHER << TAG_SHIFT | OTHER_BIGINT_BASE | u64::from(id.0))
    }

    /// An Object.
    pub fn object(id: ObjectId) -> Value {
        Value(TAG_OBJECT << TAG_SHIFT | u64::from(id.0))
    }

    /// Decode the value.
    pub fn kind(self) -> Kind {
        let tag = self.0 >> TAG_SHIFT;
        let payload = self.0 & 0xFFFF_FFFF;
        // Truncation is the payload extraction: the mask above keeps the
        // value within `u32`.
        let index = payload as u32;
        match tag {
            TAG_UNDEFINED => Kind::Undefined,
            TAG_NULL => Kind::Null,
            TAG_BOOLEAN => Kind::Boolean(payload != 0),
            TAG_STRING => Kind::String(StringId(index)),
            TAG_SYMBOL => Kind::Symbol(SymbolId(index)),
            TAG_OBJECT => Kind::Object(ObjectId(index)),
            TAG_OTHER if self.0 & OTHER_BIGINT_BASE != 0 => Kind::BigInt(BigIntId(index)),
            TAG_OTHER => Kind::Uninitialized,
            _ => Kind::Number(f64::from_bits(self.0)),
        }
    }

    /// The language type. The uninitialized marker reports `Undefined`; it
    /// never reaches code that asks.
    pub fn type_of(self) -> Type {
        match self.kind() {
            Kind::Undefined | Kind::Uninitialized => Type::Undefined,
            Kind::Null => Type::Null,
            Kind::Boolean(_) => Type::Boolean,
            Kind::Number(_) => Type::Number,
            Kind::String(_) => Type::String,
            Kind::Symbol(_) => Type::Symbol,
            Kind::BigInt(_) => Type::BigInt,
            Kind::Object(_) => Type::Object,
        }
    }

    /// The Number, if this is one.
    #[inline]
    pub fn as_number(self) -> Option<f64> {
        // Every tag sorts above every Number's bits (the canonical NaN
        // included), so one comparison decides.
        if self.0 < TAG_UNDEFINED << TAG_SHIFT {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    /// The object, if this is one.
    #[inline]
    pub fn as_object(self) -> Option<ObjectId> {
        if self.0 >> TAG_SHIFT == TAG_OBJECT {
            // The payload is the low 32 bits.
            Some(ObjectId(self.0 as u32))
        } else {
            None
        }
    }

    /// The string, if this is one.
    #[inline]
    pub fn as_string(self) -> Option<StringId> {
        if self.0 >> TAG_SHIFT == TAG_STRING {
            // The payload is the low 32 bits.
            Some(StringId(self.0 as u32))
        } else {
            None
        }
    }

    /// `undefined` or `null`.
    pub fn is_nullish(self) -> bool {
        self == Value::UNDEFINED || self == Value::NULL
    }

    /// The value with these bits, which must come from [`Value::bits`].
    pub(crate) const fn from_raw(bits: u64) -> Value {
        Value(bits)
    }

    /// The raw encoding, for hashing and tests.
    pub fn bits(self) -> u64 {
        self.0
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::boolean(b)
    }
}

impl From<ObjectId> for Value {
    fn from(o: ObjectId) -> Self {
        Value::object(o)
    }
}

impl From<StringId> for Value {
    fn from(s: StringId) -> Self {
        Value::string(s)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            Kind::Undefined => f.write_str("undefined"),
            Kind::Null => f.write_str("null"),
            Kind::Boolean(b) => write!(f, "{b}"),
            Kind::Number(n) => write!(f, "{n:?}"),
            Kind::String(s) => write!(f, "string#{}", s.0),
            Kind::Symbol(s) => write!(f, "symbol#{}", s.0),
            Kind::BigInt(b) => write!(f, "bigint#{}", b.0),
            Kind::Object(o) => write!(f, "object#{}", o.0),
            Kind::Uninitialized => f.write_str("<uninitialized>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_eight_bytes() {
        assert_eq!(std::mem::size_of::<Value>(), 8);
        assert_eq!(std::mem::size_of::<Option<ObjectId>>(), 8);
    }

    #[test]
    fn every_double_round_trips_and_nan_is_canonical() {
        for n in [
            0.0,
            -0.0,
            1.5,
            -1e308,
            f64::MIN_POSITIVE,
            5e-324,
            -5e-324,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
        ] {
            let v = Value::number(n);
            assert_eq!(v.kind(), Kind::Number(n));
            assert_eq!(v.as_number().unwrap().to_bits(), n.to_bits());
        }
        for bits in [
            0x7FF8_0000_0000_0001u64,
            0xFFF8_0000_0000_0000,
            0xFFFF_FFFF_FFFF_FFFF,
            0x7FF0_0000_0000_0001,
        ] {
            let v = Value::number(f64::from_bits(bits));
            assert_eq!(v, Value::NAN);
            assert!(v.as_number().unwrap().is_nan());
        }
    }

    #[test]
    fn tagged_values_decode() {
        assert_eq!(Value::UNDEFINED.kind(), Kind::Undefined);
        assert_eq!(Value::NULL.kind(), Kind::Null);
        assert_eq!(Value::TRUE.kind(), Kind::Boolean(true));
        assert_eq!(Value::FALSE.kind(), Kind::Boolean(false));
        assert_eq!(Value::UNINITIALIZED.kind(), Kind::Uninitialized);
        for i in [0, 1, 0xFFFF_FFFF] {
            assert_eq!(Value::string(StringId(i)).kind(), Kind::String(StringId(i)));
            assert_eq!(Value::symbol(SymbolId(i)).kind(), Kind::Symbol(SymbolId(i)));
            assert_eq!(Value::object(ObjectId(i)).kind(), Kind::Object(ObjectId(i)));
            assert_eq!(Value::bigint(BigIntId(i)).kind(), Kind::BigInt(BigIntId(i)));
        }
        assert_ne!(Value::UNINITIALIZED, Value::bigint(BigIntId(0)));
        assert!(Value::NULL.is_nullish() && !Value::FALSE.is_nullish());
    }
}
