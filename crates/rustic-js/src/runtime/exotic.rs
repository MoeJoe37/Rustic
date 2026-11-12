//! Exotic objects whose internal methods differ from the ordinary ones
//! (§10.4, §10.5): the ordinary methods in `internal.rs` hand any object
//! flagged [`FLAG_EXOTIC`](super::object::FLAG_EXOTIC) to the dispatch
//! here. Proxies are in `builtins/proxy.rs`; mapped arguments objects
//! (§10.4.4) are below.

use super::agent::{Agent, JsResult};
use super::builtins::proxy;
use super::env::EnvId;
use super::object::{Internal, PropertyDescriptor};
use super::shape::PropertyKey;
use super::value::{ObjectId, Value};

/// How deep proxies may nest through their targets, handlers and
/// prototype chains in one operation.
const MAX_PROXY_DEPTH: u32 = 128;

/// Which exotic behavior an object has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exotic {
    Proxy,
    Arguments,
}

impl Agent {
    fn exotic_kind(&self, o: ObjectId) -> Exotic {
        match self.heap.object(o).internal() {
            Some(Internal::Proxy { .. }) => Exotic::Proxy,
            Some(Internal::Arguments { .. }) => Exotic::Arguments,
            _ => unreachable!("only exotic objects carry the exotic flag"),
        }
    }

    /// Run one internal method of a proxy. Proxies of proxies recurse in
    /// native code without a frame of their own, so their depth has its
    /// own bound.
    pub(crate) fn in_proxy<T>(&mut self, f: impl FnOnce(&mut Agent) -> JsResult<T>) -> JsResult<T> {
        if self.vm.proxy_depth >= MAX_PROXY_DEPTH {
            return Err(self.throw(
                super::agent::ErrorKind::RangeError,
                "Maximum call stack size exceeded",
            ));
        }
        self.vm.proxy_depth += 1;
        let result = f(self);
        self.vm.proxy_depth -= 1;
        result
    }

    /// Whether `o` is a Proxy exotic object.
    pub(crate) fn is_exotic_proxy(&self, o: ObjectId) -> bool {
        matches!(self.heap.object(o).internal(), Some(Internal::Proxy { .. }))
    }

    pub(crate) fn exotic_get_prototype_of(&mut self, o: ObjectId) -> JsResult<Option<ObjectId>> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::get_prototype_of(a, o)),
            Exotic::Arguments => Ok(self.heap.object(o).proto()),
        }
    }

    pub(crate) fn exotic_set_prototype_of(
        &mut self,
        o: ObjectId,
        proto: Option<ObjectId>,
    ) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::set_prototype_of(a, o, proto)),
            Exotic::Arguments => Ok(self.ordinary_set_prototype_of(o, proto)),
        }
    }

    pub(crate) fn exotic_is_extensible(&mut self, o: ObjectId) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::is_extensible(a, o)),
            Exotic::Arguments => Ok(self.heap.object(o).has_flag(super::object::FLAG_EXTENSIBLE)),
        }
    }

    pub(crate) fn exotic_prevent_extensions(&mut self, o: ObjectId) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::prevent_extensions(a, o)),
            Exotic::Arguments => {
                self.heap
                    .object_mut(o)
                    .set_flag(super::object::FLAG_EXTENSIBLE, false);
                Ok(true)
            }
        }
    }

    pub(crate) fn exotic_get_own_property(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
    ) -> JsResult<Option<PropertyDescriptor>> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::get_own_property(a, o, key)),
            Exotic::Arguments => {
                // §10.4.4.1: a mapped index reports the parameter's value.
                let Some(mut desc) = self.ordinary_get_own_property(o, key)? else {
                    return Ok(None);
                };
                if let Some((env, slot)) = self.mapped_parameter(o, key) {
                    desc.value = Some(self.env_slot(env, slot));
                }
                Ok(Some(desc))
            }
        }
    }

    pub(crate) fn exotic_define_own_property(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        desc: PropertyDescriptor,
    ) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::define_own_property(a, o, key, desc)),
            Exotic::Arguments => {
                // §10.4.4.2.
                let mapped = self.mapped_parameter(o, key);
                let mut new_desc = desc;
                if let Some((env, slot)) = mapped {
                    if desc.is_data() && desc.value.is_none() && desc.writable == Some(false) {
                        new_desc.value = Some(self.env_slot(env, slot));
                    }
                }
                if !self.ordinary_define_own_property(o, key, new_desc)? {
                    return Ok(false);
                }
                if let Some((env, slot)) = mapped {
                    if desc.is_accessor() {
                        self.unmap_parameter(o, key);
                    } else {
                        if let Some(value) = desc.value {
                            self.set_env_slot(env, slot, value);
                        }
                        if desc.writable == Some(false) {
                            self.unmap_parameter(o, key);
                        }
                    }
                }
                Ok(true)
            }
        }
    }

    pub(crate) fn exotic_has_property(&mut self, o: ObjectId, key: PropertyKey) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::has_property(a, o, key)),
            Exotic::Arguments => self.has_property_from(o, key, true),
        }
    }

    pub(crate) fn exotic_get(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        receiver: Value,
    ) -> JsResult<Value> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::get(a, o, key, receiver)),
            Exotic::Arguments => match self.mapped_parameter(o, key) {
                // §10.4.4.3.
                Some((env, slot)) => Ok(self.env_slot(env, slot)),
                None => self.get_from(o, key, receiver, true),
            },
        }
    }

    pub(crate) fn exotic_set(
        &mut self,
        o: ObjectId,
        key: PropertyKey,
        value: Value,
        receiver: Value,
    ) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::set(a, o, key, value, receiver)),
            Exotic::Arguments => {
                // §10.4.4.4: only a write through the object itself
                // reaches the parameter.
                if receiver == Value::object(o) {
                    if let Some((env, slot)) = self.mapped_parameter(o, key) {
                        self.set_env_slot(env, slot, value);
                    }
                }
                self.set_from(o, key, value, receiver, true)
            }
        }
    }

    pub(crate) fn exotic_delete(&mut self, o: ObjectId, key: PropertyKey) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::delete(a, o, key)),
            Exotic::Arguments => {
                // §10.4.4.5.
                let deleted = self.ordinary_delete(o, key)?;
                if deleted {
                    self.unmap_parameter(o, key);
                }
                Ok(deleted)
            }
        }
    }

    pub(crate) fn exotic_own_property_keys(&mut self, o: ObjectId) -> JsResult<Vec<PropertyKey>> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::own_property_keys(a, o)),
            Exotic::Arguments => Ok(self.ordinary_own_property_keys(o)),
        }
    }

    // ---- mapped arguments objects (§10.4.4) -----------------------------

    /// The parameter `key` is mapped to: the function's environment and
    /// the parameter's slot in it.
    fn mapped_parameter(&self, o: ObjectId, key: PropertyKey) -> Option<(EnvId, u16)> {
        let PropertyKey::Index(index) = key else {
            return None;
        };
        match self.heap.object(o).internal() {
            Some(Internal::Arguments { env, slots }) => {
                let slot = (*slots.get(index as usize)?)?;
                Some((*env, slot))
            }
            _ => None,
        }
    }

    /// Remove `key` from the parameter map.
    fn unmap_parameter(&mut self, o: ObjectId, key: PropertyKey) {
        let PropertyKey::Index(index) = key else {
            return;
        };
        if let Some(Internal::Arguments { slots, .. }) = self.heap.object_mut(o).internal_mut() {
            if let Some(slot) = slots.get_mut(index as usize) {
                *slot = None;
            }
        }
    }
}
