//! Object internal methods (§10.1 ordinary, §10.4.2 Array exotic, §10.4.3
//! String exotic) and the object abstract operations of §7.3, over the
//! storage in `object.rs`.

use std::collections::BTreeMap;

use super::agent::{Agent, ErrorKind, JsResult};
use super::number::to_uint32;
use super::object::{
    Class, Dictionary, FLAG_ARRAY_LENGTH_WRITABLE, FLAG_EXTENSIBLE, PropertyDescriptor, Stored,
};
use super::shape::{Attributes, PropertyKey, ROOT_SHAPE};
use super::string::JsStr;
use super::value::{Kind, ObjectId, Value};

/// Dense elements may grow past their length by this many holes plus their
/// length again before the object switches to sparse elements.
const DENSE_SLACK: usize = 16;

/// Where an own property lives.
#[derive(Clone, Copy, Debug)]
enum Location {
    Slot(u32, Attributes),
    Dictionary(Stored),
    Element(Value),
    Sparse(Stored),
    ArrayLength,
    StringIndex(u16),
}

/// `SetIntegrityLevel` levels (§7.3.15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrityLevel {
    /// `Object.seal`.
    Sealed,
    /// `Object.freeze`.
    Frozen,
}

impl Agent {
    fn length_key(&self) -> PropertyKey {
        PropertyKey::String(self.names.length)
    }

    fn string_data(&self, o: ObjectId) -> Option<&JsStr> {
        let object = self.heap.object(o);
        if object.class != Class::String {
            return None;
        }
        object.primitive()?.as_string().map(|s| self.heap.string(s))
    }

    fn locate(&self, o: ObjectId, key: PropertyKey) -> Option<Location> {
        let object = self.heap.object(o);
        match key {
            PropertyKey::Index(i) => {
                if let Some(unit) = self.string_data(o).and_then(|s| s.unit(i as usize)) {
                    return Some(Location::StringIndex(unit));
                }
                if let Some(sparse) = object.sparse() {
                    return sparse.get(&i).copied().map(Location::Sparse);
                }
                object
                    .elements
                    .get(i as usize)
                    .copied()
                    .filter(|v| *v != Value::UNINITIALIZED)
                    .map(Location::Element)
            }
            _ if object.class == Class::Array && key == self.length_key() => {
                Some(Location::ArrayLength)
            }
            _ => match object.dictionary() {
                Some(d) => d.get(key).map(Location::Dictionary),
                None => self
                    .heap
                    .shapes
                    .lookup(object.shape, key)
                    .map(|s| Location::Slot(s.slot, s.attributes)),
            },
        }
    }

    /// Create the `prototype` object of a function on first use. Until
    /// then the property exists with its final attributes and holds
    /// [`Value::UNINITIALIZED`], which no other object property can, so a
    /// function that is never constructed or inspected costs one slot
    /// instead of a second object.
    fn realize_function_prototype(
        &mut self,
        f: ObjectId,
        attributes: Attributes,
    ) -> JsResult<Value> {
        let prototype = self.new_object()?;
        let constructor = PropertyKey::String(self.names.constructor);
        self.define_property_or_throw(
            prototype,
            constructor,
            PropertyDescriptor::data_with(Value::object(f), true, false, true),
        )?;
        let value = Value::object(prototype);
        let key = PropertyKey::String(self.names.prototype);
        self.store(
            f,
            key,
            Stored {
                first: value,
                second: Value::UNDEFINED,
                attributes,
            },
        )?;
        Ok(value)
    }

