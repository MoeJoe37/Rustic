//! Object storage (ADR 0015): a shape (or a private dictionary) for named
//! properties, a slot vector for their values, and dense or sparse indexed
//! elements. The internal methods that read and change this storage are in
//! `internal.rs`.

use std::collections::{BTreeMap, HashMap};

use super::agent::NativeFn;
use super::shape::{Attributes, PropertyKey, ROOT_SHAPE, ShapeId};
use super::value::{ObjectId, Value};

/// What kind of object this is, for exotic behavior and brand checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// An ordinary object.
    Ordinary,
    /// An Array exotic object (§10.4.2).
    Array,
    /// A function object (native until the interpreter exists).
    Function,
    /// An Error instance (has `[[ErrorData]]`).
    Error,
    /// A Boolean object (`[[BooleanData]]` in the primitive slot).
    Boolean,
    /// A Number object.
    Number,
    /// A String exotic object (§10.4.3).
    String,
    /// A Symbol object.
    Symbol,
    /// A BigInt object.
    BigInt,
}

/// A complete or partial property descriptor (§6.2.6).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PropertyDescriptor {
    /// `[[Value]]`.
    pub value: Option<Value>,
    /// `[[Writable]]`.
    pub writable: Option<bool>,
    /// `[[Get]]` (undefined or a callable object).
    pub get: Option<Value>,
    /// `[[Set]]` (undefined or a callable object).
    pub set: Option<Value>,
    /// `[[Enumerable]]`.
    pub enumerable: Option<bool>,
    /// `[[Configurable]]`.
    pub configurable: Option<bool>,
}

impl PropertyDescriptor {
    /// A writable, enumerable, configurable data property.
    pub fn data(value: Value) -> Self {
        Self::data_with(value, true, true, true)
    }

    /// A data property with explicit attributes.
    pub fn data_with(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Self {
        Self {
            value: Some(value),
            writable: Some(writable),
            get: None,
            set: None,
            enumerable: Some(enumerable),
            configurable: Some(configurable),
        }
    }

    /// An accessor property.
    pub fn accessor(get: Value, set: Value, enumerable: bool, configurable: bool) -> Self {
        Self {
            value: None,
            writable: None,
            get: Some(get),
            set: Some(set),
            enumerable: Some(enumerable),
            configurable: Some(configurable),
        }
    }

    /// `IsAccessorDescriptor`.
    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }

    /// `IsDataDescriptor`.
    pub fn is_data(&self) -> bool {
        self.value.is_some() || self.writable.is_some()
    }

    /// `IsGenericDescriptor`.
    pub fn is_generic(&self) -> bool {
        !self.is_accessor() && !self.is_data()
    }

    pub(crate) fn from_stored(attributes: Attributes, first: Value, second: Value) -> Self {
        let enumerable = attributes.has(Attributes::ENUMERABLE);
        let configurable = attributes.has(Attributes::CONFIGURABLE);
        if attributes.is_accessor() {
            Self::accessor(first, second, enumerable, configurable)
        } else {
            Self::data_with(
                first,
                attributes.has(Attributes::WRITABLE),
                enumerable,
                configurable,
            )
        }
    }

    /// The stored form of a complete descriptor.
    pub(crate) fn to_stored(self) -> (Attributes, Value, Value) {
        let mut bits = 0;
        if self.enumerable == Some(true) {
            bits |= Attributes::ENUMERABLE;
        }
        if self.configurable == Some(true) {
            bits |= Attributes::CONFIGURABLE;
        }
        if self.is_accessor() {
            bits |= Attributes::ACCESSOR;
            (
                Attributes::new(bits),
                self.get.unwrap_or(Value::UNDEFINED),
                self.set.unwrap_or(Value::UNDEFINED),
            )
        } else {
            if self.writable == Some(true) {
                bits |= Attributes::WRITABLE;
            }
            (
                Attributes::new(bits),
                self.value.unwrap_or(Value::UNDEFINED),
                Value::UNDEFINED,
            )
        }
    }
}

/// A stored property outside the shape tree: dictionary entries and sparse
/// elements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stored {
    pub attributes: Attributes,
    /// The value, or the getter.
    pub first: Value,
    /// The setter (accessors only).
    pub second: Value,
}

/// Named properties of an object that left the shared shape tree, kept in
/// insertion order. Deleted entries leave a hole until compaction.
#[derive(Clone, Debug, Default)]
pub(crate) struct Dictionary {
    pub entries: Vec<Option<(PropertyKey, Stored)>>,
    pub index: HashMap<PropertyKey, u32>,
    pub holes: u32,
}

