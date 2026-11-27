//! Keyed collections and weak references: `Map`, `Set` (§24.1, §24.2),
//! `WeakMap`, `WeakSet` (§24.3, §24.4), `WeakRef` and
//! `FinalizationRegistry` (§26.1, §26.2), with `Map.groupBy` and
//! `Object.groupBy` (§24.1.2.1, §20.1.2.12).
//!
//! Map and Set keep their entries in an [`OrderedTable`]; the weak
//! collections use the ephemeron tables of JS-03. A FinalizationRegistry
//! records registrations and collects held values, but its cleanup
//! callback waits for host jobs (JS-06).

use super::{
    Method, arg, install_constructor, install_methods, iterate, method, prototype_from_new_target,
};
use crate::runtime::agent::{Agent, JsResult};
use crate::runtime::object::{Class, Internal, IteratorKind, Object, PropertyDescriptor};
use crate::runtime::shape::PropertyKey;
use crate::runtime::table::{OrderedTable, TABLE_ENTRY_BYTES};
use crate::runtime::value::{ObjectId, Value};

const MAP_METHODS: &[Method] = &[
    method("clear", map_clear, 0),
    method("delete", map_delete, 1),
    method("entries", map_entries, 0),
    method("forEach", map_for_each, 1),
    method("get", map_get, 1),
    method("has", map_has, 1),
    method("keys", map_keys, 0),
    method("getOrInsert", map_get_or_insert, 2),
    method("getOrInsertComputed", map_get_or_insert_computed, 2),
    method("set", map_set, 2),
    method("values", map_values, 0),
];

const SET_METHODS: &[Method] = &[
    method("add", set_add, 1),
    method("clear", set_clear, 0),
    method("delete", set_delete, 1),
    method("difference", set_difference, 1),
    method("entries", set_entries, 0),
    method("forEach", set_for_each, 1),
    method("has", set_has, 1),
    method("intersection", set_intersection, 1),
    method("isDisjointFrom", set_is_disjoint_from, 1),
    method("isSubsetOf", set_is_subset_of, 1),
    method("isSupersetOf", set_is_superset_of, 1),
    method("symmetricDifference", set_symmetric_difference, 1),
    method("union", set_union, 1),
    method("values", set_values, 0),
];

const WEAK_MAP_METHODS: &[Method] = &[
    method("delete", weak_map_delete, 1),
    method("get", weak_map_get, 1),
    method("getOrInsert", weak_map_get_or_insert, 2),
    method("getOrInsertComputed", weak_map_get_or_insert_computed, 2),
    method("has", weak_map_has, 1),
    method("set", weak_map_set, 2),
];

const WEAK_SET_METHODS: &[Method] = &[
    method("add", weak_set_add, 1),
    method("delete", weak_map_delete, 1),
    method("has", weak_map_has, 1),
];

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let object_prototype = agent.intrinsics.object_prototype;
    let iterator_prototype = agent
        .get_prototype_of(agent.intrinsics.array_iterator_prototype)?
        .expect("%ArrayIteratorPrototype% inherits from %IteratorPrototype%");

    let map_prototype = agent.new_object_with(Some(object_prototype))?;
    let map = install_constructor(agent, "Map", map_constructor, 0, map_prototype)?;
    install_methods(agent, map_prototype, MAP_METHODS)?;
    install_methods(agent, map, &[method("groupBy", map_group_by, 2)])?;
    size_getter(agent, map_prototype, map_size)?;
    alias(
        agent,
        map_prototype,
        "entries",
        PropertyKey::Symbol(agent.symbols.iterator),
    )?;
    to_string_tag(agent, map_prototype, "Map")?;
    species_getter(agent, map)?;

    let set_prototype = agent.new_object_with(Some(object_prototype))?;
    let set = install_constructor(agent, "Set", set_constructor, 0, set_prototype)?;
    install_methods(agent, set_prototype, SET_METHODS)?;
    size_getter(agent, set_prototype, set_size)?;
    let keys = agent.heap.key("keys")?;
    alias(agent, set_prototype, "values", keys)?;
    alias(
        agent,
        set_prototype,
        "values",
        PropertyKey::Symbol(agent.symbols.iterator),
    )?;
    to_string_tag(agent, set_prototype, "Set")?;
    species_getter(agent, set)?;

    for (slot, tag) in ["Map Iterator", "Set Iterator"].into_iter().enumerate() {
        let prototype = agent.new_object_with(Some(iterator_prototype))?;
        install_methods(agent, prototype, &[method("next", table_iterator_next, 0)])?;
        to_string_tag(agent, prototype, tag)?;
        agent.intrinsics.table_iterator_prototypes[slot] = prototype;
    }

    let weak_map_prototype = agent.new_object_with(Some(object_prototype))?;
    install_constructor(
        agent,
        "WeakMap",
        weak_map_constructor,
        0,
        weak_map_prototype,
    )?;
    install_methods(agent, weak_map_prototype, WEAK_MAP_METHODS)?;
    to_string_tag(agent, weak_map_prototype, "WeakMap")?;

    let weak_set_prototype = agent.new_object_with(Some(object_prototype))?;
    install_constructor(
        agent,
        "WeakSet",
        weak_set_constructor,
        0,
        weak_set_prototype,
    )?;
    install_methods(agent, weak_set_prototype, WEAK_SET_METHODS)?;
    to_string_tag(agent, weak_set_prototype, "WeakSet")?;

    let weak_ref_prototype = agent.new_object_with(Some(object_prototype))?;
    install_constructor(
        agent,
        "WeakRef",
        weak_ref_constructor,
        1,
        weak_ref_prototype,
    )?;
    install_methods(
        agent,
        weak_ref_prototype,
        &[method("deref", weak_ref_deref, 0)],
    )?;
    to_string_tag(agent, weak_ref_prototype, "WeakRef")?;

    let registry_prototype = agent.new_object_with(Some(object_prototype))?;
    install_constructor(
        agent,
        "FinalizationRegistry",
        registry_constructor,
        1,
        registry_prototype,
    )?;
    install_methods(
        agent,
        registry_prototype,
        &[
            method("register", registry_register, 2),
            method("unregister", registry_unregister, 1),
        ],
    )?;
    to_string_tag(agent, registry_prototype, "FinalizationRegistry")?;

    let global = agent.intrinsics.global;
    let object_key = agent.heap.key("Object")?;
    if let Some(object_constructor) = agent.get(global, object_key)?.as_object() {
        install_methods(
            agent,
            object_constructor,
            &[method("groupBy", object_group_by, 2)],
        )?;
    }
    Ok(())
}