    fn describe(&mut self, o: ObjectId, location: Location) -> JsResult<PropertyDescriptor> {
        let lazy = match location {
            Location::Slot(slot, attributes) => (self.heap.object(o).slots[slot as usize]
                == Value::UNINITIALIZED)
                .then_some(attributes),
            Location::Dictionary(s) => (s.first == Value::UNINITIALIZED).then_some(s.attributes),
            _ => None,
        };
        if let Some(attributes) = lazy {
            let value = self.realize_function_prototype(o, attributes)?;
            return Ok(PropertyDescriptor::from_stored(
                attributes,
                value,
                Value::UNDEFINED,
            ));
        }
        let object = self.heap.object(o);
        Ok(match location {
            Location::Slot(slot, attributes) => {
                let first = object.slots[slot as usize];
                let second = if attributes.is_accessor() {
                    object.slots[slot as usize + 1]
                } else {
                    Value::UNDEFINED
                };
                PropertyDescriptor::from_stored(attributes, first, second)
            }
            Location::Dictionary(s) | Location::Sparse(s) => {
                PropertyDescriptor::from_stored(s.attributes, s.first, s.second)
            }
            Location::Element(v) => PropertyDescriptor::data(v),
            Location::ArrayLength => PropertyDescriptor::data_with(
                Value::number(f64::from(object.length)),
                object.has_flag(FLAG_ARRAY_LENGTH_WRITABLE),
                false,
                false,
            ),
            Location::StringIndex(unit) => {
                let s = self.heap.intern(&JsStr::from_units(&[unit]))?;
                PropertyDescriptor::data_with(Value::string(s), false, true, false)
            }
        })
    }

    // ---- storage ------------------------------------------------------

    fn make_dictionary(&mut self, o: ObjectId) {
        if self.heap.object(o).dictionary().is_some() {
            return;
        }
        let shape = self.heap.object(o).shape;
        let properties = self.heap.shapes.properties(shape);
        let object = self.heap.object_mut(o);
        let mut dictionary = Dictionary::default();
        for (key, slot) in properties {
            let first = object.slots[slot.slot as usize];
            let second = if slot.attributes.is_accessor() {
                object.slots[slot.slot as usize + 1]
            } else {
                Value::UNDEFINED
            };
            dictionary.set(
                key,
                Stored {
                    attributes: slot.attributes,
                    first,
                    second,
                },
            );
        }
        object.slots = Vec::new();
        // The shape is unused from now on; dropping it lets the collector
        // free shapes only dictionaries remember.
        object.shape = ROOT_SHAPE;
        object.extra_mut().dictionary = Some(dictionary);
    }

    fn make_sparse(&mut self, o: ObjectId) {
        let object = self.heap.object_mut(o);
        if object.sparse().is_some() {
            return;
        }
        let elements = std::mem::take(&mut object.elements);
        let sparse: BTreeMap<u32, Stored> = elements
            .into_iter()
            .enumerate()
            .filter(|(_, v)| *v != Value::UNINITIALIZED)
            // Dense elements are indexed below the 2³² − 1 array-index limit.
            .map(|(i, v)| {
                (
                    i as u32,
                    Stored {
                        attributes: Attributes::DEFAULT,
                        first: v,
                        second: Value::UNDEFINED,
                    },
                )
            })
            .collect();
        object.extra_mut().sparse = Some(sparse);
    }

    /// Store a complete property, creating or replacing it.
    fn store(&mut self, o: ObjectId, key: PropertyKey, stored: Stored) -> JsResult<()> {
        if let PropertyKey::Index(i) = key {
            return self.store_element(o, i, stored);
        }
        self.heap.note_named_change(o);
        if self.heap.object(o).dictionary().is_none() {
            let shape = self.heap.object(o).shape;
            match self.heap.shapes.lookup(shape, key) {
                Some(slot) if slot.attributes == stored.attributes => {
                    let object = self.heap.object_mut(o);
                    object.slots[slot.slot as usize] = stored.first;
                    if stored.attributes.is_accessor() {
                        object.slots[slot.slot as usize + 1] = stored.second;
                    }
                    return Ok(());
                }
                Some(_) => self.make_dictionary(o),
                None => {
                    let before = self.heap.shapes.len();
                    match self.heap.shapes.add(shape, key, stored.attributes) {
                        Some(next) => {
                            if self.heap.shapes.len() != before {
                                self.heap.charge(64)?;
                            }
                            self.heap.charge(8 * stored.attributes.slots() as usize)?;
                            let object = self.heap.object_mut(o);
                            object.shape = next;
                            object.slots.push(stored.first);
                            if stored.attributes.is_accessor() {
                                object.slots.push(stored.second);
                            }
                            return Ok(());
                        }
                        None => self.make_dictionary(o),
                    }
                }
            }
        }
        self.heap.charge(48)?;
        let object = self.heap.object_mut(o);
        object
            .extra_mut()
            .dictionary
            .get_or_insert_with(Dictionary::default)
            .set(key, stored);
        Ok(())
    }

