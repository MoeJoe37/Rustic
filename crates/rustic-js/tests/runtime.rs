//! JS-02 data model through the public API: memory layout, ordinary and
//! exotic object internal methods, conversions, comparisons, environments
//! and references.

use std::mem::size_of;

use rustic_js::runtime::{
    Abrupt, Agent, Class, IntegrityLevel, JsResult, JsStr, Kind, ObjectId, PropertyDescriptor,
    PropertyKey, Reference, ReferenceBase, Value,
};

fn agent() -> Agent {
    Agent::new().expect("a default heap fits the intrinsics")
}

fn key(agent: &mut Agent, text: &str) -> PropertyKey {
    agent.heap.key(text).unwrap()
}

fn text(agent: &Agent, value: Value) -> String {
    agent.display(value)
}

/// The prototype of a thrown error, to tell TypeError from RangeError.
fn thrown_proto(agent: &Agent, result: JsResult<impl std::fmt::Debug>) -> ObjectId {
    match result {
        Err(Abrupt::Throw(v)) => agent
            .get_prototype_of(v.as_object().expect("errors are objects"))
            .unwrap(),
        other => panic!("expected a throw, got {other:?}"),
    }
}

fn names(agent: &Agent, keys: &[PropertyKey]) -> Vec<String> {
    keys.iter()
        .map(|k| match *k {
            PropertyKey::Index(i) => i.to_string(),
            PropertyKey::String(s) => agent.heap.string(s).to_text(),
            PropertyKey::Symbol(s) => format!("@{}", s.index()),
        })
        .collect()
}

// ---- memory -----------------------------------------------------------

#[test]
fn objects_built_alike_share_shapes_and_cost_eight_bytes_per_property() {
    let mut a = agent();
    let (x, y) = (key(&mut a, "x"), key(&mut a, "y"));
    let shapes = a.heap.shape_count();
    let before = a.heap.bytes_used();
    for i in 0..10_000 {
        let o = a.new_object().unwrap();
        a.create_data_property_or_throw(o, x, Value::number(f64::from(i)))
            .unwrap();
        a.create_data_property_or_throw(o, y, Value::TRUE).unwrap();
    }
    // Two new shapes serve every object.
    assert_eq!(a.heap.shape_count() - shapes, 2);
    let per_object = (a.heap.bytes_used() - before) / 10_000;
    assert!(
        per_object <= 72 + 16,
        "{per_object} bytes per two-property object"
    );
    assert_eq!(std::mem::size_of::<Value>(), 8);
}

#[test]
fn latin1_strings_take_one_byte_per_code_unit() {
    let mut a = agent();
    let before = a.heap.bytes_used();
    a.string(&"a".repeat(1000)).unwrap();
    assert_eq!(a.heap.bytes_used() - before, size_of::<JsStr>() + 1000);
    let before = a.heap.bytes_used();
    a.string(&"€".repeat(1000)).unwrap();
    assert_eq!(a.heap.bytes_used() - before, size_of::<JsStr>() + 2000);
}

#[test]
fn deleting_or_many_properties_switch_to_a_dictionary_without_losing_order() {
    let mut a = agent();
    let o = a.new_object().unwrap();
    let keys: Vec<PropertyKey> = (0..40).map(|i| key(&mut a, &format!("p{i}"))).collect();
    for (i, k) in keys.iter().enumerate() {
        a.create_data_property_or_throw(o, *k, Value::number(i as f64))
            .unwrap();
    }
    a.delete(o, keys[3]).unwrap();
    let own = a.own_property_keys(o);
    assert_eq!(own.len(), 39);
    assert_eq!(own[3], keys[4]);
    assert_eq!(a.get(o, keys[39]).unwrap(), Value::number(39.0));
    // Deleting the newest property of a shared shape keeps sharing.
    let p = a.new_object().unwrap();
    a.create_data_property_or_throw(p, keys[0], Value::NULL)
        .unwrap();
    a.create_data_property_or_throw(p, keys[1], Value::NULL)
        .unwrap();
    let shapes = a.heap.shape_count();
    a.delete(p, keys[1]).unwrap();
    a.create_data_property_or_throw(p, keys[1], Value::TRUE)
        .unwrap();
    assert_eq!(a.heap.shape_count(), shapes);
    assert_eq!(a.get(p, keys[1]).unwrap(), Value::TRUE);
}