fn to_string_tag(agent: &mut Agent, o: ObjectId, tag: &str) -> JsResult<()> {
    let tag = agent.string(tag)?;
    agent.define_property_or_throw(
        o,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(tag, false, false, true),
    )
}

/// `o[to] = o[from]`, the same function object under another key.
fn alias(agent: &mut Agent, o: ObjectId, from: &str, to: PropertyKey) -> JsResult<()> {
    let key = agent.heap.key(from)?;
    let f = agent.get(o, key)?;
    agent.define_property_or_throw(o, to, PropertyDescriptor::data_with(f, true, false, true))
}

fn size_getter(
    agent: &mut Agent,
    prototype: ObjectId,
    behavior: crate::runtime::NativeFn,
) -> JsResult<()> {
    let getter = agent.new_native_function(behavior, "get size", 0)?;
    let key = agent.heap.key("size")?;
    agent.define_property_or_throw(
        prototype,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )
}

fn species_getter(agent: &mut Agent, constructor: ObjectId) -> JsResult<()> {
    let getter = agent.new_native_function(return_this, "get [Symbol.species]", 0)?;
    agent.define_property_or_throw(
        constructor,
        PropertyKey::Symbol(agent.symbols.species),
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, false, true),
    )
}

fn return_this(_: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    Ok(this)
}

// ---- tables -----------------------------------------------------------------

/// The table of a `class` receiver, taken out of the object so that its
/// operations can read the heap; [`put_table`] puts it back.
fn take_table(
    agent: &mut Agent,
    this: Value,
    class: Class,
    name: &str,
) -> JsResult<(ObjectId, Box<OrderedTable>)> {
    if let Some(o) = this.as_object() {
        let object = agent.heap.object_mut(o);
        if object.class == class {
            if let Some(Internal::Table(table)) = object.internal_mut() {
                return Ok((o, std::mem::take(table)));
            }
        }
    }
    agent.type_error(&format!("{name} called on an incompatible receiver"))
}

fn put_table(agent: &mut Agent, o: ObjectId, table: Box<OrderedTable>) {
    if let Some(Internal::Table(slot)) = agent.heap.object_mut(o).internal_mut() {
        *slot = table;
    }
}

/// Reads the table of a `class` receiver without taking it.
fn with_table<T>(
    agent: &mut Agent,
    this: Value,
    class: Class,
    name: &str,
    read: impl FnOnce(&Agent, &OrderedTable) -> T,
) -> JsResult<T> {
    let (o, table) = take_table(agent, this, class, name)?;
    let result = read(agent, &table);
    put_table(agent, o, table);
    Ok(result)
}

/// Changes the table of a `class` receiver; the closure says how many
/// entries it added, for the budget.
fn change_table<T>(
    agent: &mut Agent,
    this: Value,
    class: Class,
    name: &str,
    change: impl FnOnce(&Agent, &mut OrderedTable) -> (T, bool),
) -> JsResult<T> {
    let (o, mut table) = take_table(agent, this, class, name)?;
    let (result, added) = change(agent, &mut table);
    put_table(agent, o, table);
    if added {
        agent.heap.charge(TABLE_ENTRY_BYTES)?;
    }
    Ok(result)
}

fn new_table_object(agent: &mut Agent, class: Class, default: ObjectId) -> JsResult<ObjectId> {
    let prototype = prototype_from_new_target(agent, default)?;
    let mut object = Object::new(class, Some(prototype));
    object.extra_mut().internal = Some(Internal::Table(Box::default()));
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::object::Extra>())?;
    Ok(agent.heap.alloc_object(object)?)
}