    fn store_element(&mut self, o: ObjectId, i: u32, stored: Stored) -> JsResult<()> {
        let index = i as usize;
        let plain = stored.attributes == Attributes::DEFAULT;
        let object = self.heap.object(o);
        let dense_ok =
            plain && object.sparse().is_none() && index <= object.elements.len() * 2 + DENSE_SLACK;
        if dense_ok {
            let grow = (index + 1).saturating_sub(object.elements.len());
            self.heap.charge(8 * grow)?;
            let object = self.heap.object_mut(o);
            if grow > 0 {
                object.elements.resize(index + 1, Value::UNINITIALIZED);
            }
            object.elements[index] = stored.first;
            return Ok(());
        }
        self.make_sparse(o);
        self.heap.charge(48)?;
        let object = self.heap.object_mut(o);
        object
            .extra_mut()
            .sparse
            .get_or_insert_with(BTreeMap::new)
            .insert(i, stored);
        Ok(())
    }

    fn remove(&mut self, o: ObjectId, key: PropertyKey) {
        if let PropertyKey::Index(i) = key {
            let object = self.heap.object_mut(o);
            if let Some(sparse) = object.extra.as_mut().and_then(|e| e.sparse.as_mut()) {
                sparse.remove(&i);
            } else if let Some(slot) = object.elements.get_mut(i as usize) {
                *slot = Value::UNINITIALIZED;
                while object.elements.last() == Some(&Value::UNINITIALIZED) {
                    object.elements.pop();
                }
            }
            return;
        }
        self.heap.note_named_change(o);
        if self.heap.object(o).dictionary().is_none() {
            let shape = self.heap.object(o).shape;
            if let Some(parent) = self.heap.shapes.remove_last(shape, key) {
                let count = self.heap.shapes.slot_count(parent) as usize;
                let object = self.heap.object_mut(o);
                object.shape = parent;
                object.slots.truncate(count);
                return;
            }
            self.make_dictionary(o);
        }
        if let Some(d) = self
            .heap
            .object_mut(o)
            .extra
            .as_mut()
            .and_then(|e| e.dictionary.as_mut())
        {
            d.remove(key);
        }
    }

    // ---- ordinary internal methods ------------------------------------

    /// `[[GetPrototypeOf]]`.
    pub fn get_prototype_of(&self, o: ObjectId) -> Option<ObjectId> {
        self.heap.object(o).proto()
    }

    /// `[[SetPrototypeOf]]` (OrdinarySetPrototypeOf).
    pub fn set_prototype_of(&mut self, o: ObjectId, proto: Option<ObjectId>) -> bool {
        let current = self.heap.object(o).proto();
        if current == proto {
            return true;
        }
        if !self.is_extensible(o) {
            return false;
        }
        let mut p = proto;
        while let Some(candidate) = p {
            if candidate == o {
                return false;
            }
            p = self.heap.object(candidate).proto();
        }
        // Whatever was cached about this object's chain, or about chains
        // through it, no longer holds.
        self.heap.proto_epoch = self.heap.proto_epoch.wrapping_add(1);
        if let Some(p) = proto {
            self.heap
                .object_mut(p)
                .set_flag(super::object::FLAG_PROTOTYPE, true);
        }
        self.heap.object_mut(o).set_proto(proto);
        true
    }

    /// `[[IsExtensible]]`.
    pub fn is_extensible(&self, o: ObjectId) -> bool {
        self.heap.object(o).has_flag(FLAG_EXTENSIBLE)
    }

