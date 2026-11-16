//! Read-only indexed and named legacy platform properties (Web IDL §3.9).

use super::agent::{Agent, JsResult};
use super::heap::OutOfMemory;
use super::object::{Class, FLAG_EXOTIC, Internal, Object};
use super::shape::PropertyKey;
use super::value::{ObjectId, StringId, Value};

/// Native accessors for a live indexed host object. Indices are contiguous
/// from zero to the length. Accessors must finish without running script.
#[derive(Debug)]
pub struct LiveIndexAccess {
    /// Read the current number of supported indices from the rooted source.
    pub length: fn(&mut Agent, Value) -> JsResult<u32>,
    /// Read the current value at an index, or `None` beyond the end.
    pub item: fn(&mut Agent, Value, u32) -> JsResult<Option<Value>>,
    /// Optional non-enumerable named properties, such as HTMLCollection names.
    pub named: Option<&'static LiveNameAccess>,
}

/// Native queries for Web IDL named properties without LegacyOverrideBuiltIns.
#[derive(Debug)]
pub struct LiveNameAccess {
    /// Test whether the collection has the name, without building its wrapper.
    pub contains: fn(&mut Agent, Value, StringId) -> JsResult<bool>,
    /// Find a value by name, skipping index and prototype visibility rules.
    pub item: fn(&mut Agent, Value, StringId) -> JsResult<Option<Value>>,
    /// Distinct non-index names, in supported property order.
    pub keys: fn(&mut Agent, Value) -> JsResult<Vec<StringId>>,
}

#[derive(Clone, Debug)]
enum IndexSource {
    Snapshot(Box<[Value]>),
    Live {
        source: Value,
        access: &'static LiveIndexAccess,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct IndexedHost {
    interface: &'static str,
    source: IndexSource,
}

impl IndexedHost {
    pub(crate) fn roots(&self) -> &[Value] {
        match &self.source {
            IndexSource::Snapshot(values) => values,
            IndexSource::Live { source, .. } => std::slice::from_ref(source),
        }
    }

    pub(crate) fn footprint(&self) -> usize {
        match &self.source {
            IndexSource::Snapshot(values) => std::mem::size_of_val(values.as_ref()),
            IndexSource::Live { .. } => 0,
        }
    }
}

impl Agent {
    /// Allocate a branded host object with fixed read-only indexed values.
    pub fn new_indexed_snapshot(
        &mut self,
        interface: &'static str,
        prototype: ObjectId,
        values: Box<[Value]>,
    ) -> JsResult<ObjectId> {
        u32::try_from(values.len()).map_err(|_| OutOfMemory)?;
        self.new_indexed_host(interface, prototype, IndexSource::Snapshot(values))
    }

    /// Allocate a branded host object whose indices come from a traced source.
    pub fn new_indexed_view(
        &mut self,
        interface: &'static str,
        prototype: ObjectId,
        source: Value,
        access: &'static LiveIndexAccess,
    ) -> JsResult<ObjectId> {
        self.new_indexed_host(interface, prototype, IndexSource::Live { source, access })
    }

    fn new_indexed_host(
        &mut self,
        interface: &'static str,
        prototype: ObjectId,
        source: IndexSource,
    ) -> JsResult<ObjectId> {
        let data = IndexedHost { interface, source };
        let bytes = data.footprint();
        self.heap.charge(bytes)?;
        let mut object = Object::new(Class::Host, Some(prototype));
        object.set_flag(FLAG_EXOTIC, true);
        object.extra_mut().internal = Some(Internal::HostIndexed(data));
        match self.heap.alloc_object(object) {
            Ok(id) => Ok(id),
            Err(error) => {
                let _ = self.heap.budget.release(bytes);
                Err(error.into())
            }
        }
    }

    /// Whether the given receiver has this host interface's private brand.
    pub fn has_indexed_brand(&self, object: ObjectId, interface: &'static str) -> bool {
        matches!(
            self.heap.objects.try_get(object.0).and_then(Object::internal),
            Some(Internal::HostIndexed(indexed)) if indexed.interface == interface
        )
    }

    fn indexed_host(&self, object: ObjectId) -> &IndexedHost {
        let Some(Internal::HostIndexed(data)) = self.heap.object(object).internal() else {
            unreachable!("missing indexed host data");
        };
        data
    }

    /// The count of supported indices of a branded indexed host object.
    pub fn indexed_host_len(&mut self, object: ObjectId) -> JsResult<u32> {
        match self.indexed_host(object).source {
            IndexSource::Snapshot(ref values) => {
                Ok(u32::try_from(values.len()).expect("bounded snapshot"))
            }
            IndexSource::Live { source, access } => (access.length)(self, source),
        }
    }

    /// The value at an index of a branded indexed host object.
    pub fn indexed_host_item(&mut self, object: ObjectId, index: u32) -> JsResult<Option<Value>> {
        match self.indexed_host(object).source {
            IndexSource::Snapshot(ref values) => Ok(values.get(index as usize).copied()),
            IndexSource::Live { source, access } => (access.item)(self, source, index),
        }
    }