fn require_new(agent: &mut Agent, name: &str) -> JsResult<()> {
    if agent.new_target().is_none() {
        return agent.type_error(&format!("Constructor {name} requires 'new'"));
    }
    Ok(())
}

/// The prototype the constructor named `name` gives its instances: its
/// own `prototype`, found through the active function.
fn own_prototype(agent: &mut Agent) -> JsResult<ObjectId> {
    let f = agent.active_function().expect("a constructor is running");
    let key = PropertyKey::String(agent.names.prototype);
    match agent.get(f, key)?.as_object() {
        Some(p) => Ok(p),
        None => Ok(agent.intrinsics.object_prototype),
    }
}

/// Fills a new collection from `iterable` by calling its `adder` (`set`
/// or `add`) with each entry, as the constructors do.
fn fill_from_iterable(
    agent: &mut Agent,
    target: ObjectId,
    iterable: Value,
    adder_name: &str,
    pairs: bool,
) -> JsResult<()> {
    if iterable.is_nullish() {
        return Ok(());
    }
    let key = agent.heap.key(adder_name)?;
    let adder = agent.get(target, key)?;
    if !agent.is_callable(adder) {
        return agent.type_error(&format!("'{adder_name}' is not a function"));
    }
    let iterator_key = PropertyKey::Symbol(agent.symbols.iterator);
    let Some(method) = agent.get_method(iterable, iterator_key)? else {
        return agent.type_error("value is not iterable");
    };
    iterate(agent, iterable, method, |agent, item| {
        if pairs {
            let Some(entry) = item.as_object() else {
                return agent.type_error("iterator value is not an entry object");
            };
            let k = agent.get(entry, PropertyKey::Index(0))?;
            let v = agent.get(entry, PropertyKey::Index(1))?;
            agent.call(adder, Value::object(target), &[k, v])?;
        } else {
            agent.call(adder, Value::object(target), &[item])?;
        }
        Ok(true)
    })
}

// ---- Map ----------------------------------------------------------------------

fn map_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    require_new(agent, "Map")?;
    let default = own_prototype(agent)?;
    let map = new_table_object(agent, Class::Map, default)?;
    fill_from_iterable(agent, map, arg(args, 0), "set", true)?;
    Ok(Value::object(map))
}

fn map_get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = arg(args, 0);
    with_table(agent, this, Class::Map, "Map.prototype.get", |agent, t| {
        t.get(agent, key).unwrap_or(Value::UNDEFINED)
    })
}

fn map_has(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = arg(args, 0);
    with_table(agent, this, Class::Map, "Map.prototype.has", |agent, t| {
        Value::boolean(t.has(agent, key))
    })
}

fn map_set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (key, value) = (arg(args, 0), arg(args, 1));
    change_table(agent, this, Class::Map, "Map.prototype.set", |agent, t| {
        ((), t.set(agent, key, value))
    })?;
    Ok(this)
}

/// `Map.prototype.getOrInsert(key, value)` (upsert proposal, stage 3).
fn map_get_or_insert(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (key, value) = (arg(args, 0), arg(args, 1));
    change_table(
        agent,
        this,
        Class::Map,
        "Map.prototype.getOrInsert",
        |agent, t| match t.get(agent, key) {
            Some(existing) => (existing, false),
            None => (value, t.set(agent, key, value)),
        },
    )
}

/// `Map.prototype.getOrInsertComputed(key, callback)`: the callback runs
/// only for a missing key, and its result is stored even if the
/// callback added the key meanwhile.
fn map_get_or_insert_computed(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (key, callback) = (arg(args, 0), arg(args, 1));
    let name = "Map.prototype.getOrInsertComputed";
    with_table(agent, this, Class::Map, name, |_, _| ())?;
    if !agent.is_callable(callback) {
        return agent.type_error(&format!("{name}: callback is not a function"));
    }
    let key = if key.as_number() == Some(0.0) {
        Value::number(0.0)
    } else {
        key
    };
    if let Some(existing) = with_table(agent, this, Class::Map, name, |agent, t| t.get(agent, key))?
    {
        return Ok(existing);
    }
    let value = agent.call(callback, Value::UNDEFINED, &[key])?;
    change_table(agent, this, Class::Map, name, |agent, t| {
        ((), t.set(agent, key, value))
    })?;
    Ok(value)
}

fn map_delete(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = arg(args, 0);
    change_table(
        agent,
        this,
        Class::Map,
        "Map.prototype.delete",
        |agent, t| (Value::boolean(t.delete(agent, key)), false),
    )
}

fn map_clear(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    change_table(agent, this, Class::Map, "Map.prototype.clear", |_, t| {
        t.clear();
        ((), false)
    })?;
    Ok(Value::UNDEFINED)
}

fn map_size(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    with_table(agent, this, Class::Map, "get Map.prototype.size", |_, t| {
        Value::number(t.len() as f64)
    })
}