#[test]
fn the_heap_budget_fails_cleanly() {
    let mut a = Agent::with_heap_limit(64 * 1024).unwrap();
    let mut result = Ok(());
    for _ in 0..10_000 {
        if let Err(e) = a.new_object() {
            result = Err(e);
            break;
        }
    }
    assert_eq!(result, Err(Abrupt::OutOfMemory));
    assert!(a.heap.bytes_used() <= a.heap.byte_limit());
    assert!(Agent::with_heap_limit(100).is_err());
}

// ---- ordinary objects ---------------------------------------------------

#[test]
fn define_own_property_validates_against_the_current_descriptor() {
    let mut a = agent();
    let o = a.new_object().unwrap();
    let k = key(&mut a, "k");
    assert!(
        a.define_own_property(
            o,
            k,
            PropertyDescriptor::data_with(Value::TRUE, false, true, false)
        )
        .unwrap()
    );
    // Missing fields of a new property default to false.
    let desc = a.get_own_property(o, k).unwrap().unwrap();
    assert_eq!(
        desc,
        PropertyDescriptor::data_with(Value::TRUE, false, true, false)
    );
    // A non-configurable, non-writable property accepts only the same value.
    assert!(
        a.define_own_property(
            o,
            k,
            PropertyDescriptor {
                value: Some(Value::TRUE),
                ..Default::default()
            }
        )
        .unwrap()
    );
    assert!(
        !a.define_own_property(
            o,
            k,
            PropertyDescriptor {
                value: Some(Value::FALSE),
                ..Default::default()
            }
        )
        .unwrap()
    );
    assert!(
        !a.define_own_property(
            o,
            k,
            PropertyDescriptor {
                configurable: Some(true),
                ..Default::default()
            }
        )
        .unwrap()
    );
    assert!(
        !a.define_own_property(
            o,
            k,
            PropertyDescriptor {
                enumerable: Some(false),
                ..Default::default()
            }
        )
        .unwrap()
    );
    assert!(
        !a.define_own_property(
            o,
            k,
            PropertyDescriptor::accessor(Value::UNDEFINED, Value::UNDEFINED, true, false)
        )
        .unwrap()
    );
    assert!(!a.delete(o, k).unwrap());
    // +0 and -0 are different values here.
    let z = key(&mut a, "z");
    a.define_own_property(
        o,
        z,
        PropertyDescriptor::data_with(Value::number(0.0), false, false, false),
    )
    .unwrap();
    assert!(
        !a.define_own_property(
            o,
            z,
            PropertyDescriptor {
                value: Some(Value::number(-0.0)),
                ..Default::default()
            }
        )
        .unwrap()
    );
    // A configurable property can change kind, keeping its flags.
    let c = key(&mut a, "c");
    a.create_data_property(o, c, Value::NULL).unwrap();
    assert!(
        a.define_own_property(
            o,
            c,
            PropertyDescriptor {
                get: Some(Value::UNDEFINED),
                ..Default::default()
            }
        )
        .unwrap()
    );
    let desc = a.get_own_property(o, c).unwrap().unwrap();
    assert!(desc.is_accessor() && desc.enumerable == Some(true) && desc.configurable == Some(true));
    assert_eq!(desc.set, Some(Value::UNDEFINED));
}

fn getter(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    // Returns the receiver, to check what [[Get]] passes.
    let _ = agent;
    Ok(this)
}

fn recording_setter(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let o = this.as_object().unwrap();
    let k = agent.heap.key("seen").unwrap();
    agent.create_data_property_or_throw(o, k, args[0])?;
    Ok(Value::UNDEFINED)
}

