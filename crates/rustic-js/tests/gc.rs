//! JS-03 garbage collector through the public API: reachability from each
//! kind of root, freeing and reuse of every arena, atoms and shapes,
//! budget return, deep graphs, host roots and the collection policy.

use std::collections::{HashMap, HashSet};

use rustic_js::runtime::{
    Abrupt, Agent, BigInt, HostTracer, JsResult, Kind, MIN_GC_THRESHOLD, ObjectId,
    PropertyDescriptor, PropertyKey, Roots, Value,
};

fn agent() -> Agent {
    Agent::new().expect("a default heap fits the intrinsics")
}

fn key(agent: &mut Agent, text: &str) -> PropertyKey {
    agent.heap.key(text).unwrap()
}

/// Collect with nothing held but the intrinsics, and return the bytes the
/// bare agent costs.
fn baseline(agent: &mut Agent) -> usize {
    agent.collect_garbage(Roots::default()).bytes_after
}

fn holding(values: &[Value]) -> Roots<'_> {
    Roots {
        values,
        envs: &[],
        codes: &[],
    }
}

#[test]
fn unreachable_objects_are_freed_and_the_budget_returns_to_baseline() {
    let mut a = agent();
    let base = baseline(&mut a);
    let objects = a.heap.object_count();
    let strings = a.heap.string_count();
    let (x, y) = (key(&mut a, "x"), key(&mut a, "y"));
    for i in 0..50_000 {
        let o = a.new_object().unwrap();
        a.create_data_property_or_throw(o, x, Value::number(f64::from(i)))
            .unwrap();
        let s = a.string(&format!("garbage {i}")).unwrap();
        a.create_data_property_or_throw(o, y, s).unwrap();
        let array = a.create_array_from_list(&[s, Value::TRUE]).unwrap();
        a.create_data_property_or_throw(o, x, Value::object(array))
            .unwrap();
    }
    assert!(a.heap.bytes_used() > base + 50_000 * 100);
    let stats = a.collect_garbage(Roots::default());
    assert_eq!(stats.objects_freed, 100_000);
    assert!(stats.strings_freed >= 50_000);
    assert_eq!(a.heap.object_count(), objects);
    // The unused atoms "x" and "y" go too.
    assert_eq!(a.heap.string_count(), strings);
    assert_eq!(stats.bytes_after, base);
    assert_eq!(a.heap.bytes_used(), base);
}

#[test]
fn reachable_graphs_survive_intact_including_cycles_through_roots() {
    let mut a = agent();
    let next = key(&mut a, "next");
    let label = key(&mut a, "label");
    let head = a.new_object().unwrap();
    let mut tail = head;
    for i in 0..1000 {
        let o = a.new_object().unwrap();
        let s = a.string(&format!("node {i}")).unwrap();
        a.create_data_property_or_throw(o, label, s).unwrap();
        a.create_data_property_or_throw(tail, next, Value::object(o))
            .unwrap();
        tail = o;
    }
    // Close the cycle, then add garbage between live cells so freed and
    // live indices interleave.
    a.create_data_property_or_throw(tail, next, Value::object(head))
        .unwrap();
    for _ in 0..1000 {
        a.new_object().unwrap();
        a.string("dead").unwrap();
    }
    let stats = a.collect_garbage(holding(&[Value::object(head)]));
    assert_eq!(stats.objects_freed, 1000);
    assert_eq!(stats.strings_freed, 1000);
    // Allocations after the collection reuse freed indices, and must not
    // disturb the survivors.
    for i in 0..2000 {
        let o = a.new_object().unwrap();
        a.create_data_property_or_throw(o, label, Value::number(f64::from(i)))
            .unwrap();
    }
    let mut current = a.get(head, next).unwrap().as_object().unwrap();
    for i in 0..1000 {
        let s = a.get(current, label).unwrap();
        assert_eq!(a.display(s), format!("node {i}"));
        current = a.get(current, next).unwrap().as_object().unwrap();
    }
    assert_eq!(current, head);
}

#[test]
fn unrooted_cycles_are_collected() {
    let mut a = agent();
    let base = baseline(&mut a);
    let other = key(&mut a, "other");
    for _ in 0..1000 {
        let p = a.new_object().unwrap();
        let q = a.new_object().unwrap();
        a.create_data_property_or_throw(p, other, Value::object(q))
            .unwrap();
        a.create_data_property_or_throw(q, other, Value::object(p))
            .unwrap();
        a.set_prototype_of(p, Some(q)).unwrap();
    }
    let stats = a.collect_garbage(Roots::default());
    assert_eq!(stats.objects_freed, 2000);
    assert_eq!(a.heap.bytes_used(), base);
}