/// `forEach` over a table, holding its positions while the callback runs.
fn table_for_each(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
    class: Class,
    name: &str,
) -> JsResult<Value> {
    // Collect between callbacks when called from a safe site (TD-94).
    let gc = agent.loop_collect();
    // Brand check first.
    with_table(agent, this, class, name, |_, _| ())?;
    let o = this.as_object().expect("checked above");
    let f = arg(args, 0);
    if !agent.is_callable(f) {
        return agent.type_error(&format!("{name}: callback is not a function"));
    }
    let this_arg = arg(args, 1);
    adjust_loops(agent, o, 1);
    let mut position = 0;
    let result = loop {
        let step = with_table(agent, this, class, name, |_, t| {
            (position..t.extent()).find_map(|p| t.at(p).map(|(k, v)| (p, k, v)))
        });
        let (p, k, v) = match step {
            Ok(Some(found)) => found,
            Ok(None) => break Ok(Value::UNDEFINED),
            Err(e) => break Err(e),
        };
        position = p + 1;
        if let Err(e) = agent.call(f, this_arg, &[v, k, this]) {
            break Err(e);
        }
        if let Err(e) = agent.loop_safepoint(gc, &[this, f, this_arg]) {
            break Err(e);
        }
    };
    adjust_loops(agent, o, -1);
    result
}

/// Count an iterator object's hold on its table (the collector recounts
/// these from live iterators).
fn adjust_iterators(agent: &mut Agent, o: ObjectId, by: i32) {
    if let Some(Internal::Table(t)) = agent.heap.object_mut(o).internal_mut() {
        t.iterators = t.iterators.saturating_add_signed(by);
    }
}

/// Count a native loop's hold on its table (the collector leaves these).
fn adjust_loops(agent: &mut Agent, o: ObjectId, by: i32) {
    if let Some(Internal::Table(t)) = agent.heap.object_mut(o).internal_mut() {
        t.loops = t.loops.saturating_add_signed(by);
    }
}

fn map_for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    table_for_each(agent, this, args, Class::Map, "Map.prototype.forEach")
}

fn new_table_iterator(
    agent: &mut Agent,
    this: Value,
    class: Class,
    kind: IteratorKind,
    name: &str,
) -> JsResult<Value> {
    with_table(agent, this, class, name, |_, _| ())?;
    let o = this.as_object().expect("checked above");
    let slot = usize::from(class == Class::Set);
    let prototype = agent.intrinsics.table_iterator_prototypes[slot];
    let iterator = agent.new_object_with(Some(prototype))?;
    agent
        .heap
        .charge(std::mem::size_of::<crate::runtime::object::Extra>())?;
    agent.heap.object_mut(iterator).extra_mut().internal = Some(Internal::Iterator {
        target: this,
        index: 0,
        kind,
    });
    adjust_iterators(agent, o, 1);
    Ok(Value::object(iterator))
}

fn map_keys(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    new_table_iterator(
        agent,
        this,
        Class::Map,
        IteratorKind::TableKeys,
        "Map.prototype.keys",
    )
}

fn map_values(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    new_table_iterator(
        agent,
        this,
        Class::Map,
        IteratorKind::TableValues,
        "Map.prototype.values",
    )
}

fn map_entries(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    new_table_iterator(
        agent,
        this,
        Class::Map,
        IteratorKind::TableEntries,
        "Map.prototype.entries",
    )
}

/// `%MapIteratorPrototype%.next` and `%SetIteratorPrototype%.next`.
fn table_iterator_next(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let state = this
        .as_object()
        .and_then(|o| match agent.heap.object(o).internal() {
            Some(Internal::Iterator {
                target,
                index,
                kind,
            }) if kind.walks_table() => Some((o, *target, *index, *kind)),
            _ => None,
        });
    let Some((iterator, target, index, kind)) = state else {
        return agent.type_error("next called on an incompatible receiver");
    };
    let Some(table_object) = target.as_object() else {
        return super::iterator_result(agent, Value::UNDEFINED, true);
    };
    let class = agent.class(table_object);
    let found = with_table(agent, target, class, "next", |_, t| {
        (index as usize..t.extent()).find_map(|p| t.at(p).map(|(k, v)| (p, k, v)))
    })?;
    match found {
        Some((p, k, v)) => {
            if let Some(Internal::Iterator { index, .. }) =
                agent.heap.object_mut(iterator).internal_mut()
            {
                // Positions stay below 2³² (the heap budget bounds tables).
                *index = p as u32 + 1;
            }
            let value = match kind {
                IteratorKind::TableKeys => k,
                IteratorKind::TableValues => v,
                _ => Value::object(agent.create_array_from_list(&[k, v])?),
            };
            super::iterator_result(agent, value, false)
        }
        None => {
            if let Some(Internal::Iterator { target, .. }) =
                agent.heap.object_mut(iterator).internal_mut()
            {
                *target = Value::UNDEFINED;
            }
            adjust_iterators(agent, table_object, -1);
            super::iterator_result(agent, Value::UNDEFINED, true)
        }
    }
}

