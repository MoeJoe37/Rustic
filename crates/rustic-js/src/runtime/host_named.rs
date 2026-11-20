//! Web IDL legacy platform objects with a named getter, setter and
//! deleter and no indexed properties (Web IDL §3.9, as `Storage` uses
//! them, HTML §12.2.1): every string key, including `"0"`, is a name.

use super::agent::{Agent, JsResult};
use super::object::{Class, FLAG_EXOTIC, Internal, Object, PropertyDescriptor};
use super::shape::PropertyKey;
use super::value::{ObjectId, StringId, Value};

/// Native named-property operations over a traced source value. They may
/// throw (a setter's quota error) but must not run script.
#[derive(Debug)]
pub struct NamedPropertyAccess {
    /// Whether `name` is a supported property name.
    pub contains: fn(&mut Agent, Value, StringId) -> JsResult<bool>,
    /// The named property getter: the value of a supported name.
    pub get: fn(&mut Agent, Value, StringId) -> JsResult<Option<Value>>,
    /// The named property setter (it converts the value itself).
    pub set: fn(&mut Agent, Value, StringId, Value) -> JsResult<()>,
    /// The named property deleter.
    pub delete: fn(&mut Agent, Value, StringId) -> JsResult<()>,
    /// The supported property names, in order.
    pub names: fn(&mut Agent, Value) -> JsResult<Vec<StringId>>,
}

#[derive(Clone, Debug)]
pub(crate) struct NamedHost {
    interface: &'static str,
    source: Value,
    access: &'static NamedPropertyAccess,
}

impl NamedHost {
    pub(crate) fn roots(&self) -> &[Value] {
        std::slice::from_ref(&self.source)
    }
}

impl Agent {
    /// Allocate a branded legacy platform object whose named properties
    /// come from `source` through `access`.
    pub fn new_named_host(
        &mut self,
        interface: &'static str,
        prototype: ObjectId,
        source: Value,
        access: &'static NamedPropertyAccess,
    ) -> JsResult<ObjectId> {
        let mut object = Object::new(Class::Host, Some(prototype));
        object.set_flag(FLAG_EXOTIC, true);
        object.extra_mut().internal = Some(Internal::HostNamed(NamedHost {
            interface,
            source,
            access,
        }));
        Ok(self.heap.alloc_object(object)?)
    }

    /// The source of a named host object with this interface's brand.
    pub fn named_host_source(&self, object: ObjectId, interface: &'static str) -> Option<Value> {
        match self
            .heap
            .objects
            .try_get(object.0)
            .and_then(Object::internal)
        {
            Some(Internal::HostNamed(host)) if host.interface == interface => Some(host.source),
            _ => None,
        }
    }

    fn named_host(&self, object: ObjectId) -> (Value, &'static NamedPropertyAccess) {
        let Some(Internal::HostNamed(host)) = self.heap.object(object).internal() else {
            unreachable!("missing named host data");
        };
        (host.source, host.access)
    }

    /// The property name a key denotes, or `None` for symbols.
    fn key_name(&mut self, key: PropertyKey) -> JsResult<Option<StringId>> {
        Ok(match key {
            PropertyKey::Index(index) => Some(self.heap.intern_str(&index.to_string())?),
            PropertyKey::String(name) => Some(name),
            PropertyKey::Symbol(_) => None,
        })
    }

    /// Web IDL §3.9.1 "named property visibility algorithm" (no
    /// `[LegacyOverrideBuiltIns]`): supported, not an own property, and not
    /// on the prototype chain.
    fn named_visible(&mut self, object: ObjectId, key: PropertyKey) -> JsResult<Option<StringId>> {
        let Some(name) = self.key_name(key)? else {
            return Ok(None);
        };
        self.with_rooted(Value::object(object), |agent| {
            agent.with_rooted(Value::string(name), |agent| {
                let (source, access) = agent.named_host(object);
                if !(access.contains)(agent, source, name)? {
                    return Ok(None);
                }
                if agent.ordinary_get_own_property(object, key)?.is_some() {
                    return Ok(None);
                }
                let parent = agent.stored_prototype(object);
                Ok(agent.prototype_allows_name(parent, key)?.then_some(name))
            })
        })
    }

    fn named_value(&mut self, object: ObjectId, name: StringId) -> JsResult<Value> {
        let (source, access) = self.named_host(object);
        Ok((access.get)(self, source, name)?.unwrap_or(Value::UNDEFINED))
    }

    pub(crate) fn host_named_get_own_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<Option<PropertyDescriptor>> {
        if let Some(name) = self.named_visible(object, key)? {
            let value = self.named_value(object, name)?;
            // Writable (a setter exists), enumerable (no
            // [LegacyUnenumerableNamedProperties]) and configurable.
            return Ok(Some(PropertyDescriptor::data_with(value, true, true, true)));
        }
        self.ordinary_get_own_property(object, key)
    }

    pub(crate) fn host_named_define_own_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        // Web IDL §3.9.3 step 2: without an own property, a definition
        // becomes a call to the named setter.
        if let Some(name) = self.key_name(key)? {
            if self.ordinary_get_own_property(object, key)?.is_none() {
                if !desc.is_data() {
                    return Ok(false);
                }
                let (source, access) = self.named_host(object);
                let value = desc.value.unwrap_or(Value::UNDEFINED);
                (access.set)(self, source, name, value)?;
                return Ok(true);
            }
        }
        self.ordinary_define_own_property(object, key, desc)
    }

    pub(crate) fn host_named_has_property(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<bool> {
        Ok(self.named_visible(object, key)?.is_some()
            || self.has_property_from(object, key, true)?)
    }

    pub(crate) fn host_named_get(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        receiver: Value,
    ) -> JsResult<Value> {
        if let Some(name) = self.named_visible(object, key)? {
            return self.named_value(object, name);
        }
        self.get_from(object, key, receiver, true)
    }

    pub(crate) fn host_named_set(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
        value: Value,
        receiver: Value,
    ) -> JsResult<bool> {
        // Web IDL §3.9.4: a write through the object itself calls the setter.
        if receiver == Value::object(object) {
            if let Some(name) = self.key_name(key)? {
                let (source, access) = self.named_host(object);
                (access.set)(self, source, name, value)?;
                return Ok(true);
            }
        }
        self.set_from(object, key, value, receiver, true)
    }

    pub(crate) fn host_named_delete(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<bool> {
        if let Some(name) = self.named_visible(object, key)? {
            let (source, access) = self.named_host(object);
            (access.delete)(self, source, name)?;
            return Ok(true);
        }
        self.ordinary_delete(object, key)
    }

    pub(crate) fn host_named_keys(&mut self, object: ObjectId) -> JsResult<Vec<PropertyKey>> {
        let (source, access) = self.named_host(object);
        let names = (access.names)(self, source)?;
        self.with_rooted_names(object, &names, |agent| {
            let own = agent.ordinary_own_property_keys(object);
            let mut keys = Vec::new();
            keys.try_reserve_exact(names.len() + own.len())
                .map_err(|_| super::heap::OutOfMemory)?;
            for &name in &names {
                let key = agent.heap.key_for_string(name)?;
                if agent.named_visible(object, key)?.is_some() {
                    keys.push(key);
                }
            }
            keys.extend(own);
            Ok(keys)
        })
    }
}