#[test]
fn deep_chains_mark_without_recursion() {
    // Two hundred thousand nested prototypes and environments: a recursive
    // marker would overflow a 2 MiB test thread long before this.
    let mut a = agent();
    let mut proto = a.new_object().unwrap();
    let bottom = proto;
    for _ in 0..200_000 {
        proto = a.new_object_with(Some(proto)).unwrap();
    }
    let mut env = a.new_declarative_environment(None).unwrap();
    for _ in 0..200_000 {
        env = a.new_declarative_environment(Some(env)).unwrap();
    }
    let objects = a.heap.object_count();
    let envs = a.heap.env_count();
    let stats = a.collect_garbage(Roots {
        values: &[Value::object(proto)],
        envs: &[env],
        codes: &[],
    });
    assert_eq!(stats.objects_freed, 0);
    assert_eq!(stats.envs_freed, 0);
    assert_eq!((a.heap.object_count(), a.heap.env_count()), (objects, envs));
    let mut walk = proto;
    while let Some(p) = a.get_prototype_of(walk).unwrap() {
        walk = p;
        if walk == bottom {
            break;
        }
    }
    assert_eq!(walk, bottom);
    let stats = a.collect_garbage(Roots::default());
    // Everything but the realm's own global environment.
    assert_eq!(stats.envs_freed, envs - 1);
    assert_eq!(stats.objects_freed, 200_001);
}

#[test]
fn environments_keep_their_bindings_outer_scopes_and_objects() {
    let mut a = agent();
    let global = a.intrinsics.global;
    let genv = a.new_global_environment(global, global).unwrap();
    let outer = a.new_declarative_environment(Some(genv)).unwrap();
    let inner = a.new_declarative_environment(Some(outer)).unwrap();
    let held = a.new_object().unwrap();
    let with = a.new_object_environment(held, true, Some(inner)).unwrap();
    let x = a.heap.intern_str("captured").unwrap();
    a.create_mutable_binding(outer, x, false).unwrap();
    let o = a.new_object().unwrap();
    a.initialize_binding(outer, x, Value::object(o)).unwrap();
    let marker = key(&mut a, "marker");
    a.create_data_property_or_throw(o, marker, Value::TRUE)
        .unwrap();
    // A scope nobody holds.
    let dead = a.new_declarative_environment(Some(genv)).unwrap();
    let y = a.heap.intern_str("only in a dead scope").unwrap();
    a.create_mutable_binding(dead, y, false).unwrap();
    let strings = a.heap.string_count();

    let stats = a.collect_garbage(Roots {
        values: &[],
        envs: &[with],
        codes: &[],
    });
    assert_eq!(stats.envs_freed, 1);
    assert_eq!(a.heap.string_count(), strings - 1);
    assert_eq!(a.outer_environment(with), Some(inner));
    let value = a.get_binding_value(outer, x, true).unwrap();
    assert_eq!(value, Value::object(o));
    assert_eq!(a.get(o, marker).unwrap(), Value::TRUE);
    assert_eq!(a.with_base_object(with), Value::object(held));
    // The binding name is still the interned atom.
    assert_eq!(a.heap.intern_str("captured").unwrap(), x);
}

#[test]
fn atoms_are_freed_with_their_last_user_and_stay_deduplicated() {
    let mut a = agent();
    baseline(&mut a);
    let live = a.new_object().unwrap();
    let kept = key(&mut a, "kept");
    a.create_data_property_or_throw(live, kept, Value::TRUE)
        .unwrap();
    let strings = a.heap.string_count();
    for i in 0..10_000 {
        let o = a.new_object().unwrap();
        let k = key(&mut a, &format!("key{i}"));
        a.create_data_property_or_throw(o, k, Value::NULL).unwrap();
    }
    assert_eq!(a.heap.string_count(), strings + 10_000);
    a.collect_garbage(holding(&[Value::object(live)]));
    assert_eq!(a.heap.string_count(), strings);
    assert_eq!(key(&mut a, "kept"), kept);
    assert_eq!(a.get(live, kept).unwrap(), Value::TRUE);
    // A freed atom's text interns afresh, and stays unique.
    let again = key(&mut a, "key5");
    assert_eq!(key(&mut a, "key5"), again);
    assert_ne!(again, kept);
}

#[test]
fn shapes_used_only_by_garbage_are_freed_and_transitions_rebuilt() {
    let mut a = agent();
    let (x, y) = (key(&mut a, "x"), key(&mut a, "y"));
    let survivor = a.new_object().unwrap();
    a.create_data_property_or_throw(survivor, x, Value::number(1.0))
        .unwrap();
    let shapes = a.heap.shape_count();
    for i in 0..5000 {
        let o = a.new_object().unwrap();
        a.create_data_property_or_throw(o, x, Value::NULL).unwrap();
        let k = key(&mut a, &format!("unique{i}"));
        a.create_data_property_or_throw(o, k, Value::NULL).unwrap();
    }
    assert_eq!(a.heap.shape_count(), shapes + 5000);
    let stats = a.collect_garbage(holding(&[Value::object(survivor)]));
    assert_eq!(stats.shapes_freed, 5000);
    assert_eq!(a.heap.shape_count(), shapes);
    // The surviving shape still describes its object, and new objects
    // built the same way share it again.
    assert_eq!(a.get(survivor, x).unwrap(), Value::number(1.0));
    let o = a.new_object().unwrap();
    a.create_data_property_or_throw(o, x, Value::number(2.0))
        .unwrap();
    a.create_data_property_or_throw(o, y, Value::number(3.0))
        .unwrap();
    assert_eq!(a.heap.shape_count(), shapes + 1);
    assert_eq!(a.get(o, y).unwrap(), Value::number(3.0));
    let keys = a.own_property_keys(o).unwrap();
    assert_eq!(keys, [x, y]);
}