// ---- Set ----------------------------------------------------------------------

fn set_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    require_new(agent, "Set")?;
    let default = own_prototype(agent)?;
    let set = new_table_object(agent, Class::Set, default)?;
    fill_from_iterable(agent, set, arg(args, 0), "add", false)?;
    Ok(Value::object(set))
}

fn set_add(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let value = arg(args, 0);
    change_table(agent, this, Class::Set, "Set.prototype.add", |agent, t| {
        ((), t.set(agent, value, value))
    })?;
    Ok(this)
}

fn set_has(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = arg(args, 0);
    with_table(agent, this, Class::Set, "Set.prototype.has", |agent, t| {
        Value::boolean(t.has(agent, key))
    })
}

fn set_delete(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let key = arg(args, 0);
    change_table(
        agent,
        this,
        Class::Set,
        "Set.prototype.delete",
        |agent, t| (Value::boolean(t.delete(agent, key)), false),
    )
}

fn set_clear(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    change_table(agent, this, Class::Set, "Set.prototype.clear", |_, t| {
        t.clear();
        ((), false)
    })?;
    Ok(Value::UNDEFINED)
}

fn set_size(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    with_table(agent, this, Class::Set, "get Set.prototype.size", |_, t| {
        Value::number(t.len() as f64)
    })
}

fn set_for_each(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    table_for_each(agent, this, args, Class::Set, "Set.prototype.forEach")
}

fn set_values(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    new_table_iterator(
        agent,
        this,
        Class::Set,
        IteratorKind::TableKeys,
        "Set.prototype.values",
    )
}

fn set_entries(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    new_table_iterator(
        agent,
        this,
        Class::Set,
        IteratorKind::TableEntries,
        "Set.prototype.entries",
    )
}

/// `GetSetRecord(obj)` (§24.2.1.2): the size and the `has` and `keys`
/// methods of a set-like argument.
struct SetRecord {
    object: Value,
    size: f64,
    has: Value,
    keys: Value,
}

fn set_record(agent: &mut Agent, value: Value) -> JsResult<SetRecord> {
    let Some(o) = value.as_object() else {
        return agent.type_error("set operation argument is not an object");
    };
    let size_key = agent.heap.key("size")?;
    let raw_size = agent.get(o, size_key)?;
    let num_size = agent.to_number(raw_size)?;
    if num_size.is_nan() {
        return agent.type_error("set-like size is not a number");
    }
    let size = crate::runtime::number::to_integer_or_infinity(num_size);
    if size < 0.0 {
        return Err(agent.throw(
            crate::runtime::ErrorKind::RangeError,
            "set-like size is negative",
        ));
    }
    let has_key = agent.heap.key("has")?;
    let has = agent.get(o, has_key)?;
    if !agent.is_callable(has) {
        return agent.type_error("set-like has is not a function");
    }
    let keys_key = agent.heap.key("keys")?;
    let keys = agent.get(o, keys_key)?;
    if !agent.is_callable(keys) {
        return agent.type_error("set-like keys is not a function");
    }
    Ok(SetRecord {
        object: value,
        size,
        has,
        keys,
    })
}

/// Walks `record.keys()`, calling `each` with every key (-0 as +0) until
/// it returns false, which closes the iterator.
fn each_key(
    agent: &mut Agent,
    record: &SetRecord,
    mut each: impl FnMut(&mut Agent, Value) -> JsResult<bool>,
) -> JsResult<()> {
    let iterator = agent.call(record.keys, record.object, &[])?;
    let Some(iterator_object) = iterator.as_object() else {
        return agent.type_error("keys() did not return an object");
    };
    let next = agent.get(iterator_object, PropertyKey::String(agent.names.next))?;
    loop {
        let result = agent.call(next, iterator, &[])?;
        let Some(result) = result.as_object() else {
            return agent.type_error("iterator result is not an object");
        };
        let done = agent.get(result, PropertyKey::String(agent.names.done))?;
        if agent.to_boolean(done) {
            return Ok(());
        }
        let mut value = agent.get(result, PropertyKey::String(agent.names.value))?;
        if value.as_number() == Some(0.0) {
            value = Value::number(0.0);
        }
        if !each(agent, value)? {
            let key = PropertyKey::String(agent.names.return_);
            if let Some(close) = agent.get_method(iterator, key)? {
                let r = agent.call(close, iterator, &[])?;
                if r.as_object().is_none() {
                    return agent.type_error("iterator result is not an object");
                }
            }
            return Ok(());
        }
    }
}

fn record_has(agent: &mut Agent, record: &SetRecord, value: Value) -> JsResult<bool> {
    let r = agent.call(record.has, record.object, &[value])?;
    Ok(agent.to_boolean(r))
}

/// The live values of this set, in order (a copy, so callbacks may
/// change the set).
fn set_snapshot(agent: &mut Agent, this: Value, name: &str) -> JsResult<Vec<Value>> {
    with_table(agent, this, Class::Set, name, |_, t| {
        (0..t.extent())
            .filter_map(|p| t.at(p).map(|(k, _)| k))
            .collect()
    })
}