    /// `[[PreventExtensions]]`.
    pub fn prevent_extensions(&mut self, o: ObjectId) -> bool {
        self.heap.object_mut(o).set_flag(FLAG_EXTENSIBLE, false);
        true
    }

    /// `[[GetOwnProperty]]`.
    pub fn get_own_property(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
    ) -> JsResult<Option<PropertyDescriptor>> {
        match self.locate(o, key) {
            Some(location) => self.describe(o, location).map(Some),
            None => Ok(None),
        }
    }

    /// `[[DefineOwnProperty]]`, dispatching to the Array and String exotic
    /// definitions.
    pub fn define_own_property(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        match self.heap.object(o).class {
            Class::Array => self.array_define_own_property(o, key, desc),
            Class::String => {
                if let PropertyKey::Index(i) = key {
                    if let Some(unit) = self.string_data(o).and_then(|s| s.unit(i as usize)) {
                        let current = self.describe(o, Location::StringIndex(unit))?;
                        return self.validate_only(false, desc, Some(current));
                    }
                }
                self.ordinary_define_own_property(o, key, desc)
            }
            _ => self.ordinary_define_own_property(o, key, desc),
        }
    }

    /// `OrdinaryDefineOwnProperty`.
    pub fn ordinary_define_own_property(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        let current = self.get_own_property(o, key)?;
        let extensible = self.is_extensible(o);
        self.validate_and_apply(Some((o, key)), extensible, desc, current)
    }

    fn validate_only(
        &mut self,
        extensible: bool,
        desc: PropertyDescriptor,
        current: Option<PropertyDescriptor>,
    ) -> JsResult<bool> {
        self.validate_and_apply(None, extensible, desc, current)
    }

    /// `ValidateAndApplyPropertyDescriptor` (§10.1.6.3).
    fn validate_and_apply(
        &mut self,
        target: Option<(ObjectId, PropertyKey)>,
        extensible: bool,
        desc: PropertyDescriptor,
        current: Option<PropertyDescriptor>,
    ) -> JsResult<bool> {
        let Some(current) = current else {
            if !extensible {
                return Ok(false);
            }
            if let Some((o, key)) = target {
                let complete = if desc.is_accessor() {
                    PropertyDescriptor::accessor(
                        desc.get.unwrap_or(Value::UNDEFINED),
                        desc.set.unwrap_or(Value::UNDEFINED),
                        desc.enumerable.unwrap_or(false),
                        desc.configurable.unwrap_or(false),
                    )
                } else {
                    PropertyDescriptor::data_with(
                        desc.value.unwrap_or(Value::UNDEFINED),
                        desc.writable.unwrap_or(false),
                        desc.enumerable.unwrap_or(false),
                        desc.configurable.unwrap_or(false),
                    )
                };
                let (attributes, first, second) = complete.to_stored();
                self.store(
                    o,
                    key,
                    Stored {
                        attributes,
                        first,
                        second,
                    },
                )?;
            }
            return Ok(true);
        };
        if desc == PropertyDescriptor::default() {
            return Ok(true);
        }
        if current.configurable == Some(false) {
            if desc.configurable == Some(true) {
                return Ok(false);
            }
            if desc.enumerable.is_some() && desc.enumerable != current.enumerable {
                return Ok(false);
            }
            if !desc.is_generic() && desc.is_accessor() != current.is_accessor() {
                return Ok(false);
            }
            if current.is_accessor() {
                if let Some(get) = desc.get {
                    if !self.same_value(get, current.get.unwrap_or(Value::UNDEFINED)) {
                        return Ok(false);
                    }
                }
                if let Some(set) = desc.set {
                    if !self.same_value(set, current.set.unwrap_or(Value::UNDEFINED)) {
                        return Ok(false);
                    }
                }
            } else if current.writable == Some(false) {
                if desc.writable == Some(true) {
                    return Ok(false);
                }
                if let Some(value) = desc.value {
                    if !self.same_value(value, current.value.unwrap_or(Value::UNDEFINED)) {
                        return Ok(false);
                    }
                }
            }
        }
        if let Some((o, key)) = target {
            let enumerable = desc.enumerable.or(current.enumerable).unwrap_or(false);
            let configurable = desc.configurable.or(current.configurable).unwrap_or(false);
            let merged = if current.is_data() && desc.is_accessor() {
                PropertyDescriptor::accessor(
                    desc.get.unwrap_or(Value::UNDEFINED),
                    desc.set.unwrap_or(Value::UNDEFINED),
                    enumerable,
                    configurable,
                )
            } else if current.is_accessor() && desc.is_data() {
                PropertyDescriptor::data_with(
                    desc.value.unwrap_or(Value::UNDEFINED),
                    desc.writable.unwrap_or(false),
                    enumerable,
                    configurable,
                )
            } else if current.is_accessor() {
                PropertyDescriptor::accessor(
                    desc.get.or(current.get).unwrap_or(Value::UNDEFINED),
                    desc.set.or(current.set).unwrap_or(Value::UNDEFINED),
                    enumerable,
                    configurable,
                )
            } else {
                PropertyDescriptor::data_with(
                    desc.value.or(current.value).unwrap_or(Value::UNDEFINED),
                    desc.writable.or(current.writable).unwrap_or(false),
                    enumerable,
                    configurable,
                )
            };
            let (attributes, first, second) = merged.to_stored();
            self.store(
                o,
                key,
                Stored {
                    attributes,
                    first,
                    second,
                },
            )?;
        }
        Ok(true)
    }

