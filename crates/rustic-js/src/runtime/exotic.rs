//! Exotic objects whose internal methods differ from the ordinary ones
//! (§10.4, §10.5): the ordinary methods in `internal.rs` hand any object
//! flagged [`FLAG_EXOTIC`](super::object::FLAG_EXOTIC) to the dispatch
//! here. Proxies are in `builtins/proxy.rs`; mapped arguments objects
//! (§10.4.4) are below.

use super::agent::{Agent, JsResult};
use super::builtins::{proxy, typed_array};
use super::env::EnvId;
use super::object::{
    Class, FLAG_EXOTIC, FLAG_EXTENSIBLE, Internal, NamespaceBinding, Object, PropertyDescriptor,
};
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
    TypedArray,
    ModuleNamespace,
}

impl Agent {
    fn exotic_kind(&self, o: ObjectId) -> Exotic {
        match self.heap.object(o).internal() {
            Some(Internal::Proxy { .. }) => Exotic::Proxy,
            Some(Internal::Arguments { .. }) => Exotic::Arguments,
            Some(Internal::TypedArray { .. }) => Exotic::TypedArray,
            Some(Internal::ModuleNamespace(..)) => Exotic::ModuleNamespace,
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
            Exotic::Arguments | Exotic::TypedArray | Exotic::ModuleNamespace => {
                Ok(self.heap.object(o).proto())
            }
        }
    }

    pub(crate) fn exotic_set_prototype_of(
        &mut self,
        o: ObjectId,
        proto: Option<ObjectId>,
    ) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::set_prototype_of(a, o, proto)),
            Exotic::Arguments | Exotic::TypedArray | Exotic::ModuleNamespace => {
                Ok(self.ordinary_set_prototype_of(o, proto))
            }
        }
    }

    pub(crate) fn exotic_is_extensible(&mut self, o: ObjectId) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::is_extensible(a, o)),
            Exotic::Arguments | Exotic::TypedArray | Exotic::ModuleNamespace => {
                Ok(self.heap.object(o).has_flag(super::object::FLAG_EXTENSIBLE))
            }
        }
    }

    pub(crate) fn exotic_prevent_extensions(&mut self, o: ObjectId) -> JsResult<bool> {
        match self.exotic_kind(o) {
            Exotic::Proxy => self.in_proxy(|a| proxy::prevent_extensions(a, o)),
            Exotic::Arguments | Exotic::TypedArray | Exotic::ModuleNamespace => {
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
            Exotic::TypedArray => typed_array::get_own_property(self, o, key),
            Exotic::ModuleNamespace => match self.namespace_binding(o, key) {
                Some(binding) => Ok(Some(PropertyDescriptor::data_with(
                    self.namespace_value(binding)?,
                    true,
                    true,
                    false,
                ))),
                None => self.ordinary_get_own_property(o, key),
            },
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
            Exotic::TypedArray => typed_array::define_own_property(self, o, key, desc),
            Exotic::ModuleNamespace => {
                if matches!(key, PropertyKey::Symbol(_)) {
                    return self.ordinary_define_own_property(o, key, desc);
                }
                let Some(current) = self.exotic_get_own_property(o, key)? else {
                    return Ok(false);
                };
                Ok(!desc.is_accessor()
                    && desc.configurable != Some(true)
                    && desc.enumerable != Some(false)
                    && desc.writable != Some(false)
                    && desc
                        .value
                        .is_none_or(|value| self.same_value(value, current.value.unwrap())))
            }
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
            Exotic::TypedArray => typed_array::has_property(self, o, key),
            Exotic::Arguments => self.has_property_from(o, key, true),
            Exotic::ModuleNamespace => Ok(self.namespace_binding(o, key).is_some()
                || self.ordinary_get_own_property(o, key)?.is_some()),
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
            Exotic::TypedArray => typed_array::get(self, o, key, receiver),
            Exotic::ModuleNamespace => match self.namespace_binding(o, key) {
                Some(binding) => self.namespace_value(binding),
                None => self.get_from(o, key, receiver, true),
            },
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
            Exotic::TypedArray => typed_array::set(self, o, key, value, receiver),
            Exotic::ModuleNamespace => Ok(false),
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
            Exotic::TypedArray => typed_array::delete(self, o, key),
            Exotic::ModuleNamespace => {
                if self.namespace_binding(o, key).is_some() {
                    Ok(false)
                } else {
                    self.ordinary_delete(o, key)
                }
            }
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
            Exotic::TypedArray => Ok(typed_array::own_property_keys(self, o)),
            Exotic::ModuleNamespace => Ok(self
                .namespace_exports(o)
                .iter()
                .map(|&(key, _)| key)
                .chain(self.ordinary_own_property_keys(o))
                .collect()),
            Exotic::Arguments => Ok(self.ordinary_own_property_keys(o)),
        }
    }

    // ---- module namespace objects (§10.4.6) ----------------------------

    pub(crate) fn new_module_namespace(
        &mut self,
        env: crate::runtime::EnvId,
    ) -> JsResult<ObjectId> {
        let mut object = Object::new(Class::ModuleNamespace, None);
        object.extra_mut().internal = Some(Internal::ModuleNamespace(Box::default(), env));
        let namespace = self.heap.alloc_object(object)?;
        let tag = self.string("Module")?;
        let key = PropertyKey::Symbol(self.symbols.to_string_tag);
        self.ordinary_define_own_property(
            namespace,
            key,
            PropertyDescriptor::data_with(tag, false, false, false),
        )?;
        let object = self.heap.object_mut(namespace);
        object.set_flag(FLAG_EXTENSIBLE, false);
        object.set_flag(FLAG_EXOTIC, true);
        Ok(namespace)
    }

    pub(crate) fn set_namespace_exports(
        &mut self,
        namespace: ObjectId,
        exports: Vec<(PropertyKey, NamespaceBinding)>,
    ) -> JsResult<()> {
        self.heap
            .charge(std::mem::size_of_val(exports.as_slice()))?;
        let Some(Internal::ModuleNamespace(entries, _)) =
            self.heap.object_mut(namespace).internal_mut()
        else {
            unreachable!("export entries belong to a module namespace");
        };
        *entries = exports.into_boxed_slice();
        Ok(())
    }

    fn namespace_exports(&self, namespace: ObjectId) -> &[(PropertyKey, NamespaceBinding)] {
        let Some(Internal::ModuleNamespace(entries, _)) = self.heap.object(namespace).internal()
        else {
            unreachable!("export entries belong to a module namespace");
        };
        entries
    }

    pub(crate) fn module_namespace_environment(
        &self,
        namespace: ObjectId,
    ) -> crate::runtime::EnvId {
        let Some(Internal::ModuleNamespace(_, env)) = self.heap.object(namespace).internal() else {
            unreachable!("only module namespaces can be reused");
        };
        *env
    }

    fn namespace_binding(&self, namespace: ObjectId, key: PropertyKey) -> Option<NamespaceBinding> {
        self.namespace_exports(namespace)
            .iter()
            .find_map(|&(name, binding)| (name == key).then_some(binding))
    }

    fn namespace_value(&mut self, binding: NamespaceBinding) -> JsResult<Value> {
        match binding {
            NamespaceBinding::Namespace(object) => Ok(Value::object(object)),
            NamespaceBinding::Local(env, slot) => {
                let value = self.env_slot(env, slot);
                if value == Value::UNINITIALIZED {
                    let (name, _) = self.env_slot_binding(env, slot);
                    return Err(self.tdz_error(name));
                }
                Ok(value)
            }
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