fn answer(_: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    Ok(Value::number(42.0))
}

#[test]
fn every_kind_of_reference_inside_an_object_is_traced() {
    let mut a = agent();
    let root = a.new_object().unwrap();
    // Accessor (getter object), symbol key with a description, BigInt,
    // wrapper primitive, sparse element and dictionary entry.
    let getter = a.new_native_function(answer, "get", 0).unwrap();
    let computed = key(&mut a, "computed");
    a.define_property_or_throw(
        root,
        computed,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
    )
    .unwrap();
    let description = a.heap.alloc_str("tag").unwrap();
    let symbol = a.heap.alloc_symbol(Some(description)).unwrap();
    let big = a
        .heap
        .alloc_bigint(BigInt::from_digits("123456789012345678901234567890", 10, false).unwrap())
        .unwrap();
    a.create_data_property_or_throw(root, PropertyKey::Symbol(symbol), Value::bigint(big))
        .unwrap();
    let text = a.string("wrapped").unwrap();
    let wrapper = a.to_object(text).unwrap();
    let array = a.new_array().unwrap();
    a.create_data_property_or_throw(array, PropertyKey::Index(1_000_000), Value::object(wrapper))
        .unwrap();
    let w = key(&mut a, "w");
    a.create_data_property_or_throw(root, w, Value::object(array))
        .unwrap();
    let dictionary = a.new_object().unwrap();
    for i in 0..40 {
        let k = key(&mut a, &format!("d{i}"));
        let s = a.string(&format!("value {i}")).unwrap();
        a.create_data_property_or_throw(dictionary, k, s).unwrap();
    }
    let d = key(&mut a, "d");
    a.create_data_property_or_throw(root, d, Value::object(dictionary))
        .unwrap();
    for _ in 0..100 {
        a.string("garbage").unwrap();
        a.heap.alloc_symbol(None).unwrap();
    }

    let stats = a.collect_garbage(holding(&[Value::object(root)]));
    assert_eq!(stats.objects_freed, 0);
    assert_eq!(stats.strings_freed, 100);
    assert_eq!(stats.symbols_freed, 100);
    assert_eq!(stats.bigints_freed, 0);
    for _ in 0..200 {
        a.string("reuse").unwrap();
        a.heap.alloc_symbol(None).unwrap();
        a.new_object().unwrap();
    }

    assert_eq!(a.get(root, computed).unwrap(), Value::number(42.0));
    let stored = a.get(root, PropertyKey::Symbol(symbol)).unwrap();
    assert_eq!(a.display(stored), "123456789012345678901234567890n");
    let description = a.heap.symbol(symbol).description.unwrap();
    assert_eq!(a.heap.string(description).to_text(), "tag");
    let element = a.get(array, PropertyKey::Index(1_000_000)).unwrap();
    let Kind::Object(found) = element.kind() else {
        panic!("element lost")
    };
    assert_eq!(found, wrapper);
    let unwrapped = a.to_string(Value::object(found)).unwrap();
    assert_eq!(a.heap.string(unwrapped).to_text(), "wrapped");
    for i in [0, 17, 39] {
        let k = key(&mut a, &format!("d{i}"));
        let v = a.get(dictionary, k).unwrap();
        assert_eq!(a.display(v), format!("value {i}"));
    }
}

#[test]
fn host_roots_keep_values_alive_until_released() {
    let mut a = agent();
    let o = a.new_object().unwrap();
    let root = a.root(Value::object(o)).unwrap();
    let objects = a.heap.object_count();
    a.collect_garbage(Roots::default());
    assert_eq!(a.heap.object_count(), objects);
    assert_eq!(a.rooted(root), Some(Value::object(o)));
    assert!(a.unroot(root));
    assert!(!a.unroot(root));
    assert_eq!(a.rooted(root), None);
    let stats = a.collect_garbage(Roots::default());
    assert_eq!(stats.objects_freed, 1);
    // The slot is reused, but the old handle never reads the new value.
    let other = a.root(Value::TRUE).unwrap();
    assert_ne!(other, root);
    assert_eq!(a.rooted(root), None);
    assert_eq!(a.rooted(other), Some(Value::TRUE));
}