    /// `[[HasProperty]]`.
    pub fn has_property(&mut self, o: ObjectId, key: PropertyKey) -> JsResult<bool> {
        let mut current = Some(o);
        while let Some(object) = current {
            if self.locate(object, key).is_some() {
                return Ok(true);
            }
            current = self.get_prototype_of(object);
        }
        Ok(false)
    }

    /// `[[Get]]`.
    pub fn get_with_receiver(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        receiver: Value,
    ) -> JsResult<Value> {
        let mut current = Some(o);
        while let Some(object) = current {
            if let Some(location) = self.locate(object, key) {
                // Fast paths that need no descriptor.
                match location {
                    Location::Element(v) => return Ok(v),
                    Location::Slot(slot, attributes) if !attributes.is_accessor() => {
                        let value = self.heap.object(object).slots[slot as usize];
                        if value != Value::UNINITIALIZED {
                            return Ok(value);
                        }
                        // A function's `prototype` not created yet.
                    }
                    _ => {}
                }
                let desc = self.describe(object, location)?;
                if desc.is_accessor() {
                    let getter = desc.get.unwrap_or(Value::UNDEFINED);
                    if getter == Value::UNDEFINED {
                        return Ok(Value::UNDEFINED);
                    }
                    return self.call(getter, receiver, &[]);
                }
                return Ok(desc.value.unwrap_or(Value::UNDEFINED));
            }
            current = self.get_prototype_of(object);
        }
        Ok(Value::UNDEFINED)
    }

    /// `[[Set]]` (OrdinarySet, with the Array/String exotic definitions
    /// reached through `[[DefineOwnProperty]]`).
    pub fn set_with_receiver(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        value: Value,
        receiver: Value,
    ) -> JsResult<bool> {
        // Fast path: a writable own data property on the receiver itself.
        if receiver == Value::object(o) {
            match self.locate(o, key) {
                Some(Location::Slot(slot, attributes))
                    if attributes.has(Attributes::WRITABLE) && !attributes.is_accessor() =>
                {
                    self.heap.note_named_change(o);
                    self.heap.object_mut(o).slots[slot as usize] = value;
                    return Ok(true);
                }
                Some(Location::Element(_)) => {
                    if let PropertyKey::Index(i) = key {
                        self.heap.object_mut(o).elements[i as usize] = value;
                        return Ok(true);
                    }
                }
                _ => {}
            }
        }
        let mut current = o;
        let own = loop {
            if let Some(desc) = self.get_own_property(current, key)? {
                break desc;
            }
            match self.get_prototype_of(current) {
                Some(parent) => current = parent,
                None => break PropertyDescriptor::data(Value::UNDEFINED),
            }
        };
        if own.is_data() {
            if own.writable != Some(true) {
                return Ok(false);
            }
            let Some(receiver) = receiver.as_object() else {
                return Ok(false);
            };
            return match self.get_own_property(receiver, key)? {
                Some(existing) => {
                    if existing.is_accessor() || existing.writable != Some(true) {
                        return Ok(false);
                    }
                    self.define_own_property(
                        receiver,
                        key,
                        PropertyDescriptor {
                            value: Some(value),
                            ..Default::default()
                        },
                    )
                }
                None => self.create_data_property(receiver, key, value),
            };
        }
        let setter = own.set.unwrap_or(Value::UNDEFINED);
        if setter == Value::UNDEFINED {
            return Ok(false);
        }
        self.call(setter, receiver, &[value])?;
        Ok(true)
    }