fn set_from(agent: &mut Agent, values: &[Value]) -> JsResult<Value> {
    let prototype = own_set_prototype(agent)?;
    let mut object = Object::new(Class::Set, Some(prototype));
    let mut table = OrderedTable::default();
    for &v in values {
        if table.set(agent, v, v) {
            agent.heap.charge(TABLE_ENTRY_BYTES)?;
        }
    }
    object.extra_mut().internal = Some(Internal::Table(Box::new(table)));
    Ok(Value::object(agent.heap.alloc_object(object)?))
}

fn own_set_prototype(agent: &mut Agent) -> JsResult<ObjectId> {
    let global = agent.intrinsics.global;
    let key = agent.heap.key("Set")?;
    let set = agent.get(global, key)?;
    let prototype = match set.as_object() {
        Some(s) => {
            let key = PropertyKey::String(agent.names.prototype);
            agent.get(s, key)?.as_object()
        }
        None => None,
    };
    Ok(prototype.unwrap_or(agent.intrinsics.object_prototype))
}

/// Walks this set's entries live, by position, as the set methods do
/// with `O.[[SetData]]`: entries added meanwhile are visited. Holds the
/// table's positions while `each` runs; stops when `each` says so.
fn walk_this(
    agent: &mut Agent,
    this: Value,
    mut each: impl FnMut(&mut Agent, Value) -> JsResult<bool>,
) -> JsResult<()> {
    let o = this.as_object().expect("a set was checked");
    adjust_loops(agent, o, 1);
    let mut position = 0;
    let result = loop {
        let step = with_table(agent, this, Class::Set, "Set method", |_, t| {
            (position..t.extent()).find_map(|p| t.at(p).map(|(k, _)| (p, k)))
        });
        match step {
            Ok(Some((p, v))) => {
                position = p + 1;
                match each(agent, v) {
                    Ok(true) => {}
                    Ok(false) => break Ok(()),
                    Err(e) => break Err(e),
                }
            }
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    adjust_loops(agent, o, -1);
    result
}

fn this_set_has(agent: &mut Agent, this: Value, value: Value) -> JsResult<bool> {
    with_table(agent, this, Class::Set, "Set method", |agent, t| {
        t.has(agent, value)
    })
}

fn set_union(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    with_table(agent, this, Class::Set, "Set.prototype.union", |_, _| ())?;
    let record = set_record(agent, arg(args, 0))?;
    let mut values = set_snapshot(agent, this, "Set.prototype.union")?;
    each_key(agent, &record, |agent, v| {
        agent.heap.charge(8)?;
        values.push(v);
        Ok(true)
    })?;
    set_from(agent, &values)
}

fn set_intersection(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    with_table(
        agent,
        this,
        Class::Set,
        "Set.prototype.intersection",
        |_, _| (),
    )?;
    let record = set_record(agent, arg(args, 0))?;
    let size = set_snapshot(agent, this, "Set.prototype.intersection")?.len();
    let mut out = Vec::new();
    if size as f64 <= record.size {
        walk_this(agent, this, |agent, v| {
            if record_has(agent, &record, v)? {
                out.push(v);
            }
            Ok(true)
        })?;
    } else {
        each_key(agent, &record, |agent, v| {
            if this_set_has(agent, this, v)? {
                out.push(v);
            }
            Ok(true)
        })?;
    }
    set_from(agent, &out)
}

fn set_difference(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    with_table(
        agent,
        this,
        Class::Set,
        "Set.prototype.difference",
        |_, _| (),
    )?;
    let record = set_record(agent, arg(args, 0))?;
    let mine = set_snapshot(agent, this, "Set.prototype.difference")?;
    let result = set_from(agent, &mine)?;
    if mine.len() as f64 <= record.size {
        for v in mine {
            if record_has(agent, &record, v)? {
                set_delete(agent, result, &[v])?;
            }
        }
    } else {
        each_key(agent, &record, |agent, v| {
            set_delete(agent, result, &[v])?;
            Ok(true)
        })?;
    }
    Ok(result)
}

fn set_symmetric_difference(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    with_table(
        agent,
        this,
        Class::Set,
        "Set.prototype.symmetricDifference",
        |_, _| (),
    )?;
    let record = set_record(agent, arg(args, 0))?;
    let mine = set_snapshot(agent, this, "Set.prototype.symmetricDifference")?;
    let result = set_from(agent, &mine)?;
    each_key(agent, &record, |agent, v| {
        if this_set_has(agent, this, v)? {
            set_delete(agent, result, &[v])?;
        } else {
            set_add(agent, result, &[v])?;
        }
        Ok(true)
    })?;
    Ok(result)
}

fn set_is_subset_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    with_table(
        agent,
        this,
        Class::Set,
        "Set.prototype.isSubsetOf",
        |_, _| (),
    )?;
    let record = set_record(agent, arg(args, 0))?;
    let size = set_snapshot(agent, this, "Set.prototype.isSubsetOf")?.len();
    if size as f64 > record.size {
        return Ok(Value::FALSE);
    }
    let mut all = true;
    walk_this(agent, this, |agent, v| {
        all = record_has(agent, &record, v)?;
        Ok(all)
    })?;
    Ok(Value::boolean(all))
}

fn set_is_superset_of(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    with_table(
        agent,
        this,
        Class::Set,
        "Set.prototype.isSupersetOf",
        |_, _| (),
    )?;
    let record = set_record(agent, arg(args, 0))?;
    let size = set_snapshot(agent, this, "Set.prototype.isSupersetOf")?.len();
    if (size as f64) < record.size {
        return Ok(Value::FALSE);
    }
    let mut all = true;
    each_key(agent, &record, |agent, v| {
        all = this_set_has(agent, this, v)?;
        Ok(all)
    })?;
    Ok(Value::boolean(all))
}

fn set_is_disjoint_from(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    with_table(
        agent,
        this,
        Class::Set,
        "Set.prototype.isDisjointFrom",
        |_, _| (),
    )?;
    let record = set_record(agent, arg(args, 0))?;
    let size = set_snapshot(agent, this, "Set.prototype.isDisjointFrom")?.len();
    if size as f64 <= record.size {
        let mut disjoint = true;
        walk_this(agent, this, |agent, v| {
            disjoint = !record_has(agent, &record, v)?;
            Ok(disjoint)
        })?;
        return Ok(Value::boolean(disjoint));
    }
    let mut disjoint = true;
    each_key(agent, &record, |agent, v| {
        disjoint = !this_set_has(agent, this, v)?;
        Ok(disjoint)
    })?;
    Ok(Value::boolean(disjoint))
}

// ---- groupBy --------------------------------------------------------------------

/// `GroupBy(items, callback, keyCoercion)` (§7.3.35): the groups in
/// order of first appearance.
fn group_by(
    agent: &mut Agent,
    items: Value,
    callback: Value,
    property_keys: bool,
) -> JsResult<Vec<(Value, Vec<Value>)>> {
    agent.require_object_coercible(items)?;
    if !agent.is_callable(callback) {
        return agent.type_error("groupBy: callback is not a function");
    }
    let iterator_key = PropertyKey::Symbol(agent.symbols.iterator);
    let Some(method) = agent.get_method(items, iterator_key)? else {
        return agent.type_error("groupBy: items are not iterable");
    };
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    let mut k = 0.0;
    iterate(agent, items, method, |agent, value| {
        let mut key = agent.call(callback, Value::UNDEFINED, &[value, Value::number(k)])?;
        if property_keys {
            key = match agent.to_property_key(key)? {
                PropertyKey::Symbol(s) => Value::symbol(s),
                PropertyKey::String(s) => Value::string(s),
                PropertyKey::Index(i) => agent.string(&i.to_string())?,
            };
        } else if key.as_number() == Some(0.0) {
            key = Value::number(0.0);
        }
        agent.heap.charge(8)?;
        match groups.iter_mut().find(|(g, _)| agent.same_value(*g, key)) {
            Some((_, members)) => members.push(value),
            None => groups.push((key, vec![value])),
        }
        k += 1.0;
        Ok(true)
    })?;
    Ok(groups)
}

fn object_group_by(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let groups = group_by(agent, arg(args, 0), arg(args, 1), true)?;
    let o = agent.new_object_with(None)?;
    for (key, members) in groups {
        let key = agent.to_property_key(key)?;
        let array = agent.create_array_from_list(&members)?;
        agent.create_data_property_or_throw(o, key, Value::object(array))?;
    }
    Ok(Value::object(o))
}

fn map_group_by(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let groups = group_by(agent, arg(args, 0), arg(args, 1), false)?;
    let global = agent.intrinsics.global;
    let key = agent.heap.key("Map")?;
    let map = agent.get(global, key)?;
    let map = agent.construct(map, &[])?;
    for (key, members) in groups {
        let array = agent.create_array_from_list(&members)?;
        map_set(agent, map, &[key, Value::object(array)])?;
    }
    Ok(map)
}

// ---- weak collections ---------------------------------------------------------

fn weak_map_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    require_new(agent, "WeakMap")?;
    let default = own_prototype(agent)?;
    let prototype = prototype_from_new_target(agent, default)?;
    let map = agent.new_weak_map()?;
    agent.set_prototype_of(map, Some(prototype))?;
    fill_from_iterable(agent, map, arg(args, 0), "set", true)?;
    Ok(Value::object(map))
}

