//! HTML §12.2 Web Storage: the `Storage` interface and
//! `Window.localStorage` / `Window.sessionStorage`.
//!
//! The embedder supplies a [`StorageHost`] for the document's storage key
//! with [`super::attach_storage`]; without one (an opaque origin, or a
//! host without storage) both getters throw a `SecurityError`
//! `DOMException`, as HTML requires for opaque origins. Each getter returns
//! the same `Storage` object every time. Keys and values keep their UTF-16
//! code units.

use std::fmt::Debug;

use rustic_dom::DomString;
use rustic_js::runtime::{
    Abrupt, Agent, JsResult, NamedPropertyAccess, NativeFn, PropertyDescriptor, StringId, Value,
};

use super::events::define_method;
use super::{illegal_constructor, install_interface, prototype, with_page};
use crate::webidl::{
    DomStringConversion, IntegerConversion, from_dom_units, to_dom_string, to_unsigned_long,
};

const INTERFACE: &str = "Storage";

/// Which storage area a `Storage` object reflects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageKind {
    /// `localStorage`: the profile's area for the document's storage key.
    Local,
    /// `sessionStorage`: the top-level browsing session's area.
    Session,
}

impl StorageKind {
    fn index(self) -> usize {
        match self {
            Self::Local => 0,
            Self::Session => 1,
        }
    }

    fn from_source(value: Value) -> Self {
        if value.as_number() == Some(1.0) {
            Self::Session
        } else {
            Self::Local
        }
    }
}

/// A setter refused because the area or profile is full.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaExceeded;

/// The embedder's storage areas for one document. Methods take `&self`;
/// the host uses interior mutability, and must not call back into script.
pub trait StorageHost: Debug {
    /// The number of keys (`length`).
    fn length(&self, kind: StorageKind) -> u32;
    /// The `index`th key in the area's stable order (`key()`).
    fn key(&self, kind: StorageKind, index: u32) -> Option<Vec<u16>>;
    /// The value of `key` (`getItem`).
    fn get(&self, kind: StorageKind, key: &[u16]) -> Option<Vec<u16>>;
    /// Store a value (`setItem`).
    fn set(&self, kind: StorageKind, key: &[u16], value: &[u16]) -> Result<(), QuotaExceeded>;
    /// Remove a key (`removeItem`).
    fn remove(&self, kind: StorageKind, key: &[u16]);
    /// Remove every key (`clear()`).
    fn clear(&self, kind: StorageKind);
    /// Every key, in order.
    fn keys(&self, kind: StorageKind) -> Vec<Vec<u16>>;
}

static ACCESS: NamedPropertyAccess = NamedPropertyAccess {
    contains: named_contains,
    get: named_get,
    set: named_set,
    delete: named_delete,
    names: named_names,
};

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    install_interface(
        agent,
        INTERFACE,
        None,
        illegal_constructor,
        0,
        &[("length", "get length", length)],
        |agent, proto| {
            for (name, method, arguments) in [
                ("key", key as NativeFn, 1),
                ("getItem", get_item, 1),
                ("setItem", set_item, 2),
                ("removeItem", remove_item, 1),
                ("clear", clear, 0),
            ] {
                define_method(agent, proto, name, method, arguments)?;
            }
            Ok(())
        },
    )?;
    let global = agent.intrinsics.global;
    for (name, getter) in [
        ("localStorage", local_storage as NativeFn),
        ("sessionStorage", session_storage),
    ] {
        let get = Value::object(agent.new_native_function(getter, &format!("get {name}"), 0)?);
        let key = agent.heap.key(name)?;
        agent.define_property_or_throw(
            global,
            key,
            PropertyDescriptor::accessor(get, Value::UNDEFINED, true, true),
        )?;
    }
    Ok(())
}

fn local_storage(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    storage_object(agent, StorageKind::Local)
}

fn session_storage(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    storage_object(agent, StorageKind::Session)
}

fn security_error(agent: &mut Agent) -> Abrupt {
    match crate::dom_exception::new(
        agent,
        "SecurityError",
        "this document's origin has no Web Storage",
    ) {
        Ok(error) => Abrupt::Throw(error),
        Err(error) => error,
    }
}

fn storage_object(agent: &mut Agent, kind: StorageKind) -> JsResult<Value> {
    with_page(agent, |agent, page| {
        if page.storage.borrow().is_none() {
            return Err(security_error(agent));
        }
        if let Some((object, _)) = page.storage_objects.borrow()[kind.index()] {
            return Ok(object);
        }
        let proto = prototype(agent, INTERFACE)?;
        let source = Value::number(kind.index() as f64);
        let object = Value::object(agent.new_named_host(INTERFACE, proto, source, &ACCESS)?);
        // The page keeps its Storage objects for its lifetime.
        let root = agent.root(object)?;
        page.storage_objects.borrow_mut()[kind.index()] = Some((object, root));
        Ok(object)
    })
}

/// Run `operation` with the host of the `Storage` receiver.
fn with_host<T>(
    agent: &mut Agent,
    receiver: Value,
    operation: impl FnOnce(&mut Agent, &dyn StorageHost, StorageKind) -> JsResult<T>,
) -> JsResult<T> {
    let Some(source) = receiver
        .as_object()
        .and_then(|object| agent.named_host_source(object, INTERFACE))
    else {
        return agent.type_error("expected a Storage receiver");
    };
    with_source(agent, source, operation)
}

