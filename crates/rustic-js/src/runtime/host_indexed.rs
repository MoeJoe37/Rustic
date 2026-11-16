//! Read-only indexed legacy platform objects (Web IDL §3.9).

use super::agent::{Agent, JsResult};
use super::heap::OutOfMemory;
use super::object::{Class, FLAG_EXOTIC, Internal, Object};
use super::shape::PropertyKey;
use super::value::{ObjectId, Value};

/// Native accessors for a live indexed host object. Indices are contiguous
/// from zero to the length. Accessors must finish without running script.
#[derive(Debug)]
pub struct LiveIndexAccess {
    /// Read the current number of supported indices from the rooted source.
    pub length: fn(&mut Agent, Value) -> JsResult<u32>,
    /// Read the current value at an index, or `None` beyond the end.
    pub item: fn(&mut Agent, Value, u32) -> JsResult<Option<Value>>,
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

    pub(crate) fn host_indexed_keys(&mut self, object: ObjectId) -> JsResult<Vec<PropertyKey>> {
        let count = self.indexed_host_len(object)?;
        let own = self.ordinary_own_property_keys(object);
        let mut keys = Vec::new();
        let capacity = (count as usize).checked_add(own.len()).ok_or(OutOfMemory)?;
        keys.try_reserve_exact(capacity).map_err(|_| OutOfMemory)?;
        keys.extend((0..count).map(PropertyKey::Index));
        keys.extend(own);
        Ok(keys)
    }
}