#[test]
fn the_policy_asks_for_a_collection_after_enough_growth() {
    let mut a = agent();
    baseline(&mut a);
    assert!(!a.should_collect());
    let mut held = Vec::new();
    while !a.should_collect() {
        held.push(Value::object(a.new_object().unwrap()));
    }
    let live = a.heap.bytes_used();
    assert!(live >= MIN_GC_THRESHOLD);
    a.collect_garbage(holding(&held));
    assert!(!a.should_collect());
    // Everything was live, so the next trigger is about double.
    let mut more = 0usize;
    while !a.should_collect() {
        a.new_object().unwrap();
        more += 1;
    }
    let per_object = (a.heap.bytes_used() - live) / more;
    assert!(per_object > 0);
    assert!(a.heap.bytes_used() >= 2 * live - per_object);
}

#[test]
fn a_full_heap_recovers_after_collection() {
    let mut a = Agent::with_heap_limit(2 * MIN_GC_THRESHOLD).unwrap();
    let mut count = 0;
    while a.new_object().is_ok() {
        count += 1;
    }
    assert!(count > 1000);
    a.collect_garbage(Roots::default());
    for _ in 0..count {
        a.new_object().expect("collected memory is reusable");
    }
}

#[test]
fn freed_tails_return_arena_memory() {
    let mut a = agent();
    let base = baseline(&mut a);
    let first: Vec<ObjectId> = (0..10).map(|_| a.new_object().unwrap()).collect();
    for _ in 0..100_000 {
        a.new_object().unwrap();
    }
    let held: Vec<Value> = first.iter().map(|&o| Value::object(o)).collect();
    a.collect_garbage(holding(&held));
    // New objects fill the lowest freed indices first.
    let fresh = a.new_object().unwrap();
    assert!(fresh.index() < first[9].index() + 2);
    assert!(a.heap.bytes_used() < base + 2000);
}

// ---- weak objects -------------------------------------------------------

fn is_type_error(agent: &Agent, result: JsResult<impl std::fmt::Debug>) -> bool {
    match result {
        Err(Abrupt::Throw(v)) => {
            agent.stored_prototype(v.as_object().unwrap())
                == Some(agent.intrinsics.type_error_prototype)
        }
        _ => false,
    }
}

#[test]
fn weak_refs_do_not_keep_their_target_and_clear_when_it_dies() {
    let mut a = agent();
    let target = a.new_object().unwrap();
    let weak = a.new_weak_ref(Value::object(target)).unwrap();
    let symbol = a.heap.alloc_symbol(None).unwrap();
    let weak_symbol = a.new_weak_ref(Value::symbol(symbol)).unwrap();
    let r = a.new_weak_ref(Value::number(1.0));
    assert!(is_type_error(&a, r));
    let holders = [Value::object(weak), Value::object(weak_symbol)];

    // While something else holds the target, the reference reads it.
    let mut all = holders.to_vec();
    all.extend([Value::object(target), Value::symbol(symbol)]);
    a.collect_garbage(holding(&all));
    assert_eq!(a.weak_ref_deref(weak), Value::object(target));
    assert_eq!(a.weak_ref_deref(weak_symbol), Value::symbol(symbol));

    let stats = a.collect_garbage(holding(&holders));
    assert_eq!((stats.objects_freed, stats.symbols_freed), (1, 1));
    assert_eq!(a.weak_ref_deref(weak), Value::UNDEFINED);
    assert_eq!(a.weak_ref_deref(weak_symbol), Value::UNDEFINED);
    // The freed index is reused; the cleared reference must not see it.
    let reused = a.new_object().unwrap();
    assert_eq!(reused, target);
    assert_eq!(a.weak_ref_deref(weak), Value::UNDEFINED);
    assert_eq!(a.weak_ref_deref(reused), Value::UNDEFINED);
}

#[test]
fn weak_map_entries_live_exactly_as_long_as_their_keys() {
    let mut a = agent();
    let map = a.new_weak_map().unwrap();
    let set = a.new_weak_set().unwrap();
    let kept = a.new_object().unwrap();
    let dropped = a.new_object().unwrap();
    let kept_value = a.new_object().unwrap();
    let dropped_value = a.new_object().unwrap();
    a.weak_map_set(map, Value::object(kept), Value::object(kept_value))
        .unwrap();
    a.weak_map_set(map, Value::object(dropped), Value::object(dropped_value))
        .unwrap();
    a.weak_map_set(set, Value::object(dropped), Value::UNDEFINED)
        .unwrap();
    let r = a.weak_map_set(map, Value::TRUE, Value::NULL);
    assert!(is_type_error(&a, r));
    let r = a.weak_map_set(kept, Value::object(kept), Value::NULL);
    assert!(is_type_error(&a, r));
    assert!(a.weak_map_has(set, Value::object(dropped)));
    // Replacing a value does not add an entry.
    a.weak_map_set(map, Value::object(kept), Value::object(kept_value))
        .unwrap();
    assert_eq!(a.weak_map_len(map), 2);

    let roots = [Value::object(map), Value::object(set), Value::object(kept)];
    let stats = a.collect_garbage(holding(&roots));
    // `dropped` and its value go (with the two thrown TypeErrors); the map
    // keeps `kept_value` alive.
    assert_eq!(stats.objects_freed, 4);
    assert_eq!((a.weak_map_len(map), a.weak_map_len(set)), (1, 0));
    assert_eq!(
        a.weak_map_get(map, Value::object(kept)),
        Value::object(kept_value)
    );
    assert!(!a.weak_map_has(map, Value::object(dropped)));
    assert!(a.weak_map_delete(map, Value::object(kept)));
    assert!(!a.weak_map_delete(map, Value::object(kept)));
    let stats = a.collect_garbage(holding(&roots));
    assert_eq!(stats.objects_freed, 1);
}