    /// `[[Delete]]`.
    pub fn delete(&mut self, o: ObjectId, key: PropertyKey) -> JsResult<bool> {
        match self.get_own_property(o, key)? {
            None => Ok(true),
            Some(desc) if desc.configurable == Some(true) => {
                self.remove(o, key);
                Ok(true)
            }
            Some(_) => Ok(false),
        }
    }

    /// `[[OwnPropertyKeys]]`: array indices ascending, then strings and
    /// then symbols in creation order (§10.1.11.1).
    pub fn own_property_keys(&self, o: ObjectId) -> Vec<PropertyKey> {
        let object = self.heap.object(o);
        let mut keys = Vec::new();
        let string_len = self.string_data(o).map_or(0, JsStr::len);
        // String indices are below the source string's length, < 2³².
        keys.extend((0..string_len).map(|i| PropertyKey::Index(i as u32)));
        match object.sparse() {
            Some(sparse) => keys.extend(
                sparse
                    .keys()
                    .filter(|&&i| i as usize >= string_len)
                    .map(|&i| PropertyKey::Index(i)),
            ),
            None => keys.extend(
                object
                    .elements
                    .iter()
                    .enumerate()
                    .filter(|(i, v)| *v != &Value::UNINITIALIZED && *i >= string_len)
                    .map(|(i, _)| PropertyKey::Index(i as u32)),
            ),
        }
        if object.class == Class::Array {
            keys.push(self.length_key());
        }
        let named: Vec<PropertyKey> = match object.dictionary() {
            Some(d) => d.keys().collect(),
            None => self
                .heap
                .shapes
                .properties(object.shape)
                .into_iter()
                .map(|(k, _)| k)
                .collect(),
        };
        keys.extend(named.iter().filter(|k| matches!(k, PropertyKey::String(_))));
        keys.extend(named.iter().filter(|k| matches!(k, PropertyKey::Symbol(_))));
        keys
    }

    // ---- Array exotic -------------------------------------------------

    fn array_define_own_property(
        &mut self,
        a: ObjectId,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        if key == self.length_key() {
            return self.array_set_length(a, desc);
        }
        if let PropertyKey::Index(index) = key {
            let object = self.heap.object(a);
            let length = object.length;
            if index >= length && !object.has_flag(FLAG_ARRAY_LENGTH_WRITABLE) {
                return Ok(false);
            }
            if !self.ordinary_define_own_property(a, key, desc)? {
                return Ok(false);
            }
            if index >= length {
                self.heap.object_mut(a).length = index + 1;
            }
            return Ok(true);
        }
        self.ordinary_define_own_property(a, key, desc)
    }