fn with_source<T>(
    agent: &mut Agent,
    source: Value,
    operation: impl FnOnce(&mut Agent, &dyn StorageHost, StorageKind) -> JsResult<T>,
) -> JsResult<T> {
    let kind = StorageKind::from_source(source);
    with_page(agent, |agent, page| {
        let Some(host) = page.storage.borrow().clone() else {
            return Err(security_error(agent));
        };
        operation(agent, &*host, kind)
    })
}

fn string_units(agent: &mut Agent, value: Value) -> JsResult<DomString> {
    to_dom_string(agent, value, DomStringConversion::Default)
}

fn name_units(agent: &Agent, name: StringId) -> Vec<u16> {
    agent.heap.string(name).units().collect()
}

fn quota_error(agent: &mut Agent) -> Abrupt {
    match crate::dom_exception::new(
        agent,
        "QuotaExceededError",
        "the storage quota has been exceeded",
    ) {
        Ok(error) => Abrupt::Throw(error),
        Err(error) => error,
    }
}

fn length(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    with_host(agent, receiver, |_, host, kind| {
        Ok(Value::number(f64::from(host.length(kind))))
    })
}

fn key(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let index = super::required_arg(agent, args, 0)?;
    let index = to_unsigned_long(agent, index, IntegerConversion::Wrap)?;
    with_host(agent, receiver, |agent, host, kind| {
        match host.key(kind, index) {
            Some(units) => from_dom_units(agent, &units),
            None => Ok(Value::NULL),
        }
    })
}

fn get_item(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let name = super::required_arg(agent, args, 0)?;
    let name = string_units(agent, name)?;
    with_host(agent, receiver, |agent, host, kind| {
        match host.get(kind, name.as_units()) {
            Some(units) => from_dom_units(agent, &units),
            None => Ok(Value::NULL),
        }
    })
}

fn set_item(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let name = super::required_arg(agent, args, 0)?;
    let value = super::required_arg(agent, args, 1)?;
    let name = string_units(agent, name)?;
    let value = string_units(agent, value)?;
    with_host(agent, receiver, |agent, host, kind| {
        host.set(kind, name.as_units(), value.as_units())
            .map_err(|_| quota_error(agent))?;
        Ok(Value::UNDEFINED)
    })
}

fn remove_item(agent: &mut Agent, receiver: Value, args: &[Value]) -> JsResult<Value> {
    let name = super::required_arg(agent, args, 0)?;
    let name = string_units(agent, name)?;
    with_host(agent, receiver, |_, host, kind| {
        host.remove(kind, name.as_units());
        Ok(Value::UNDEFINED)
    })
}

fn clear(agent: &mut Agent, receiver: Value, _: &[Value]) -> JsResult<Value> {
    with_host(agent, receiver, |_, host, kind| {
        host.clear(kind);
        Ok(Value::UNDEFINED)
    })
}

fn named_contains(agent: &mut Agent, source: Value, name: StringId) -> JsResult<bool> {
    let units = name_units(agent, name);
    with_source(agent, source, |_, host, kind| {
        Ok(host.get(kind, &units).is_some())
    })
}

fn named_get(agent: &mut Agent, source: Value, name: StringId) -> JsResult<Option<Value>> {
    let units = name_units(agent, name);
    with_source(agent, source, |agent, host, kind| {
        host.get(kind, &units)
            .map(|value| from_dom_units(agent, &value))
            .transpose()
    })
}

fn named_set(agent: &mut Agent, source: Value, name: StringId, value: Value) -> JsResult<()> {
    // The name's units are copied before the value's conversion can run
    // script and collect.
    let units = name_units(agent, name);
    let value = string_units(agent, value)?;
    with_source(agent, source, |agent, host, kind| {
        host.set(kind, &units, value.as_units())
            .map_err(|_| quota_error(agent))
    })
}

fn named_delete(agent: &mut Agent, source: Value, name: StringId) -> JsResult<()> {
    let units = name_units(agent, name);
    with_source(agent, source, |_, host, kind| {
        host.remove(kind, &units);
        Ok(())
    })
}

fn named_names(agent: &mut Agent, source: Value) -> JsResult<Vec<StringId>> {
    let keys = with_source(agent, source, |_, host, kind| Ok(host.keys(kind)))?;
    let mut names = Vec::new();
    names
        .try_reserve_exact(keys.len())
        .map_err(|_| rustic_js::runtime::OutOfMemory)?;
    let mut roots = Vec::new();
    roots
        .try_reserve_exact(keys.len())
        .map_err(|_| rustic_js::runtime::OutOfMemory)?;
    let mut result = Ok(());
    for key in keys {
        // Keep earlier names alive while later ones allocate.
        let rooted = from_dom_units(agent, &key).and_then(|value| Ok((value, agent.root(value)?)));
        match rooted {
            Ok((value, root)) => {
                names.push(value.as_string().expect("a string value"));
                roots.push(root);
            }
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    for root in roots {
        agent.unroot(root);
    }
    result.map(|()| names)
}