fn weak_set_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    require_new(agent, "WeakSet")?;
    let default = own_prototype(agent)?;
    let prototype = prototype_from_new_target(agent, default)?;
    let set = agent.new_weak_set()?;
    agent.set_prototype_of(set, Some(prototype))?;
    fill_from_iterable(agent, set, arg(args, 0), "add", false)?;
    Ok(Value::object(set))
}

fn weak_receiver(agent: &mut Agent, this: Value, class: Class, name: &str) -> JsResult<ObjectId> {
    match this.as_object() {
        Some(o) if agent.class(o) == class => Ok(o),
        _ => agent.type_error(&format!("{name} called on an incompatible receiver")),
    }
}

fn weak_any_receiver(agent: &mut Agent, this: Value, name: &str) -> JsResult<ObjectId> {
    match this.as_object() {
        Some(o) if matches!(agent.class(o), Class::WeakMap | Class::WeakSet) => Ok(o),
        _ => agent.type_error(&format!("{name} called on an incompatible receiver")),
    }
}

fn weak_map_get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_receiver(agent, this, Class::WeakMap, "WeakMap.prototype.get")?;
    Ok(agent.weak_map_get(o, arg(args, 0)))
}

fn weak_map_has(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_any_receiver(agent, this, "has")?;
    Ok(Value::boolean(agent.weak_map_has(o, arg(args, 0))))
}