    /// `ArraySetLength` (§10.4.2.4).
    fn array_set_length(&mut self, a: ObjectId, desc: PropertyDescriptor) -> JsResult<bool> {
        let current = self.describe(a, Location::ArrayLength)?;
        let Some(value) = desc.value else {
            return self.validate_length_attributes(a, desc, current);
        };
        let number = self.to_number(value)?;
        let new_len = to_uint32(number);
        if f64::from(new_len) != number {
            return Err(self.throw(ErrorKind::RangeError, "invalid array length"));
        }
        let old_len = self.heap.object(a).length;
        let length_desc = PropertyDescriptor {
            value: Some(Value::number(f64::from(new_len))),
            ..desc
        };
        if new_len >= old_len {
            if !self.validate_length_attributes(a, length_desc, current)? {
                return Ok(false);
            }
            self.heap.object_mut(a).length = new_len;
            return Ok(true);
        }
        if current.writable == Some(false) {
            return Ok(false);
        }
        let new_writable = desc.writable != Some(false);
        // Validate the attribute change before deleting anything, keeping
        // `length` writable until the deletions are done.
        let probe = PropertyDescriptor {
            writable: None,
            ..length_desc
        };
        if !self.validate_length_attributes(a, probe, current)? {
            return Ok(false);
        }
        let mut indices: Vec<u32> = self
            .own_property_keys(a)
            .into_iter()
            .filter_map(|k| match k {
                PropertyKey::Index(i) if i >= new_len => Some(i),
                _ => None,
            })
            .collect();
        indices.reverse();
        for index in indices {
            if !self.delete(a, PropertyKey::Index(index))? {
                let object = self.heap.object_mut(a);
                object.length = index + 1;
                if !new_writable {
                    object.set_flag(FLAG_ARRAY_LENGTH_WRITABLE, false);
                }
                return Ok(false);
            }
        }
        let object = self.heap.object_mut(a);
        object.length = new_len;
        if !new_writable {
            object.set_flag(FLAG_ARRAY_LENGTH_WRITABLE, false);
        }
        Ok(true)
    }

    fn validate_length_attributes(
        &mut self,
        a: ObjectId,
        desc: PropertyDescriptor,
        current: PropertyDescriptor,
    ) -> JsResult<bool> {
        if !self.validate_only(false, desc, Some(current))? {
            return Ok(false);
        }
        if desc.writable == Some(false) {
            self.heap
                .object_mut(a)
                .set_flag(FLAG_ARRAY_LENGTH_WRITABLE, false);
        }
        Ok(true)
    }

    // ---- abstract operations (§7.3) -----------------------------------

    /// `Get(O, P)`.
    pub fn get(&mut self, o: ObjectId, key: PropertyKey) -> JsResult<Value> {
        self.get_with_receiver(o, key, Value::object(o))
    }

    /// `GetV(V, P)`: property lookup on any value, starting at a
    /// primitive's prototype without allocating a wrapper.
    pub fn get_v(&mut self, value: Value, key: PropertyKey) -> JsResult<Value> {
        let start = match value.kind() {
            Kind::Object(o) => o,
            Kind::String(s) => {
                let string = self.heap.string(s);
                if key == self.length_key() {
                    return Ok(Value::number(string.len() as f64));
                }
                if let PropertyKey::Index(i) = key {
                    if let Some(unit) = string.unit(i as usize) {
                        return Ok(Value::string(
                            self.heap.intern(&JsStr::from_units(&[unit]))?,
                        ));
                    }
                }
                self.intrinsics.string_prototype
            }
            Kind::Number(_) => self.intrinsics.number_prototype,
            Kind::Boolean(_) => self.intrinsics.boolean_prototype,
            Kind::Symbol(_) => self.intrinsics.symbol_prototype,
            Kind::BigInt(_) => self.intrinsics.bigint_prototype,
            Kind::Undefined | Kind::Null | Kind::Uninitialized => {
                return self.type_error("cannot read properties of undefined or null");
            }
        };
        self.get_with_receiver(start, key, value)
    }

    /// `Set(O, P, V, Throw)`.
    pub fn set(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        value: Value,
        throw: bool,
    ) -> JsResult<()> {
        if !self.set_with_receiver(o, key, value, Value::object(o))? && throw {
            return self.type_error("cannot assign to read-only property");
        }
        Ok(())
    }

