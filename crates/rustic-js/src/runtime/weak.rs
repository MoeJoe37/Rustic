//! Host objects and weak objects (JS-03, ADR 0016): the internal slots of
//! `WeakRef`, `WeakMap`, `WeakSet` and `FinalizationRegistry` (§24.3, §24.4,
//! §26.1, §26.2) and the operations their built-ins call. The collector in
//! `gc.rs` gives them their meaning: a weak target does not keep its value
//! alive, a weak-map entry keeps its value alive only while its key is, and
//! a registry hands back the held value once the target is gone. The
//! constructors and prototype methods themselves arrive with JS-05.

use std::collections::HashMap;

use super::agent::{Agent, JsResult};
use super::gc::WEAK_ENTRY_BYTES;
use super::object::{Class, Internal, Object, RegistryCell};
use super::value::{Kind, ObjectId, Value};

impl Agent {
    fn new_internal(
        &mut self,
        class: Class,
        proto: Option<ObjectId>,
        internal: Internal,
    ) -> JsResult<ObjectId> {
        let mut object = Object::new(class, proto);
        object.extra_mut().internal = Some(internal);
        Ok(self.heap.alloc_object(object)?)
    }

    fn internal_of(&self, object: ObjectId, class: Class) -> Option<&Internal> {
        let object = self.heap.object(object);
        (object.class == class).then(|| object.internal()).flatten()
    }

    /// A host object: an ordinary object that also names embedder state by
    /// `data`. The collector asks the embedder's
    /// [`HostTracer`](super::HostTracer) for its edges and reports its
    /// death.
    pub fn new_host_object(&mut self, proto: Option<ObjectId>, data: u64) -> JsResult<ObjectId> {
        self.new_internal(Class::Host, proto, Internal::Host(data))
    }

    /// The embedder data of a live host object, or `None` if absent.
    pub fn host_data(&self, object: ObjectId) -> Option<u64> {
        match self
            .heap
            .objects
            .try_get(object.0)
            .and_then(Object::internal)
        {
            Some(Internal::Host(data)) => Some(*data),
            _ => None,
        }
    }

    /// `CanBeHeldWeakly` (§9.14): objects and symbols. Symbols from the
    /// global registry will be excluded when `Symbol.for` exists (JS-05).
    pub fn can_be_held_weakly(&self, value: Value) -> bool {
        matches!(value.kind(), Kind::Object(_) | Kind::Symbol(_))
    }

    // ---- WeakRef ----------------------------------------------------

    /// A WeakRef to `target`.
    pub fn new_weak_ref(&mut self, target: Value) -> JsResult<ObjectId> {
        if !self.can_be_held_weakly(target) {
            return self.type_error("WeakRef target must be an object or symbol");
        }
        let proto = self.intrinsics.object_prototype;
        self.new_internal(Class::WeakRef, Some(proto), Internal::WeakRef(target))
    }

    /// `WeakRefDeref`: the target, or `undefined` once it was collected.
    /// Keeping the target alive until the end of the current job
    /// (`AddToKeptObjects`) is the caller's duty: root what this returns.
    pub fn weak_ref_deref(&self, weak_ref: ObjectId) -> Value {
        match self.internal_of(weak_ref, Class::WeakRef) {
            Some(Internal::WeakRef(target)) if *target != Value::UNINITIALIZED => *target,
            _ => Value::UNDEFINED,
        }
    }

    // ---- WeakMap and WeakSet ----------------------------------------

    /// An empty WeakMap.
    pub fn new_weak_map(&mut self) -> JsResult<ObjectId> {
        let proto = self.intrinsics.object_prototype;
        self.new_internal(
            Class::WeakMap,
            Some(proto),
            Internal::WeakMap(HashMap::new()),
        )
    }

    /// An empty WeakSet: a weak map whose values are unused.
    pub fn new_weak_set(&mut self) -> JsResult<ObjectId> {
        let proto = self.intrinsics.object_prototype;
        self.new_internal(
            Class::WeakSet,
            Some(proto),
            Internal::WeakMap(HashMap::new()),
        )
    }

    fn weak_entries(&self, collection: ObjectId) -> Option<&HashMap<u64, Value>> {
        let object = self.heap.object(collection);
        match (object.class, object.internal()) {
            (Class::WeakMap | Class::WeakSet, Some(Internal::WeakMap(entries))) => Some(entries),
            _ => None,
        }
    }