#[test]
fn ephemerons_do_not_leak_keys_reachable_only_from_their_own_values() {
    let mut a = agent();
    let base = baseline(&mut a);
    let map = a.new_weak_map().unwrap();
    let back = key(&mut a, "back");
    // value -> key cycles, and a chain where each value holds the next key.
    let head = a.new_object().unwrap();
    let mut previous = head;
    for _ in 0..500 {
        let k = a.new_object().unwrap();
        let v = a.new_object().unwrap();
        a.create_data_property_or_throw(v, back, Value::object(k))
            .unwrap();
        a.weak_map_set(map, Value::object(k), Value::object(v))
            .unwrap();
        let next = a.new_object().unwrap();
        let holder = a.new_object().unwrap();
        a.create_data_property_or_throw(holder, back, Value::object(next))
            .unwrap();
        a.weak_map_set(map, Value::object(previous), Value::object(holder))
            .unwrap();
        previous = next;
    }
    assert_eq!(a.weak_map_len(map), 1000);
    // Holding the head keeps the whole chain (found only through
    // ephemeron values), but none of the self-referential pairs.
    let stats = a.collect_garbage(holding(&[Value::object(map), Value::object(head)]));
    assert_eq!(stats.objects_freed, 1000);
    assert_eq!(a.weak_map_len(map), 500);
    let mut walk = head;
    for _ in 0..500 {
        let holder = a
            .weak_map_get(map, Value::object(walk))
            .as_object()
            .unwrap();
        walk = a.get(holder, back).unwrap().as_object().unwrap();
    }
    assert_eq!(walk, previous);
    // Without the head everything goes, although the map survives.
    let stats = a.collect_garbage(holding(&[Value::object(map)]));
    assert_eq!(stats.objects_freed, 1001);
    assert_eq!(a.weak_map_len(map), 0);
    a.collect_garbage(Roots::default());
    assert_eq!(a.heap.bytes_used(), base);
}

#[test]
fn finalization_registries_queue_held_values_of_dead_targets() {
    let mut a = agent();
    let registry = a.new_finalization_registry().unwrap();
    let alive = a.new_object().unwrap();
    let dying = a.new_object().unwrap();
    let cancelled = a.new_object().unwrap();
    let token = a.new_object().unwrap();
    let held = a.string("held for dying").unwrap();
    a.registry_register(registry, Value::object(alive), Value::number(1.0), None)
        .unwrap();
    a.registry_register(registry, Value::object(dying), held, None)
        .unwrap();
    a.registry_register(
        registry,
        Value::object(cancelled),
        Value::number(3.0),
        Some(Value::object(token)),
    )
    .unwrap();
    let r = a.registry_register(registry, Value::number(1.0), Value::NULL, None);
    assert!(is_type_error(&a, r));
    let r = a.registry_register(registry, Value::object(alive), Value::object(alive), None);
    assert!(is_type_error(&a, r));
    let r = a.registry_register(
        registry,
        Value::object(alive),
        Value::NULL,
        Some(Value::TRUE),
    );
    assert!(is_type_error(&a, r));
    assert!(
        a.registry_unregister(registry, Value::object(token))
            .unwrap()
    );
    assert!(
        !a.registry_unregister(registry, Value::object(token))
            .unwrap()
    );

    let roots = [Value::object(registry), Value::object(alive)];
    a.collect_garbage(holding(&roots));
    let finalized = a.registry_take_finalized(registry);
    assert_eq!(finalized.len(), 1);
    assert_eq!(a.display(finalized[0]), "held for dying");
    assert!(a.registry_take_finalized(registry).is_empty());
    // A queued held value survives further collections until taken.
    a.registry_register(registry, Value::object(alive), held, None)
        .unwrap();
    a.collect_garbage(holding(&[Value::object(registry)]));
    a.collect_garbage(holding(&[Value::object(registry)]));
    let finalized = a.registry_take_finalized(registry);
    assert_eq!(finalized.len(), 2);
    assert_eq!(finalized[0], Value::number(1.0));
    assert_eq!(a.display(finalized[1]), "held for dying");
}