    /// `CreateDataProperty(O, P, V)`.
    pub fn create_data_property(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        value: Value,
    ) -> JsResult<bool> {
        self.define_own_property(o, key, PropertyDescriptor::data(value))
    }

    /// `CreateDataPropertyOrThrow(O, P, V)`.
    pub fn create_data_property_or_throw(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        value: Value,
    ) -> JsResult<()> {
        if !self.create_data_property(o, key, value)? {
            return self.type_error("cannot define property");
        }
        Ok(())
    }

    /// `DefinePropertyOrThrow(O, P, desc)`.
    pub fn define_property_or_throw(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<()> {
        if !self.define_own_property(o, key, desc)? {
            return self.type_error("cannot redefine property");
        }
        Ok(())
    }

    /// `DeletePropertyOrThrow(O, P)`.
    pub fn delete_property_or_throw(&mut self, o: ObjectId, key: PropertyKey) -> JsResult<()> {
        if !self.delete(o, key)? {
            return self.type_error("cannot delete property");
        }
        Ok(())
    }

    /// `HasOwnProperty(O, P)`.
    pub fn has_own_property(&mut self, o: ObjectId, key: PropertyKey) -> JsResult<bool> {
        Ok(self.locate(o, key).is_some())
    }

    /// `GetMethod(V, P)`: undefined or a callable, else a TypeError.
    pub fn get_method(&mut self, value: Value, key: PropertyKey) -> JsResult<Option<Value>> {
        let f = self.get_v(value, key)?;
        if f.is_nullish() {
            return Ok(None);
        }
        if !self.is_callable(f) {
            return self.type_error("property is not a function");
        }
        Ok(Some(f))
    }

    /// `Invoke(V, P, args)`.
    pub fn invoke(&mut self, value: Value, key: PropertyKey, args: &[Value]) -> JsResult<Value> {
        let f = self.get_v(value, key)?;
        self.call(f, value, args)
    }

    /// `LengthOfArrayLike(O)`.
    pub fn length_of_array_like(&mut self, o: ObjectId) -> JsResult<f64> {
        let length = self.get(o, self.length_key())?;
        let n = self.to_number(length)?;
        Ok(super::number::to_length(n))
    }

    /// `CreateArrayFromList(elements)`.
    pub fn create_array_from_list(&mut self, elements: &[Value]) -> JsResult<ObjectId> {
        let a = self.new_array()?;
        self.heap.charge(8 * elements.len())?;
        let object = self.heap.object_mut(a);
        object.elements = elements.to_vec();
        // The heap budget keeps lists far below 2³² elements.
        object.length = elements.len() as u32;
        Ok(a)
    }

    /// `SetIntegrityLevel(O, level)`.
    pub fn set_integrity_level(&mut self, o: ObjectId, level: IntegrityLevel) -> JsResult<bool> {
        if !self.prevent_extensions(o) {
            return Ok(false);
        }
        for key in self.own_property_keys(o) {
            let desc = match level {
                IntegrityLevel::Sealed => PropertyDescriptor {
                    configurable: Some(false),
                    ..Default::default()
                },
                IntegrityLevel::Frozen => {
                    let Some(current) = self.get_own_property(o, key)? else {
                        continue;
                    };
                    if current.is_accessor() {
                        PropertyDescriptor {
                            configurable: Some(false),
                            ..Default::default()
                        }
                    } else {
                        PropertyDescriptor {
                            configurable: Some(false),
                            writable: Some(false),
                            ..Default::default()
                        }
                    }
                }
            };
            self.define_property_or_throw(o, key, desc)?;
        }
        Ok(true)
    }

    /// `TestIntegrityLevel(O, level)`.
    pub fn test_integrity_level(&mut self, o: ObjectId, level: IntegrityLevel) -> JsResult<bool> {
        if self.is_extensible(o) {
            return Ok(false);
        }
        for key in self.own_property_keys(o) {
            if let Some(desc) = self.get_own_property(o, key)? {
                if desc.configurable == Some(true) {
                    return Ok(false);
                }
                if level == IntegrityLevel::Frozen && desc.writable == Some(true) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}