    /// `WeakMap.prototype.set` / `WeakSet.prototype.add` (with `value`
    /// undefined): throws a TypeError for a key that cannot be held weakly
    /// or a receiver that is not a weak collection.
    pub fn weak_map_set(&mut self, collection: ObjectId, key: Value, value: Value) -> JsResult<()> {
        if self.weak_entries(collection).is_none() {
            return self.type_error("not a WeakMap or WeakSet");
        }
        if !self.can_be_held_weakly(key) {
            return self.type_error("invalid value used as weak collection key");
        }
        let fresh = !self
            .weak_entries(collection)
            .is_some_and(|e| e.contains_key(&key.bits()));
        if fresh {
            self.heap.charge(WEAK_ENTRY_BYTES)?;
        }
        if let Some(Internal::WeakMap(entries)) = self.heap.object_mut(collection).internal_mut() {
            entries.insert(key.bits(), value);
        }
        Ok(())
    }

    /// `WeakMap.prototype.get`: `undefined` when absent.
    pub fn weak_map_get(&self, collection: ObjectId, key: Value) -> Value {
        self.weak_entries(collection)
            .and_then(|e| e.get(&key.bits()).copied())
            .unwrap_or(Value::UNDEFINED)
    }

    /// `WeakMap.prototype.has` / `WeakSet.prototype.has`.
    pub fn weak_map_has(&self, collection: ObjectId, key: Value) -> bool {
        self.weak_entries(collection)
            .is_some_and(|e| e.contains_key(&key.bits()))
    }

    /// `WeakMap.prototype.delete` / `WeakSet.prototype.delete`.
    pub fn weak_map_delete(&mut self, collection: ObjectId, key: Value) -> bool {
        let object = self.heap.object_mut(collection);
        match (object.class, object.internal_mut()) {
            (Class::WeakMap | Class::WeakSet, Some(Internal::WeakMap(entries))) => {
                entries.remove(&key.bits()).is_some()
            }
            _ => false,
        }
    }

    /// Entries currently stored. Not observable from script; for tests and
    /// memory reports.
    pub fn weak_map_len(&self, collection: ObjectId) -> usize {
        self.weak_entries(collection).map_or(0, HashMap::len)
    }

    // ---- FinalizationRegistry ---------------------------------------

    /// An empty FinalizationRegistry. Its cleanup callback is the
    /// built-in's concern (JS-05); the heap only queues held values.
    pub fn new_finalization_registry(&mut self) -> JsResult<ObjectId> {
        let proto = self.intrinsics.object_prototype;
        self.new_internal(
            Class::FinalizationRegistry,
            Some(proto),
            Internal::Registry {
                cells: Vec::new(),
                pending: Vec::new(),
            },
        )
    }

    /// `FinalizationRegistry.prototype.register`.
    pub fn registry_register(
        &mut self,
        registry: ObjectId,
        target: Value,
        held: Value,
        token: Option<Value>,
    ) -> JsResult<()> {
        if self
            .internal_of(registry, Class::FinalizationRegistry)
            .is_none()
        {
            return self.type_error("not a FinalizationRegistry");
        }
        if !self.can_be_held_weakly(target) {
            return self.type_error("register target must be an object or symbol");
        }
        if self.same_value(target, held) {
            return self.type_error("register target and held value must differ");
        }
        let token = match token {
            None => Value::UNINITIALIZED,
            Some(token) if token == Value::UNDEFINED => Value::UNINITIALIZED,
            Some(token) if self.can_be_held_weakly(token) => token,
            Some(_) => return self.type_error("invalid unregister token"),
        };
        self.heap.charge(WEAK_ENTRY_BYTES)?;
        if let Some(Internal::Registry { cells, .. }) =
            self.heap.object_mut(registry).internal_mut()
        {
            cells.push(RegistryCell {
                target,
                held,
                token,
            });
        }
        Ok(())
    }

    /// `FinalizationRegistry.prototype.unregister`: whether any cell was
    /// removed.
    pub fn registry_unregister(&mut self, registry: ObjectId, token: Value) -> JsResult<bool> {
        if self
            .internal_of(registry, Class::FinalizationRegistry)
            .is_none()
        {
            return self.type_error("not a FinalizationRegistry");
        }
        if !self.can_be_held_weakly(token) {
            return self.type_error("invalid unregister token");
        }
        let mut removed = false;
        if let Some(Internal::Registry { cells, .. }) =
            self.heap.object_mut(registry).internal_mut()
        {
            cells.retain(|cell| {
                let hit = cell.token == token;
                removed |= hit;
                !hit
            });
        }
        Ok(removed)
    }

    /// Held values of targets collected since the last call, in
    /// registration order per collection. The host calls the cleanup
    /// callback with each (`CleanupFinalizationRegistry`), as a job.
    pub fn registry_take_finalized(&mut self, registry: ObjectId) -> Vec<Value> {
        let object = self.heap.object_mut(registry);
        match (object.class, object.internal_mut()) {
            (Class::FinalizationRegistry, Some(Internal::Registry { pending, .. })) => {
                std::mem::take(pending)
            }
            _ => Vec::new(),
        }
    }
}