// ---- host objects -------------------------------------------------------

/// A toy host: each host object `n` keeps the listed values alive, and
/// some values are rooted by the host itself.
#[derive(Default)]
struct Host {
    edges: HashMap<u64, Vec<Value>>,
    rooted: Vec<Value>,
    traced: Vec<u64>,
    finalized: HashSet<u64>,
}

impl HostTracer for Host {
    fn roots(&mut self, sink: &mut dyn FnMut(Value)) {
        for &value in &self.rooted {
            sink(value);
        }
    }

    fn trace(&mut self, data: u64, sink: &mut dyn FnMut(Value)) {
        self.traced.push(data);
        for &value in self.edges.get(&data).into_iter().flatten() {
            sink(value);
        }
    }

    fn finalize(&mut self, data: u64) {
        assert!(self.finalized.insert(data), "finalized twice");
    }
}

#[test]
fn host_objects_report_edges_and_learn_of_their_death() {
    let mut a = agent();
    let mut host = Host::default();
    // wrapper 1 -> listener -> wrapper 2 -> payload; wrapper 3 <-> wrapper 4
    // is an unreachable cycle through host edges; wrapper 5 is host-rooted.
    let wrappers: Vec<ObjectId> = (1..=5)
        .map(|n| a.new_host_object(None, n).unwrap())
        .collect();
    assert_eq!(a.host_data(wrappers[1]), Some(2));
    let plain = a.new_object().unwrap();
    assert_eq!(a.host_data(plain), None);
    let listener = a.new_object().unwrap();
    let target = key(&mut a, "target");
    a.create_data_property_or_throw(listener, target, Value::object(wrappers[1]))
        .unwrap();
    let payload = a.string("payload").unwrap();
    host.edges.insert(1, vec![Value::object(listener)]);
    host.edges.insert(2, vec![payload]);
    host.edges.insert(3, vec![Value::object(wrappers[3])]);
    host.edges.insert(4, vec![Value::object(wrappers[2])]);
    host.rooted.push(Value::object(wrappers[4]));

    let stats = a.collect_garbage_with(holding(&[Value::object(wrappers[0])]), &mut host);
    // `plain`, wrapper 3 and wrapper 4 go.
    assert_eq!(stats.objects_freed, 3);
    host.traced.sort_unstable();
    assert_eq!(host.traced, [1, 2, 5]);
    assert_eq!(host.finalized, HashSet::from([3, 4]));
    assert_eq!(a.display(payload), "payload");
    assert_eq!(a.get(listener, target).unwrap(), Value::object(wrappers[1]));
    // Host objects are ordinary objects too.
    a.create_data_property_or_throw(wrappers[0], target, Value::TRUE)
        .unwrap();
    assert_eq!(a.get(wrappers[0], target).unwrap(), Value::TRUE);

    // A plain collection treats host objects as leaves.
    host.traced.clear();
    let stats = a.collect_garbage(Roots::default());
    assert_eq!(stats.objects_freed, 4);
}

#[test]
fn weak_map_values_reached_through_host_edges_are_found() {
    // key <- host edge of a wrapper that is itself only an ephemeron value.
    let mut a = agent();
    let mut host = Host::default();
    let map = a.new_weak_map().unwrap();
    let first = a.new_object().unwrap();
    let wrapper = a.new_host_object(None, 7).unwrap();
    let second = a.new_object().unwrap();
    let last = a.string("found").unwrap();
    a.weak_map_set(map, Value::object(first), Value::object(wrapper))
        .unwrap();
    host.edges.insert(7, vec![Value::object(second)]);
    a.weak_map_set(map, Value::object(second), last).unwrap();
    let roots = [Value::object(map), Value::object(first)];
    let stats = a.collect_garbage_with(holding(&roots), &mut host);
    assert_eq!(stats.objects_freed, 0);
    assert_eq!(stats.strings_freed, 0);
    assert_eq!(
        a.display(a.weak_map_get(map, Value::object(second))),
        "found"
    );
}

#[test]
fn abandoned_map_iterators_stop_holding_tables_after_a_collection() {
    let mut a = agent();
    a.evaluate(
        "var m = new Map(); for (var i = 0; i < 1000; i++) m.set(i, i); \
         var it = m.keys(); it.next(); for (var i = 0; i < 990; i++) m.delete(i); it = null;",
    )
    .unwrap();
    // The open iterator kept every position until now.
    let held = a.collect_garbage(Roots::default()).bytes_after;
    // The collection found the iterator dead, so the next change
    // squeezes the holes out.
    a.evaluate("m.set('x', 1); m.delete('x'); m.size").unwrap();
    let compacted = a.collect_garbage(Roots::default()).bytes_after;
    assert!(
        compacted + 30_000 < held,
        "{compacted} bytes after compaction, {held} before"
    );
    let keys = a.evaluate("[...m.keys()].join()").unwrap();
    assert_eq!(a.display(keys), "990,991,992,993,994,995,996,997,998,999");
}