    pub(crate) fn host_indexed_value(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<Option<Value>> {
        match key {
            PropertyKey::Index(index) => self.indexed_host_item(object, index),
            _ => Ok(None),
        }
    }

    pub(crate) fn host_index_is_supported(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<bool> {
        match key {
            PropertyKey::Index(index) => Ok(index < self.indexed_host_len(object)?),
            _ => Ok(false),
        }
    }

    fn named_access(&self, object: ObjectId) -> Option<(Value, &'static LiveNameAccess)> {
        match self.indexed_host(object).source {
            IndexSource::Live { source, access } => access.named.map(|named| (source, named)),
            IndexSource::Snapshot(_) => None,
        }
    }

    /// Find a named item independently of own and inherited JS properties.
    pub fn indexed_host_named_item(
        &mut self,
        object: ObjectId,
        name: StringId,
    ) -> JsResult<Option<Value>> {
        match self.named_access(object) {
            Some((source, named)) => (named.item)(self, source, name),
            None => Ok(None),
        }
    }

    pub(crate) fn host_named_exists(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<bool> {
        match (key, self.named_access(object)) {
            (PropertyKey::String(name), Some((source, named))) => {
                (named.contains)(self, source, name)
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn host_named_is_visible(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<bool> {
        let PropertyKey::String(name) = key else {
            return Ok(false);
        };
        self.with_rooted(Value::object(object), |agent| {
            agent.with_rooted(Value::string(name), |agent| {
                agent.check_host_name_visibility(object, key)
            })
        })
    }

    fn check_host_name_visibility(&mut self, object: ObjectId, key: PropertyKey) -> JsResult<bool> {
        if !self.host_named_exists(object, key)? {
            return Ok(false);
        }
        self.supported_name_is_visible(object, key)
    }

    fn supported_name_is_visible(&mut self, object: ObjectId, key: PropertyKey) -> JsResult<bool> {
        if self.ordinary_get_own_property(object, key)?.is_some() {
            return Ok(false);
        }
        let parent = self.stored_prototype(object);
        self.prototype_allows_name(parent, key)
    }

    fn prototype_allows_name(
        &mut self,
        mut parent: Option<ObjectId>,
        key: PropertyKey,
    ) -> JsResult<bool> {
        while let Some(object) = parent {
            self.check_native_work()?;
            if self.is_exotic(object) {
                // Own-property checks can recurse through other host objects,
                // and a Proxy can reenter script or return a cyclic prototype.
                return self.in_proxy(|agent| {
                    agent.with_rooted(Value::object(object), |agent| {
                        if agent.get_own_property(object, key)?.is_some() {
                            return Ok(false);
                        }
                        let next = agent.get_prototype_of(object)?;
                        agent.prototype_allows_name(next, key)
                    })
                });
            }
            if self.ordinary_get_own_property(object, key)?.is_some() {
                return Ok(false);
            }
            parent = self.stored_prototype(object);
        }
        Ok(true)
    }

    pub(crate) fn visible_host_named_item(
        &mut self,
        object: ObjectId,
        key: PropertyKey,
    ) -> JsResult<Option<Value>> {
        let PropertyKey::String(name) = key else {
            return Ok(None);
        };
        self.with_rooted(Value::object(object), |agent| {
            agent.with_rooted(Value::string(name), |agent| {
                if !agent.check_host_name_visibility(object, key)? {
                    return Ok(None);
                }
                // A prototype trap may remove the named element after the
                // visibility check. Re-read the collection before returning.
                Ok(Some(
                    agent
                        .indexed_host_named_item(object, name)?
                        .unwrap_or(Value::NULL),
                ))
            })
        })
    }

    pub(crate) fn host_indexed_keys(&mut self, object: ObjectId) -> JsResult<Vec<PropertyKey>> {
        let count = self.indexed_host_len(object)?;
        let names = match self.named_access(object) {
            Some((source, named)) => (named.keys)(self, source)?,
            None => Vec::new(),
        };
        self.with_rooted_names(object, &names, |agent| {
            let mut keys = Vec::new();
            let capacity = (count as usize)
                .checked_add(names.len())
                .ok_or(OutOfMemory)?;
            keys.try_reserve_exact(capacity).map_err(|_| OutOfMemory)?;
            keys.extend((0..count).map(PropertyKey::Index));
            for &name in &names {
                let key = PropertyKey::String(name);
                if agent.supported_name_is_visible(object, key)? {
                    keys.push(key);
                }
            }
            let own = agent.ordinary_own_property_keys(object);
            // A Proxy trap may have defined a real property during visibility
            // checks. Publish each key once.
            if !names.is_empty() && !own.is_empty() {
                let mut own_keys = std::collections::HashSet::new();
                own_keys.try_reserve(own.len()).map_err(|_| OutOfMemory)?;
                own_keys.extend(own.iter().copied());
                keys.retain(|key| !own_keys.contains(key));
            }
            keys.try_reserve_exact(own.len()).map_err(|_| OutOfMemory)?;
            keys.extend(own);
            Ok(keys)
        })
    }
}
