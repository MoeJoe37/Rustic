//! Realm-owned host interfaces and GC-traced slots for host exceptions.

use super::agent::{Agent, JsResult, NativeFn};
use super::builtins::new_constructor;
use super::heap::OutOfMemory;
use super::object::{Class, Internal, Object, PropertyDescriptor};
use super::shape::PropertyKey;
use super::value::{ObjectId, StringId, Value};

/// Account for the entry and spare capacity in the host prototype table.
pub(crate) const HOST_INTERFACE_BYTES: usize =
    size_of::<((u16, &'static str), ObjectId)>() + size_of::<usize>();

impl Agent {
    /// The current realm's host interface prototype, if installed.
    pub fn host_interface_prototype(&self, interface: &'static str) -> Option<ObjectId> {
        self.heap
            .host_interfaces
            .get(&(self.heap.realm, interface))
            .copied()
    }

    /// Link a constructor to a fresh prototype and install the interface in
    /// the running realm. `initialize` completes the constructor before it
    /// is published. Repeated installation leaves the original interface
    /// and any script changes to the global binding intact.
    pub fn install_host_interface(
        &mut self,
        interface: &'static str,
        behavior: NativeFn,
        length: u32,
        prototype: ObjectId,
        initialize: impl FnOnce(&mut Self, ObjectId) -> JsResult<()>,
    ) -> JsResult<()> {
        if self.host_interface_prototype(interface).is_some() {
            return Ok(());
        }
        let constructor = new_constructor(self, interface, behavior, length, prototype)?;
        initialize(self, constructor)?;
        let key = self.heap.key(interface)?;
        self.heap
            .host_interfaces
            .try_reserve(1)
            .map_err(|_| OutOfMemory)?;
        self.heap.charge(HOST_INTERFACE_BYTES)?;
        let published = self.define_property_or_throw(
            self.intrinsics.global,
            key,
            PropertyDescriptor::data_with(Value::object(constructor), true, false, true),
        );
        if published.is_err() {
            let _ = self.heap.budget.release(HOST_INTERFACE_BYTES);
            return published;
        }
        self.heap
            .host_interfaces
            .insert((self.heap.realm, interface), prototype);
        Ok(())
    }

    /// Obtain a constructed host instance's prototype. A non-object
    /// `target.prototype` falls back to the interface in the target's
    /// realm or, for a realm without that host interface, the active realm.
    pub fn host_prototype_from_new_target(
        &mut self,
        interface: &'static str,
        target: ObjectId,
    ) -> JsResult<ObjectId> {
        let key = PropertyKey::String(self.names.prototype);
        if let Some(prototype) = self.get(target, key)?.as_object() {
            return Ok(prototype);
        }
        let realm = self.function_realm(target)?;
        match self
            .heap
            .host_interfaces
            .get(&(realm, interface))
            .copied()
            .or_else(|| self.host_interface_prototype(interface))
        {
            Some(prototype) => Ok(prototype),
            None => self.type_error("host interface is not installed"),
        }
    }

    /// Allocate an object with `[[ErrorData]]` and the given prototype.
    pub fn new_error_object_with(&mut self, prototype: ObjectId) -> JsResult<ObjectId> {
        Ok(self
            .heap
            .alloc_object(Object::new(Class::Error, Some(prototype)))?)
    }

    /// Create a branded host exception with two private, traced strings.
    pub fn new_host_error(
        &mut self,
        interface: &'static str,
        prototype: ObjectId,
        name: StringId,
        message: StringId,
    ) -> JsResult<Value> {
        let mut object = Object::new(Class::Error, Some(prototype));
        object.extra_mut().internal = Some(Internal::HostError {
            interface,
            name,
            message,
        });
        Ok(Value::object(self.heap.alloc_object(object)?))
    }

    /// Read the private name and message of a host exception of this interface.
    pub fn host_error_data(
        &self,
        object: ObjectId,
        interface: &'static str,
    ) -> Option<(StringId, StringId)> {
        match self
            .heap
            .objects
            .try_get(object.0)
            .and_then(Object::internal)
        {
            Some(Internal::HostError {
                interface: own,
                name,
                message,
            }) if *own == interface => Some((*name, *message)),
            _ => None,
        }
    }
}

/// Agent-local host state that participates in automatic collections.
pub trait ExecutionHost: std::fmt::Debug {
    /// Collect the host and agent with the VM suspended at a safe point.
    /// The host calls [`Agent::collect_garbage_with`] with its tracer.
    /// A failure ends the run as an uncatchable abrupt completion.
    fn collect(&self, agent: &mut Agent) -> Result<(), OutOfMemory>;

    /// The concrete host state, for native binding operations.
    fn as_any(&self) -> &dyn std::any::Any;
}

impl Agent {
    /// Attach the agent's host before exposing host objects to script.
    pub fn attach_execution_host(&mut self, host: std::rc::Rc<dyn ExecutionHost>) -> JsResult<()> {
        if self.execution_host.is_some() {
            return self.type_error("an execution host is already attached");
        }
        self.execution_host = Some(host);
        Ok(())
    }

    /// The attached host, independently of the current realm.
    pub fn execution_host(&self) -> Option<std::rc::Rc<dyn ExecutionHost>> {
        self.execution_host.clone()
    }

    pub(crate) fn collect_at_safe_point(&mut self) -> JsResult<()> {
        if let Some(host) = self.execution_host() {
            host.collect(self)?;
        } else {
            self.collect_garbage(super::Roots::default());
        }
        Ok(())
    }
}