fn weak_map_delete(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_any_receiver(agent, this, "delete")?;
    Ok(Value::boolean(agent.weak_map_delete(o, arg(args, 0))))
}

fn weak_map_get_or_insert(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_receiver(agent, this, Class::WeakMap, "WeakMap.prototype.getOrInsert")?;
    let (key, value) = (arg(args, 0), arg(args, 1));
    if !agent.can_be_held_weakly(key) {
        return agent.type_error("invalid value used as weak map key");
    }
    if agent.weak_map_has(o, key) {
        return Ok(agent.weak_map_get(o, key));
    }
    agent.weak_map_set(o, key, value)?;
    Ok(value)
}

fn weak_map_get_or_insert_computed(
    agent: &mut Agent,
    this: Value,
    args: &[Value],
) -> JsResult<Value> {
    let o = weak_receiver(
        agent,
        this,
        Class::WeakMap,
        "WeakMap.prototype.getOrInsertComputed",
    )?;
    let (key, callback) = (arg(args, 0), arg(args, 1));
    if !agent.can_be_held_weakly(key) {
        return agent.type_error("invalid value used as weak map key");
    }
    if !agent.is_callable(callback) {
        return agent
            .type_error("WeakMap.prototype.getOrInsertComputed: callback is not a function");
    }
    if agent.weak_map_has(o, key) {
        return Ok(agent.weak_map_get(o, key));
    }
    let value = agent.call(callback, Value::UNDEFINED, &[key])?;
    agent.weak_map_set(o, key, value)?;
    Ok(value)
}

fn weak_map_set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_receiver(agent, this, Class::WeakMap, "WeakMap.prototype.set")?;
    agent.weak_map_set(o, arg(args, 0), arg(args, 1))?;
    Ok(this)
}

fn weak_set_add(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_receiver(agent, this, Class::WeakSet, "WeakSet.prototype.add")?;
    agent.weak_map_set(o, arg(args, 0), Value::TRUE)?;
    Ok(this)
}

fn weak_ref_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    require_new(agent, "WeakRef")?;
    let target = arg(args, 0);
    if !agent.can_be_held_weakly(target) {
        return agent.type_error("WeakRef: invalid target");
    }
    let default = own_prototype(agent)?;
    let prototype = prototype_from_new_target(agent, default)?;
    let weak_ref = agent.new_weak_ref(target)?;
    agent.set_prototype_of(weak_ref, Some(prototype))?;
    Ok(Value::object(weak_ref))
}

fn weak_ref_deref(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let o = weak_receiver(agent, this, Class::WeakRef, "WeakRef.prototype.deref")?;
    Ok(agent.weak_ref_deref(o))
}

fn registry_constructor(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    require_new(agent, "FinalizationRegistry")?;
    if !agent.is_callable(arg(args, 0)) {
        return agent.type_error("FinalizationRegistry: cleanup callback is not a function");
    }
    let default = own_prototype(agent)?;
    let prototype = prototype_from_new_target(agent, default)?;
    let registry = agent.new_finalization_registry()?;
    agent.set_prototype_of(registry, Some(prototype))?;
    Ok(Value::object(registry))
}

fn registry_register(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_receiver(
        agent,
        this,
        Class::FinalizationRegistry,
        "FinalizationRegistry.prototype.register",
    )?;
    let (target, held, token) = (arg(args, 0), arg(args, 1), arg(args, 2));
    if !agent.can_be_held_weakly(target) {
        return agent.type_error("register: invalid target");
    }
    if agent.same_value(target, held) {
        return agent.type_error("register: target and held value must differ");
    }
    if token != Value::UNDEFINED && !agent.can_be_held_weakly(token) {
        return agent.type_error("register: invalid unregister token");
    }
    agent.registry_register(
        o,
        target,
        held,
        (token != Value::UNDEFINED).then_some(token),
    )?;
    Ok(Value::UNDEFINED)
}

fn registry_unregister(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = weak_receiver(
        agent,
        this,
        Class::FinalizationRegistry,
        "FinalizationRegistry.prototype.unregister",
    )?;
    let token = arg(args, 0);
    if !agent.can_be_held_weakly(token) {
        return agent.type_error("unregister: invalid unregister token");
    }
    Ok(Value::boolean(agent.registry_unregister(o, token)?))
}