impl Dictionary {
    pub fn get(&self, key: PropertyKey) -> Option<Stored> {
        let at = *self.index.get(&key)?;
        self.entries[at as usize].map(|(_, s)| s)
    }

    pub fn set(&mut self, key: PropertyKey, stored: Stored) {
        match self.index.get(&key) {
            Some(&at) => self.entries[at as usize] = Some((key, stored)),
            None => {
                // Entry counts are bounded by the heap budget, far below 2³².
                self.index.insert(key, self.entries.len() as u32);
                self.entries.push(Some((key, stored)));
            }
        }
    }

    pub fn remove(&mut self, key: PropertyKey) {
        if let Some(at) = self.index.remove(&key) {
            self.entries[at as usize] = None;
            self.holes += 1;
            if self.holes as usize * 2 > self.entries.len() {
                self.entries.retain(Option::is_some);
                self.holes = 0;
                self.index = self
                    .entries
                    .iter()
                    .enumerate()
                    .filter_map(|(i, e)| e.map(|(k, _)| (k, i as u32)))
                    .collect();
            }
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = PropertyKey> + '_ {
        self.entries.iter().filter_map(|e| e.map(|(k, _)| k))
    }
}

/// Rarely needed per-object state, boxed so ordinary objects stay small.
#[derive(Clone, Debug, Default)]
pub(crate) struct Extra {
    /// Named properties after leaving the shape tree.
    pub dictionary: Option<Dictionary>,
    /// Indexed properties once any is not a plain data property or the
    /// dense vector would be mostly holes.
    pub sparse: Option<BTreeMap<u32, Stored>>,
    /// `[[BooleanData]]`, `[[NumberData]]`, `[[StringData]]` and friends.
    pub primitive: Option<Value>,
    /// The behavior of a native function.
    pub native: Option<NativeFn>,
}

/// The `length` of an Array is writable (cleared by freezing).
pub(crate) const FLAG_EXTENSIBLE: u8 = 1;
pub(crate) const FLAG_ARRAY_LENGTH_WRITABLE: u8 = 2;

const NO_PROTOTYPE: u32 = u32::MAX;

/// One object.
#[derive(Clone, Debug)]
pub struct Object {
    pub(crate) shape: ShapeId,
    proto: u32,
    /// Array `length` (arrays only).
    pub(crate) length: u32,
    pub(crate) class: Class,
    pub(crate) flags: u8,
    /// Named property values, in shape slot order (unused once a
    /// dictionary).
    pub(crate) slots: Vec<Value>,
    /// Dense indexed elements; holes are [`Value::UNINITIALIZED`]. Each is a
    /// writable, enumerable, configurable data property.
    pub(crate) elements: Vec<Value>,
    pub(crate) extra: Option<Box<Extra>>,
}

impl Object {
    /// A new extensible object of `class`.
    pub(crate) fn new(class: Class, proto: Option<ObjectId>) -> Self {
        Self {
            shape: ROOT_SHAPE,
            proto: proto.map_or(NO_PROTOTYPE, ObjectId::index),
            length: 0,
            class,
            flags: FLAG_EXTENSIBLE | FLAG_ARRAY_LENGTH_WRITABLE,
            slots: Vec::new(),
            elements: Vec::new(),
            extra: None,
        }
    }

    /// `[[Prototype]]`.
    pub(crate) fn proto(&self) -> Option<ObjectId> {
        (self.proto != NO_PROTOTYPE).then_some(ObjectId(self.proto))
    }

    pub(crate) fn set_proto(&mut self, proto: Option<ObjectId>) {
        self.proto = proto.map_or(NO_PROTOTYPE, ObjectId::index);
    }

    pub(crate) fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    pub(crate) fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    pub(crate) fn extra_mut(&mut self) -> &mut Extra {
        self.extra.get_or_insert_with(Box::default)
    }

    pub(crate) fn dictionary(&self) -> Option<&Dictionary> {
        self.extra.as_ref()?.dictionary.as_ref()
    }

    pub(crate) fn sparse(&self) -> Option<&BTreeMap<u32, Stored>> {
        self.extra.as_ref()?.sparse.as_ref()
    }

    /// The primitive in a wrapper object's internal slot.
    pub(crate) fn primitive(&self) -> Option<Value> {
        self.extra.as_ref()?.primitive
    }

    pub(crate) fn native(&self) -> Option<NativeFn> {
        self.extra.as_ref()?.native
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_objects_are_small() {
        assert!(
            std::mem::size_of::<Object>() <= 72,
            "{}",
            std::mem::size_of::<Object>()
        );
        assert_eq!(std::mem::size_of::<PropertyKey>(), 8);
    }
}
