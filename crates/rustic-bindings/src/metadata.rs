//! Binding metadata for the pinned DOM Standard text interfaces.

use rustic_dom::DomString;
use rustic_js::runtime::{Agent, JsResult, Value};

use crate::webidl::{DomStringConversion, IntegerConversion, to_dom_string, to_unsigned_long};

mod generated;

/// The selected Window-exposed DOM interfaces, in manifest order.
pub fn interfaces() -> &'static [Interface] {
    generated::INTERFACES
}

/// Find an interface in the selected metadata by its exact IDL name.
pub fn interface(name: &str) -> Option<&'static Interface> {
    interfaces().iter().find(|entry| entry.name == name)
}

/// One interface from the pinned subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interface {
    name: &'static str,
    parent: Option<&'static str>,
    members: &'static [Member],
}

impl Interface {
    /// The declared IDL name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The declared parent, which may be outside the selected subset.
    pub fn parent(&self) -> Option<&'static str> {
        self.parent
    }

    /// The members declared directly on this interface, in IDL order.
    pub fn members(&self) -> &'static [Member] {
        self.members
    }
}

/// The supported kinds of interface member.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Member {
    /// An interface attribute.
    Attribute {
        /// The IDL name.
        name: &'static str,
        /// The value type and any input conversion policy.
        value_type: ValueType,
        /// Whether the standard declares a read-only attribute.
        readonly: bool,
    },
    /// The argument list of a constructor.
    Constructor(&'static [Argument]),
    /// A named interface operation.
    Operation {
        /// The IDL name.
        name: &'static str,
        /// The declared result type.
        returns: ReturnType,
        /// Whether the operation is marked `[NewObject]`.
        new_object: bool,
        /// The declared argument list, in IDL order.
        arguments: &'static [Argument],
    },
}

/// The result types used by operations in the selected subset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnType {
    /// Returns the JavaScript undefined value.
    Undefined,
    /// Returns one of the supported scalar IDL types.
    Value(ValueType),
    /// Returns an object of the named IDL interface.
    Interface(&'static str),
}

/// The scalar types and conversion policies used by the selected members.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueType {
    /// DOMString with the given null conversion policy.
    DomString(DomStringConversion),
    /// Unsigned long with the Web IDL default wrapping policy.
    UnsignedLong,
}

impl ValueType {
    /// Convert an input according to this member's IDL declaration.
    ///
    /// Coercion can call JavaScript. Keep other live values rooted and finish
    /// conversion before borrowing the DOM for a write.
    pub fn convert(self, agent: &mut Agent, input: Value) -> JsResult<ConvertedValue> {
        match self {
            Self::DomString(policy) => {
                to_dom_string(agent, input, policy).map(ConvertedValue::DomString)
            }
            Self::UnsignedLong => to_unsigned_long(agent, input, IntegerConversion::Wrap)
                .map(ConvertedValue::UnsignedLong),
        }
    }
}

/// An owned scalar value ready for a host operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConvertedValue {
    /// UTF-16 code units.
    DomString(DomString),
    /// A 32-bit unsigned integer.
    UnsignedLong(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArgumentDefault {
    EmptyString,
}

/// An operation or constructor argument from the pinned IDL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Argument {
    name: &'static str,
    value_type: ValueType,
    default: Option<ArgumentDefault>,
}

impl Argument {
    /// The argument's name in IDL.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The declared input conversion.
    pub fn value_type(&self) -> ValueType {
        self.value_type
    }

    /// Whether omitting this argument is allowed.
    pub fn is_optional(&self) -> bool {
        self.default.is_some()
    }

    /// Convert one caller-supplied argument. `None` represents omission.
    ///
    /// Binding callers validate the required argument count before converting
    /// the argument list. An explicit undefined uses the default of an
    /// optional argument in this subset.
    pub fn convert(&self, agent: &mut Agent, input: Option<Value>) -> JsResult<ConvertedValue> {
        match (input, self.default) {
            (None | Some(Value::UNDEFINED), Some(ArgumentDefault::EmptyString)) => {
                Ok(ConvertedValue::DomString(DomString::default()))
            }
            (None, None) => agent.type_error("required Web IDL argument is missing"),
            (Some(value), _) => self.value_type.convert(agent, value),
        }
    }
}