#[test]
fn iterator_helpers_keep_their_iterators_and_functions_alive() {
    let mut a = agent();
    // Only the helpers reference the arrays, the mapper and the wrapped
    // iterator; collections between steps must keep all of them.
    a.evaluate(
        "var h = [[{ v: 1 }], [{ v: 2 }]].values().flatMap(function (x) { return x.map(function (o) { return { v: o.v * 10 } }) }); \
         var w = Iterator.from({ i: 0, next: function () { return { value: this.i++, done: this.i > 1 } } }); \
         var c = Iterator.concat([{ v: 3 }]);",
    )
    .unwrap();
    a.collect_garbage(Roots::default());
    let first = a.evaluate("h.next().value.v").unwrap();
    a.collect_garbage(Roots::default());
    let rest = a
        .evaluate("[h.next().value.v, h.next().done, w.next().value, w.next().done, c.next().value.v].join()")
        .unwrap();
    assert_eq!(a.display(first), "10");
    assert_eq!(a.display(rest), "20,true,0,true,3");
}

#[test]
fn suspended_generators_keep_their_frames_alive() {
    let mut a = agent();
    // Only the suspended frames reference these objects: the registers,
    // the captured environment and `this`.
    a.evaluate(
        "var g = (function* (n) { var local = { v: n }; let captured = { v: n + 1 }; \
             var f = function () { return captured.v }; \
             var sent = yield local.v; yield [sent.v, f(), this.t.v].join(); return local.v })\
             .call({ t: { v: 9 } }, 4); \
         g.next();",
    )
    .unwrap();
    for _ in 0..3 {
        a.evaluate(
            "var junk = []; for (var i = 0; i < 2000; i++) junk.push({ i: i }); junk = null",
        )
        .unwrap();
        a.collect_garbage(Roots::default());
    }
    let second = a.evaluate("g.next({ v: 7 }).value").unwrap();
    assert_eq!(a.display(second), "7,5,9");
    a.collect_garbage(Roots::default());
    let last = a
        .evaluate("var r = g.next(); r.value + ':' + r.done")
        .unwrap();
    assert_eq!(a.display(last), "4:true");
}

#[test]
fn generator_bodies_collect_at_their_safe_points() {
    // Each step allocates far more than the heap holds in total; only
    // collection inside the generator body keeps it within the limit.
    let mut a = Agent::with_heap_limit(1024 * 1024).unwrap();
    let r = a
        .evaluate(
            "function* g() { for (var n = 0; n < 3; n++) { var t = 0; \
               for (var i = 0; i < 100000; i++) { var o = { i: i, s: 'x' + i }; t += o.i & 1 } yield t } } \
             var s = 0; for (var v of g()) s += v; \
             var it = g(); s += it.next().value; s",
        )
        .unwrap();
    assert_eq!(a.display(r), "200000");
    // A generator stepped from native code while a value is unrooted (the
    // left operand's primitive, mid-conversion) must not collect, even
    // when every safe point asks to.
    let mut a = agent();
    a.heap.set_gc_stress(true);
    a.evaluate(
        "function* h() { for (var i = 0; i < 500; i++) var junk = { i: i, s: 'abcdefgh' + i }; yield 1 } \
         var gen = h(); gen.valueOf = gen.next; \
         var left = { toString: function () { return 'leftside' + 'abc' } }; \
         var bound = { valueOf: h().next.bind(h()), toString: function () { return '!' } };",
    )
    .unwrap();
    let before = a.heap.collections();
    let r = a.evaluate("(left + gen) + (left + bound)").unwrap();
    assert_eq!(a.display(r), "leftsideabc[object Generator]leftsideabc!");
    // The generator body ran without collecting.
    assert!(a.heap.collections() - before < 10);
}

#[test]
fn awaiting_async_functions_and_queued_jobs_survive_collection() {
    let mut a = agent();
    // The suspended frame is reachable only through the pending promise's
    // reaction, which only the stored resolve function reaches.
    a.evaluate(
        "var out = ''; var release; \
         (async function () { var local = { v: 'kept' }; var x = await new Promise(r => { release = r }); \
             out = local.v + x.v })(); \
         Promise.resolve({ v: 'queued' }).then(o => { out += o.v });",
    )
    .unwrap();
    a.collect_garbage(Roots::default());
    a.run_jobs().unwrap();
    a.evaluate("release({ v: '!' })").unwrap();
    for _ in 0..2 {
        a.evaluate(
            "var junk = []; for (var i = 0; i < 2000; i++) junk.push({ i: i }); junk = null",
        )
        .unwrap();
        a.collect_garbage(Roots::default());
    }
    a.run_jobs().unwrap();
    let out = a.evaluate("out").unwrap();
    assert_eq!(a.display(out), "kept!");
}