#[test]
fn get_and_set_follow_the_prototype_chain_and_receivers() {
    let mut a = agent();
    let proto = a.new_object().unwrap();
    let child = a.new_object_with(Some(proto)).unwrap();
    let (inherited, ro, acc) = (
        key(&mut a, "inherited"),
        key(&mut a, "ro"),
        key(&mut a, "acc"),
    );
    a.create_data_property(proto, inherited, Value::number(1.0))
        .unwrap();
    a.define_own_property(
        proto,
        ro,
        PropertyDescriptor::data_with(Value::number(2.0), false, true, true),
    )
    .unwrap();
    let g = a.new_native_function(getter, "get", 0).unwrap();
    let s = a.new_native_function(recording_setter, "set", 1).unwrap();
    a.define_own_property(
        proto,
        acc,
        PropertyDescriptor::accessor(Value::object(g), Value::object(s), true, true),
    )
    .unwrap();

    assert_eq!(a.get(child, inherited).unwrap(), Value::number(1.0));
    // Assignment creates an own property on the receiver, shadowing.
    a.set(child, inherited, Value::number(5.0), true).unwrap();
    assert_eq!(a.get(child, inherited).unwrap(), Value::number(5.0));
    assert_eq!(a.get(proto, inherited).unwrap(), Value::number(1.0));
    // An inherited read-only property blocks assignment.
    assert!(
        !a.set_with_receiver(child, ro, Value::NULL, Value::object(child))
            .unwrap()
    );
    let type_error = a.intrinsics.type_error_prototype;
    let r = a.set(child, ro, Value::NULL, true);
    assert_eq!(thrown_proto(&a, r), type_error);
    // Accessors see the original receiver.
    assert_eq!(a.get(child, acc).unwrap(), Value::object(child));
    a.set(child, acc, Value::TRUE, true).unwrap();
    let seen = key(&mut a, "seen");
    assert_eq!(a.get(child, seen).unwrap(), Value::TRUE);
    assert!(a.has_property(child, ro).unwrap());
    assert!(!a.has_own_property(child, ro).unwrap());
}

#[test]
fn own_keys_put_indices_first_then_strings_then_symbols() {
    let mut a = agent();
    let o = a.new_object().unwrap();
    let sym = a.heap.alloc_symbol(None).unwrap();
    for k in ["b", "10", "a", "2"] {
        let k = key(&mut a, k);
        a.create_data_property(o, k, Value::NULL).unwrap();
    }
    a.create_data_property(o, PropertyKey::Symbol(sym), Value::NULL)
        .unwrap();
    let c = key(&mut a, "c");
    a.create_data_property(o, c, Value::NULL).unwrap();
    let keys = a.own_property_keys(o);
    assert_eq!(
        names(&a, &keys),
        ["2", "10", "b", "a", "c", &format!("@{}", sym.index())]
    );
}

#[test]
fn prototypes_reject_cycles_and_non_extensible_objects() {
    let mut a = agent();
    let x = a.new_object().unwrap();
    let y = a.new_object_with(Some(x)).unwrap();
    assert!(!a.set_prototype_of(x, Some(y)));
    assert!(a.set_prototype_of(x, None));
    a.prevent_extensions(y);
    assert!(!a.set_prototype_of(y, None));
    assert!(
        a.set_prototype_of(y, Some(x)),
        "unchanged prototype is fine"
    );
    let k = key(&mut a, "new");
    assert!(!a.create_data_property(y, k, Value::NULL).unwrap());
}

#[test]
fn freezing_and_sealing() {
    let mut a = agent();
    let o = a.new_object().unwrap();
    let k = key(&mut a, "k");
    a.create_data_property(o, k, Value::TRUE).unwrap();
    assert!(!a.test_integrity_level(o, IntegrityLevel::Sealed).unwrap());
    a.set_integrity_level(o, IntegrityLevel::Frozen).unwrap();
    assert!(a.test_integrity_level(o, IntegrityLevel::Frozen).unwrap());
    assert!(
        !a.set_with_receiver(o, k, Value::FALSE, Value::object(o))
            .unwrap()
    );
    assert_eq!(a.get(o, k).unwrap(), Value::TRUE);
}

// ---- Array and String exotic objects -------------------------------------