#[test]
fn array_from_async_keeps_the_original_error_during_close() {
    let mut agent = agent();
    agent
        .evaluate(
            "var release, log = '';
         var neverUsed = { next() { return { done: false, value: 2 }; },
           return() { log += 'close'; return new Promise((_, reject) => { release = reject; }); }
         };
         Array.fromAsync({[Symbol.asyncIterator]() { return neverUsed; }}, () => {
           throw { text: 'original' };
         }).catch(error => { log += ':' + error.text; });",
        )
        .unwrap();
    agent.run_jobs().unwrap();
    let closing = agent.evaluate("log").unwrap();
    assert_eq!(agent.display(closing), "close");
    agent.collect_garbage(Roots::default());
    agent
        .evaluate("release({text: 'replacement'}); neverUsed = release = null;")
        .unwrap();
    agent.collect_garbage(Roots::default());
    agent.run_jobs().unwrap();
    let result = agent.evaluate("log").unwrap();
    assert_eq!(agent.display(result), "close:original");
}

#[test]
fn array_from_async_native_jobs_reclaim_completed_awaits() {
    let mut agent = Agent::with_heap_limit(2 * MIN_GC_THRESHOLD).unwrap();
    agent
        .evaluate("var result = Array.fromAsync({length: 30000});")
        .unwrap();
    let before = agent.heap.collections();
    agent.run_jobs().unwrap();
    assert!(agent.heap.collections() > before);
    let promise = agent.evaluate("result").unwrap().as_object().unwrap();
    assert_eq!(agent.promise_rejected(promise), Some(false));
    let array = agent.promise_result(promise).unwrap().as_object().unwrap();
    let length = key(&mut agent, "length");
    assert_eq!(agent.get(array, length), Ok(Value::number(30000.0)));
}

#[test]
fn async_generators_and_their_queues_survive_collection() {
    let mut a = agent();
    // The generator awaits a promise only `release` resolves; the
    // requests queued behind it hold the only references to their values
    // and promises.
    a.evaluate(
        "var out = ''; var release; \
         async function* g() { var local = { v: 'kept' }; var x = await new Promise(r => { release = r }); \
             var y = yield local.v + x.v; yield y.v } \
         var it = g(); it.next().then(r => { out += r.value }); \
         it.next({ v: '+sent' }).then(r => { out += r.value }); \
         it.next().then(r => { out += r.done }); it = null; \
         (async () => { for await (var v of [{ v: '|wrapped' }]) out += v.v })();",
    )
    .unwrap();
    a.collect_garbage(Roots::default());
    a.run_jobs().unwrap();
    a.evaluate("release({ v: '!' })").unwrap();
    for _ in 0..2 {
        a.evaluate(
            "var junk = []; for (var i = 0; i < 2000; i++) junk.push({ i: i }); junk = null",
        )
        .unwrap();
        a.collect_garbage(Roots::default());
    }
    a.heap.set_gc_stress(true);
    a.run_jobs().unwrap();
    a.heap.set_gc_stress(false);
    let out = a.evaluate("out").unwrap();
    assert_eq!(a.display(out), "|wrappedkept!+senttrue");
}

#[test]
fn regexps_and_their_iterators_survive_collection() {
    let mut a = agent();
    // The iterator holds the only references to its RegExp and string;
    // the RegExp holds its source and flags.
    a.evaluate(
        "var it = 'a1b22c333'.matchAll(new RegExp('\\\\d+', 'g')); \
         var re = new RegExp('(?<w>[a-z])' + 'x?', 'gi'); var first = it.next().value[0];",
    )
    .unwrap();
    for _ in 0..2 {
        a.evaluate("var junk = []; for (var i = 0; i < 2000; i++) junk.push(/x/g); junk = null")
            .unwrap();
        a.collect_garbage(Roots::default());
    }
    a.heap.set_gc_stress(true);
    let out = a
        .evaluate(
            "first + [...it].map(m => m[0]).join() + re.source + re.flags + \
             'XbY'.replace(re, '<$<w>>')",
        )
        .unwrap();
    a.heap.set_gc_stress(false);
    assert_eq!(a.display(out), "122,333(?<w>[a-z])x?gi<X><b><Y>");
}

#[test]
fn pending_wait_async_promises_and_their_buffers_survive_collection() {
    let mut a = agent();
    a.heap.set_gc_stress(true);
    a.evaluate(
        "var out = ''; (function () { var view = new Int32Array(new SharedArrayBuffer(8));
         Atomics.waitAsync(view, 0, 0).value.then(v => { out = v + view.length }); })();
         globalThis.wake = function () {};",
    )
    .expect("runs");
    a.collect_garbage(Roots::default());
    // Only the waiter list holds the promise and the buffer now.
    assert_eq!(a.expire_waiters(f64::INFINITY), Ok(1));
    a.run_jobs().expect("jobs run");
    let out = a.evaluate("out").expect("runs");
    assert_eq!(a.display(out), "timed-out2");
}