#[test]
fn array_length_tracks_indices_and_truncates() {
    let mut a = agent();
    let arr = a.new_array().unwrap();
    let length = key(&mut a, "length");
    for i in 0..5 {
        a.create_data_property(arr, PropertyKey::Index(i), Value::number(f64::from(i)))
            .unwrap();
    }
    assert_eq!(a.get(arr, length).unwrap(), Value::number(5.0));
    a.set(arr, length, Value::number(2.0), true).unwrap();
    assert_eq!(a.get(arr, PropertyKey::Index(3)).unwrap(), Value::UNDEFINED);
    assert_eq!(names(&a, &a.own_property_keys(arr)), ["0", "1", "length"]);
    // A far index makes the elements sparse but keeps semantics.
    a.create_data_property(arr, PropertyKey::Index(1_000_000), Value::TRUE)
        .unwrap();
    assert_eq!(a.get(arr, length).unwrap(), Value::number(1_000_001.0));
    assert_eq!(
        a.get(arr, PropertyKey::Index(1)).unwrap(),
        Value::number(1.0)
    );
    // A non-configurable element stops truncation just above it.
    a.define_own_property(
        arr,
        PropertyKey::Index(1),
        PropertyDescriptor {
            configurable: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !a.define_own_property(
            arr,
            length,
            PropertyDescriptor {
                value: Some(Value::number(0.0)),
                ..Default::default()
            }
        )
        .unwrap()
    );
    assert_eq!(a.get(arr, length).unwrap(), Value::number(2.0));
    // Invalid lengths throw RangeError.
    let range_error = a.intrinsics.range_error_prototype;
    let bad = a.define_own_property(
        arr,
        length,
        PropertyDescriptor {
            value: Some(Value::number(1.5)),
            ..Default::default()
        },
    );
    assert_eq!(thrown_proto(&a, bad), range_error);
    // A read-only length blocks appending.
    a.define_own_property(
        arr,
        length,
        PropertyDescriptor {
            writable: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !a.create_data_property(arr, PropertyKey::Index(7), Value::NULL)
            .unwrap()
    );
}

#[test]
fn dense_arrays_cost_eight_bytes_per_element() {
    let mut a = agent();
    let values: Vec<Value> = (0..1000).map(|i| Value::number(f64::from(i))).collect();
    let before = a.heap.bytes_used();
    let arr = a.create_array_from_list(&values).unwrap();
    assert!(a.heap.bytes_used() - before <= 72 + 8 * 1000);
    assert_eq!(a.length_of_array_like(arr).unwrap(), 1000.0);
    assert_eq!(
        a.get(arr, PropertyKey::Index(999)).unwrap(),
        Value::number(999.0)
    );
}

#[test]
fn string_objects_expose_read_only_indices() {
    let mut a = agent();
    let s = a.string("hé").unwrap();
    let o = a.to_object(s).unwrap();
    assert_eq!(a.class(o), Class::String);
    let first = a.get(o, PropertyKey::Index(1)).unwrap();
    assert_eq!(text(&a, first), "é");
    let length = key(&mut a, "length");
    assert_eq!(a.get(o, length).unwrap(), Value::number(2.0));
    assert!(
        !a.set_with_receiver(o, PropertyKey::Index(0), Value::NULL, Value::object(o))
            .unwrap()
    );
    assert!(!a.delete(o, PropertyKey::Index(0)).unwrap());
    a.create_data_property(o, PropertyKey::Index(5), Value::TRUE)
        .unwrap();
    assert_eq!(
        names(&a, &a.own_property_keys(o)),
        ["0", "1", "5", "length"]
    );
    // Primitives read through their prototype without a wrapper.
    assert_eq!(a.get_v(s, length).unwrap(), Value::number(2.0));
}

// ---- conversions ----------------------------------------------------------

fn value_of_seven(_: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(7.0))
}

fn hint_echo(_: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    Ok(args[0])
}

#[test]
fn to_primitive_uses_value_of_to_string_and_to_primitive() {
    let mut a = agent();
    let o = a.new_object().unwrap();
    let s = a.to_string(Value::object(o)).unwrap();
    assert_eq!(a.heap.string(s).to_text(), "[object Object]");
    assert!(a.to_number(Value::object(o)).unwrap().is_nan());
    let value_of = key(&mut a, "valueOf");
    let f = a.new_native_function(value_of_seven, "valueOf", 0).unwrap();
    a.create_data_property(o, value_of, Value::object(f))
        .unwrap();
    assert_eq!(a.to_number(Value::object(o)).unwrap(), 7.0);
    // String hint still prefers toString.
    let s = a.to_string(Value::object(o)).unwrap();
    assert_eq!(a.heap.string(s).to_text(), "[object Object]");
    let to_primitive = PropertyKey::Symbol(a.symbols.to_primitive);
    let f = a.new_native_function(hint_echo, "", 1).unwrap();
    a.create_data_property(o, to_primitive, Value::object(f))
        .unwrap();
    let hint = a.to_primitive(Value::object(o), None).unwrap();
    assert_eq!(text(&a, hint), "default");
    let arr = a.new_array().unwrap();
    let s = a.to_string(Value::object(arr)).unwrap();
    assert_eq!(a.heap.string(s).to_text(), "");
}

#[test]
fn primitive_conversions() {
    let mut a = agent();
    let to_text = |a: &mut Agent, v: Value| {
        let s = a.to_string(v).unwrap();
        a.heap.string(s).to_text()
    };
    assert_eq!(to_text(&mut a, Value::number(-0.0)), "0");
    assert_eq!(to_text(&mut a, Value::number(1e21)), "1e+21");
    assert_eq!(to_text(&mut a, Value::NULL), "null");
    assert_eq!(to_text(&mut a, Value::UNDEFINED), "undefined");
    let sym = Value::symbol(a.heap.alloc_symbol(None).unwrap());
    let type_error = a.intrinsics.type_error_prototype;
    let r = a.to_string(sym);
    assert_eq!(thrown_proto(&a, r), type_error);
    let n = a.string(" 0x10 ").unwrap();
    assert_eq!(a.to_number(n).unwrap(), 16.0);
    let empty = a.string("").unwrap();
    assert!(!a.to_boolean(empty));
    assert!(a.to_boolean(sym));
    assert!(!a.to_boolean(Value::NAN));
    let one = a.string("1").unwrap();
    assert_eq!(a.to_property_key(one).unwrap(), PropertyKey::Index(1));
    assert_eq!(
        a.to_property_key(Value::number(1.5)).unwrap(),
        key(&mut a, "1.5")
    );
    let wrapper = a.to_object(Value::TRUE).unwrap();
    assert_eq!(a.to_number(Value::object(wrapper)).unwrap(), 1.0);
    let r = a.to_object(Value::NULL);
    assert_eq!(thrown_proto(&a, r), type_error);
}

#[test]
fn equality_and_relational_comparison() {
    let mut a = agent();
    let one = a.string("1").unwrap();
    let one_again = a.string("1").unwrap();
    assert!(a.is_strictly_equal(one, one_again));
    assert!(!a.is_strictly_equal(Value::NAN, Value::NAN));
    assert!(a.same_value(Value::NAN, Value::NAN));
    assert!(!a.same_value(Value::number(0.0), Value::number(-0.0)));
    assert!(a.same_value_zero(Value::number(0.0), Value::number(-0.0)));
    assert!(a.is_loosely_equal(Value::NULL, Value::UNDEFINED).unwrap());
    assert!(!a.is_loosely_equal(Value::NULL, Value::number(0.0)).unwrap());
    assert!(a.is_loosely_equal(one, Value::number(1.0)).unwrap());
    assert!(a.is_loosely_equal(Value::TRUE, one).unwrap());
    let big = a
        .heap
        .alloc_bigint(rustic_js::runtime::BigInt::from_digits("1", 10, false).unwrap())
        .unwrap();
    assert!(a.is_loosely_equal(Value::bigint(big), one).unwrap());
    assert!(
        a.is_loosely_equal(Value::bigint(big), Value::number(1.0))
            .unwrap()
    );
    let arr = a.new_array().unwrap();
    let empty = a.string("").unwrap();
    // [] == "" through ToPrimitive and Array.prototype.toString.
    assert!(a.is_loosely_equal(Value::object(arr), empty).unwrap());
    let b = a.string("b").unwrap();
    let ab = a.string("ab").unwrap();
    assert_eq!(a.is_less_than(ab, b, true).unwrap(), Some(true));
    assert_eq!(
        a.is_less_than(Value::NAN, Value::number(1.0), true)
            .unwrap(),
        None
    );
    assert_eq!(
        a.is_less_than(Value::bigint(big), Value::number(1.5), true)
            .unwrap(),
        Some(true)
    );
    let two = a.string("2").unwrap();
    let ten = a.string("10").unwrap();
    assert_eq!(
        a.is_less_than(ten, two, true).unwrap(),
        Some(true),
        "string order"
    );
    assert_eq!(a.type_of(Value::NULL), "object");
    let f = a.new_native_function(value_of_seven, "f", 0).unwrap();
    assert_eq!(a.type_of(Value::object(f)), "function");
    assert_eq!(a.type_of(Value::bigint(big)), "bigint");
}

// ---- environments and references -----------------------------------------

#[test]
fn bindings_have_a_temporal_dead_zone_and_constants() {
    let mut a = agent();
    let env = a.new_declarative_environment(None).unwrap();
    let x = a.heap.intern_str("x").unwrap();
    let c = a.heap.intern_str("c").unwrap();
    a.create_mutable_binding(env, x, false).unwrap();
    let reference_error = a.intrinsics.reference_error_prototype;
    let r = a.get_binding_value(env, x, true);
    assert_eq!(thrown_proto(&a, r), reference_error);
    let r = a.set_mutable_binding(env, x, Value::TRUE, false);
    assert_eq!(thrown_proto(&a, r), reference_error);
    a.initialize_binding(env, x, Value::NULL).unwrap();
    a.set_mutable_binding(env, x, Value::TRUE, true).unwrap();
    assert_eq!(a.get_binding_value(env, x, true).unwrap(), Value::TRUE);
    a.create_immutable_binding(env, c, true).unwrap();
    a.initialize_binding(env, c, Value::number(1.0)).unwrap();
    let type_error = a.intrinsics.type_error_prototype;
    let r = a.set_mutable_binding(env, c, Value::NULL, false);
    assert_eq!(thrown_proto(&a, r), type_error);
    assert!(!a.delete_binding(env, x).unwrap());
}

#[test]
fn references_resolve_through_the_chain_to_the_global_object() {
    let mut a = agent();
    let global = a.intrinsics.global;
    let genv = a.new_global_environment(global, global).unwrap();
    let inner = a.new_declarative_environment(Some(genv)).unwrap();
    let y = a.heap.intern_str("y").unwrap();
    let missing = a.heap.intern_str("missing").unwrap();
    a.create_global_var_binding(genv, y, false).unwrap();
    assert!(a.has_var_declaration(genv, y));
    let r = a.resolve_binding(inner, y, true).unwrap();
    assert_eq!(r.base, ReferenceBase::Environment(genv));
    a.put_value(&r, Value::number(3.0)).unwrap();
    assert_eq!(
        a.get(global, PropertyKey::String(y)).unwrap(),
        Value::number(3.0)
    );
    // Unresolvable: ReferenceError when strict or read, a global when sloppy.
    let strict = a.resolve_binding(inner, missing, true).unwrap();
    assert_eq!(strict.base, ReferenceBase::Unresolvable);
    let reference_error = a.intrinsics.reference_error_prototype;
    let r = a.get_value(&strict);
    assert_eq!(thrown_proto(&a, r), reference_error);
    let r = a.put_value(&strict, Value::NULL);
    assert_eq!(thrown_proto(&a, r), reference_error);
    let sloppy = a.resolve_binding(inner, missing, false).unwrap();
    a.put_value(&sloppy, Value::TRUE).unwrap();
    assert_eq!(
        a.get(global, PropertyKey::String(missing)).unwrap(),
        Value::TRUE
    );
    // `globalThis`, `NaN` and friends exist; `undefined` is read-only.
    let undefined = a.heap.intern_str("undefined").unwrap();
    let r = a.resolve_binding(inner, undefined, true).unwrap();
    let type_error = a.intrinsics.type_error_prototype;
    let w = a.put_value(&r, Value::NULL);
    assert_eq!(thrown_proto(&a, w), type_error);
    assert_eq!(a.get_this_binding(genv).unwrap(), Value::object(global));
}

#[test]
fn with_environments_respect_unscopables_and_property_references_on_primitives() {
    let mut a = agent();
    let o = a.new_object().unwrap();
    let k = a.heap.intern_str("k").unwrap();
    a.create_data_property(o, PropertyKey::String(k), Value::TRUE)
        .unwrap();
    let outer = a.new_declarative_environment(None).unwrap();
    a.create_mutable_binding(outer, k, false).unwrap();
    a.initialize_binding(outer, k, Value::FALSE).unwrap();
    let with = a.new_object_environment(o, true, Some(outer)).unwrap();
    let r = a.resolve_binding(with, k, false).unwrap();
    assert_eq!(a.get_value(&r).unwrap(), Value::TRUE);
    assert_eq!(a.with_base_object(with), Value::object(o));
    let blocked = a.new_object().unwrap();
    a.create_data_property(blocked, PropertyKey::String(k), Value::TRUE)
        .unwrap();
    let unscopables = PropertyKey::Symbol(a.symbols.unscopables);
    a.create_data_property(o, unscopables, Value::object(blocked))
        .unwrap();
    let r = a.resolve_binding(with, k, false).unwrap();
    assert_eq!(a.get_value(&r).unwrap(), Value::FALSE);
    // "abc".length through a property reference.
    let s = a.string("abc").unwrap();
    let length = PropertyKey::String(a.names.length);
    let r = Reference {
        base: ReferenceBase::Value(s),
        name: length,
        strict: true,
        this_value: None,
    };
    assert_eq!(a.get_value(&r).unwrap(), Value::number(3.0));
    // Writing to a primitive fails in strict code.
    let type_error = a.intrinsics.type_error_prototype;
    let w = a.put_value(
        &Reference {
            name: PropertyKey::Index(0),
            ..r
        },
        Value::NULL,
    );
    assert_eq!(thrown_proto(&a, w), type_error);
    assert!(matches!(Value::number(1.0).kind(), Kind::Number(_)));
    let _ = JsStr::empty();
}
