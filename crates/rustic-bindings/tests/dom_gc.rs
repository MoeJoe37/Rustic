//! The DOC-09 tracing scenarios against the real JavaScript heap (JS-03):
//! detached trees held by script, node/wrapper/listener cycles, connected
//! trees, template and shadow ownership, engine destruction, deep and long
//! graphs, a seeded reachability model and a GC stress run.

use std::collections::{HashMap, HashSet};

use rustic_bindings::{DomGcError, DomHeap};
use rustic_dom::{Document, DocumentLimits, NodeId, QualifiedName, ShadowRootMode};
use rustic_js::runtime::{Agent, ObjectId, PropertyKey, Roots, Value};

struct World {
    agent: Agent,
    doc: Document,
    dom: DomHeap,
}

impl World {
    fn new() -> Self {
        Self::with_limits(DocumentLimits::default())
    }

    fn with_limits(limits: DocumentLimits) -> Self {
        let doc = Document::new(limits).unwrap();
        let dom = DomHeap::new(&doc);
        Self {
            agent: Agent::new().unwrap(),
            doc,
            dom,
        }
    }

    fn element(&mut self, name: &str) -> NodeId {
        self.doc
            .create_element(QualifiedName::html(name).unwrap())
            .unwrap()
    }

    fn wrap(&mut self, node: NodeId) -> ObjectId {
        self.dom.wrap(&mut self.agent, &self.doc, node).unwrap()
    }

    /// Collect holding `held`; returns (nodes destroyed, JS objects freed).
    fn collect(&mut self, held: &[ObjectId]) -> (usize, usize) {
        let values: Vec<Value> = held.iter().map(|&o| Value::object(o)).collect();
        let stats = self
            .dom
            .collect(
                &mut self.agent,
                &mut self.doc,
                Roots {
                    values: &values,
                    envs: &[],
                    codes: &[],
                },
            )
            .unwrap();
        self.doc.check_invariants().unwrap();
        (stats.nodes_destroyed, stats.js.objects_freed)
    }

    fn key(&mut self, text: &str) -> PropertyKey {
        self.agent.heap.key(text).unwrap()
    }

    fn set(&mut self, object: ObjectId, name: &str, value: Value) {
        let key = self.key(name);
        self.agent
            .create_data_property_or_throw(object, key, value)
            .unwrap();
    }

    /// A detached `div > p > span > "kept"` tree.
    fn detached_tree(&mut self) -> [NodeId; 4] {
        let div = self.element("div");
        let p = self.element("p");
        let span = self.element("span");
        let text = self.doc.create_text("kept").unwrap();
        self.doc.append_child(div, p).unwrap();
        self.doc.append_child(p, span).unwrap();
        self.doc.append_child(span, text).unwrap();
        [div, p, span, text]
    }
}

#[test]
fn a_detached_tree_held_by_script_survives_and_goes_once_released() {
    let mut w = World::new();
    let body = w.element("body");
    let root = w.doc.root();
    w.doc.append_child(root, body).unwrap();
    let tree = w.detached_tree();
    // A wrapper of the innermost node keeps the whole detached tree.
    let text = w.wrap(tree[3]);
    assert_eq!(w.wrap(tree[3]), text, "wrapper identity");
    assert_eq!(w.collect(&[text]), (0, 0));
    assert!(tree.iter().all(|&n| w.doc.node(n).is_ok()));
    assert_eq!(w.dom.node_of(&w.agent, &w.doc, text), Some(tree[3]));

    let (nodes, objects) = w.collect(&[]);
    assert_eq!((nodes, objects), (4, 1));
    assert!(tree.iter().all(|&n| w.doc.node(n).is_err()));
    assert_eq!(w.dom.wrapper_count(), 0);
    // The connected tree is always live.
    assert!(w.doc.node(body).is_ok());
    assert_eq!(w.doc.node_count(), 2);
}

#[test]
fn node_wrapper_listener_cycles_are_collected_unless_rooted() {
    let mut w = World::new();
    let tree = w.detached_tree();
    // node <-> wrapper, and node -> listener -> closure -> wrapper(p).
    let div = w.wrap(tree[0]);
    let listener = w.agent.new_object().unwrap();
    let closure = w.agent.new_object().unwrap();
    let p = w.wrap(tree[1]);
    w.set(listener, "callback", Value::object(closure));
    w.set(closure, "captured", Value::object(p));
    w.dom
        .add_edge(&w.doc, tree[2], Value::object(listener))
        .unwrap();
    w.set(div, "expando", Value::TRUE);

    // Holding only the closure keeps the cycle and the tree.
    assert_eq!(w.collect(&[closure]), (0, 0));
    let expando = w.key("expando");
    assert_eq!(w.agent.get(div, expando).unwrap(), Value::TRUE);

    let objects_before = w.agent.heap.object_count();
    let (nodes, objects) = w.collect(&[]);
    assert_eq!(nodes, 4);
    assert_eq!(objects, 4, "two wrappers, the listener and the closure");
    assert_eq!(w.agent.heap.object_count(), objects_before - 4);
    assert_eq!((w.dom.wrapper_count(), w.dom.edge_count()), (0, 0));
    // A freed wrapper's index may be reused; it never names the old node.
    let fresh = w.element("i");
    let reused = w.wrap(fresh);
    assert_eq!(w.dom.node_of(&w.agent, &w.doc, reused), Some(fresh));
}

#[test]
fn connected_nodes_keep_their_listeners_and_what_those_hold() {
    let mut w = World::new();
    let html = w.element("html");
    let root = w.doc.root();
    w.doc.append_child(root, html).unwrap();
    let tree = w.detached_tree();
    let orphan = w.element("orphan");
    // A listener on a connected node captures a detached node's wrapper.
    let listener = w.agent.new_object().unwrap();
    let span = w.wrap(tree[2]);
    w.set(listener, "captured", Value::object(span));
    w.dom
        .add_edge(&w.doc, html, Value::object(listener))
        .unwrap();
    let html_wrapper = w.wrap(html);

    assert_eq!(w.collect(&[]), (1, 0), "only the orphan");
    assert!(w.doc.node(orphan).is_err());
    assert!(tree.iter().all(|&n| w.doc.node(n).is_ok()));
    // Connected wrappers stay too, with their expandos.
    assert_eq!(w.dom.existing_wrapper(html), Some(html_wrapper));

    assert!(w.dom.remove_edge(html, Value::object(listener)));
    assert!(!w.dom.remove_edge(html, Value::object(listener)));
    assert_eq!(
        w.collect(&[]),
        (4, 2),
        "the tree, its wrapper and the listener"
    );
    // Detaching the connected element makes it collectable too.
    w.doc.detach(html).unwrap();
    assert_eq!(w.collect(&[]), (1, 1));
    assert_eq!(w.doc.node_count(), 1);
}

#[test]
fn templates_and_shadow_roots_live_with_their_owners() {
    let mut w = World::new();
    let outer = w.element("div");
    let template = w.element("template");
    w.doc.append_child(outer, template).unwrap();
    let contents = w.doc.create_template_contents(template).unwrap();
    let inner = w.element("b");
    w.doc.append_child(contents, inner).unwrap();
    let nested = w.element("template");
    w.doc.append_child(inner, nested).unwrap();
    let nested_contents = w.doc.create_template_contents(nested).unwrap();
    let deepest = w.doc.create_text("deep").unwrap();
    w.doc.append_child(nested_contents, deepest).unwrap();
    let host = w.element("section");
    w.doc.append_child(outer, host).unwrap();
    let shadow = w.doc.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let in_shadow = w.element("span");
    w.doc.append_child(shadow, in_shadow).unwrap();

    let deepest_wrapper = w.wrap(deepest);
    assert_eq!(w.collect(&[deepest_wrapper]), (0, 0));
    assert!(w.doc.node(outer).is_ok() && w.doc.node(in_shadow).is_ok());
    // A live tree keeps every wrapper of its nodes, so nothing is freed
    // while any one of them is held.
    let shadow_wrapper = w.wrap(in_shadow);
    assert_eq!(w.collect(&[shadow_wrapper]), (0, 0));
    assert!(w.doc.node(deepest).is_ok());
    let outer_wrapper = w.wrap(outer);
    assert_eq!(w.collect(&[outer_wrapper]), (0, 0));
    assert_eq!(w.collect(&[]), (10, 3));
    assert_eq!(w.doc.node_count(), 1);
}

#[test]
fn engine_destroyed_nodes_leave_no_dangling_edges_or_wrappers() {
    let mut w = World::new();
    let tree = w.detached_tree();
    let listener = w.agent.new_object().unwrap();
    w.dom
        .add_edge(&w.doc, tree[3], Value::object(listener))
        .unwrap();
    let wrapper = w.wrap(tree[1]);
    w.doc.destroy_subtree(tree[0]).unwrap();
    // The engine destroyed the tree; script still holds the wrapper.
    let values = [Value::object(wrapper)];
    let stats = w
        .dom
        .collect(
            &mut w.agent,
            &mut w.doc,
            Roots {
                values: &values,
                envs: &[],
                codes: &[],
            },
        )
        .unwrap();
    assert_eq!(stats.edges_dropped, 1);
    assert_eq!(stats.nodes_destroyed, 0);
    assert_eq!(stats.js.objects_freed, 1, "the listener");
    assert_eq!(w.dom.node_of(&w.agent, &w.doc, wrapper), None);
    assert_eq!((w.dom.wrapper_count(), w.dom.edge_count()), (0, 0));
    // The wrapper is still an ordinary object script can use.
    w.set(wrapper, "x", Value::NULL);
}

#[test]
fn foreign_documents_and_stale_nodes_are_rejected() {
    let mut w = World::new();
    let mut other = Document::new(DocumentLimits::default()).unwrap();
    let foreign = other
        .create_element(QualifiedName::html("a").unwrap())
        .unwrap();
    assert!(matches!(
        w.dom.wrap(&mut w.agent, &w.doc, foreign),
        Err(DomGcError::Dom(_))
    ));
    assert_eq!(
        w.dom.wrap(&mut w.agent, &other, foreign),
        Err(DomGcError::WrongDocument)
    );
    assert_eq!(
        w.dom.collect(&mut w.agent, &mut other, Roots::default()),
        Err(DomGcError::WrongDocument)
    );
    assert!(other.node(foreign).is_ok(), "foreign document untouched");
    let gone = w.element("a");
    w.doc.destroy_subtree(gone).unwrap();
    assert!(w.dom.wrap(&mut w.agent, &w.doc, gone).is_err());
    assert!(w.dom.add_edge(&w.doc, gone, Value::TRUE).is_err());
    // A plain object is not a wrapper.
    let plain = w.agent.new_object().unwrap();
    assert_eq!(w.dom.node_of(&w.agent, &w.doc, plain), None);
}

#[test]
fn long_cycles_through_deep_trees_are_collected_without_recursion() {
    let mut w = World::with_limits(DocumentLimits {
        max_nodes: 300_000,
        max_depth: 256,
        max_data_bytes: 1 << 20,
    });
    // A 100,000-object ring threaded through a 256-deep detached chain:
    // chain top -> ring[0] -> ... -> ring[last] -> wrapper(chain bottom).
    let mut chain = vec![w.element("d")];
    for _ in 0..256 {
        let next = w.element("d");
        w.doc.append_child(*chain.last().unwrap(), next).unwrap();
        chain.push(next);
    }
    let ring: Vec<ObjectId> = (0..100_000)
        .map(|_| w.agent.new_object().unwrap())
        .collect();
    let next = w.key("next");
    for pair in ring.windows(2) {
        w.agent
            .create_data_property_or_throw(pair[0], next, Value::object(pair[1]))
            .unwrap();
    }
    let bottom = w.wrap(chain[256]);
    w.agent
        .create_data_property_or_throw(*ring.last().unwrap(), next, Value::object(bottom))
        .unwrap();
    w.dom
        .add_edge(&w.doc, chain[0], Value::object(ring[0]))
        .unwrap();
    assert_eq!(w.collect(&[ring[50_000]]), (0, 0));
    assert_eq!(w.collect(&[]), (257, 100_001));
    assert_eq!(w.doc.node_count(), 1);
    assert_eq!(w.collect(&[]), (0, 0));
}

#[test]
fn weak_maps_keyed_by_wrappers_drop_entries_with_their_trees() {
    let mut w = World::new();
    let map = w.agent.new_weak_map().unwrap();
    let kept = w.element("kept");
    let root = w.doc.root();
    w.doc.append_child(root, kept).unwrap();
    let dropped = w.element("dropped");
    let kept_wrapper = w.wrap(kept);
    let dropped_wrapper = w.wrap(dropped);
    w.agent
        .weak_map_set(map, Value::object(kept_wrapper), Value::number(1.0))
        .unwrap();
    w.agent
        .weak_map_set(map, Value::object(dropped_wrapper), Value::number(2.0))
        .unwrap();
    assert_eq!(w.collect(&[map]), (1, 1));
    assert_eq!(w.agent.weak_map_len(map), 1);
    assert_eq!(
        w.agent.weak_map_get(map, Value::object(kept_wrapper)),
        Value::number(1.0)
    );
}

/// Tiny deterministic generator for the seeded model.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// Random trees, wrappers, listeners and object graphs; after each
/// collection the surviving nodes and objects must equal an independent
/// reachability computation.
#[test]
fn seeded_graphs_match_an_independent_reachability_model() {
    for seed in 0..40u64 {
        let mut w = World::new();
        let mut rng = Lcg(seed);
        let link = w.key("link");
        let mut nodes: Vec<NodeId> = Vec::new();
        let mut objects: Vec<ObjectId> = Vec::new();
        // Model edges.
        let mut object_edges: HashMap<ObjectId, Vec<Value>> = HashMap::new();
        let mut node_edges: HashMap<NodeId, Vec<ObjectId>> = HashMap::new();
        for _ in 0..60 {
            match rng.below(6) {
                0 | 1 => {
                    let n = w.element("x");
                    if !nodes.is_empty() && rng.below(2) == 0 {
                        let parent = nodes[rng.below(nodes.len())];
                        w.doc.append_child(parent, n).unwrap();
                    } else if rng.below(4) == 0 {
                        let root = w.doc.root();
                        if w.doc.node(root).unwrap().first_child().is_none() {
                            w.doc.append_child(root, n).unwrap();
                        }
                    }
                    nodes.push(n);
                }
                2 => objects.push(w.agent.new_object().unwrap()),
                3 if !nodes.is_empty() => {
                    let wrapper = w.wrap(nodes[rng.below(nodes.len())]);
                    if !objects.contains(&wrapper) {
                        objects.push(wrapper);
                    }
                }
                4 if !objects.is_empty() => {
                    let from = objects[rng.below(objects.len())];
                    let to = objects[rng.below(objects.len())];
                    w.agent
                        .create_data_property_or_throw(from, link, Value::object(to))
                        .unwrap();
                    // A property is overwritten, so the model keeps one.
                    object_edges.insert(from, vec![Value::object(to)]);
                }
                5 if !nodes.is_empty() && !objects.is_empty() => {
                    let node = nodes[rng.below(nodes.len())];
                    let object = objects[rng.below(objects.len())];
                    w.dom.add_edge(&w.doc, node, Value::object(object)).unwrap();
                    node_edges.entry(node).or_default().push(object);
                }
                _ => {}
            }
        }
        let held: Vec<ObjectId> = (0..3)
            .filter(|_| !objects.is_empty())
            .map(|_| objects[rng.below(objects.len())])
            .collect();

        // Independent model: trees by root, wrappers by node.
        let tree_of = |doc: &Document, mut n: NodeId| {
            while let Some(p) = doc.node(n).unwrap().parent() {
                n = p;
            }
            n
        };
        let wrapper_node: HashMap<ObjectId, NodeId> = nodes
            .iter()
            .filter_map(|&n| w.dom.existing_wrapper(n).map(|o| (o, n)))
            .collect();
        let mut live_trees: HashSet<NodeId> = HashSet::new();
        let mut live_objects: HashSet<ObjectId> = HashSet::new();
        let mut work: Vec<Result<NodeId, ObjectId>> = held.iter().map(|&o| Err(o)).collect();
        work.push(Ok(w.doc.root()));
        while let Some(item) = work.pop() {
            match item {
                Ok(node) => {
                    let tree = tree_of(&w.doc, node);
                    if !live_trees.insert(tree) {
                        continue;
                    }
                    for &n in nodes.iter().chain([&w.doc.root()]) {
                        if tree_of(&w.doc, n) == tree {
                            if let Some(o) = w.dom.existing_wrapper(n) {
                                work.push(Err(o));
                            }
                            for &o in node_edges.get(&n).into_iter().flatten() {
                                work.push(Err(o));
                            }
                        }
                    }
                }
                Err(object) => {
                    if !live_objects.insert(object) {
                        continue;
                    }
                    if let Some(&n) = wrapper_node.get(&object) {
                        work.push(Ok(n));
                    }
                    for v in object_edges.get(&object).into_iter().flatten() {
                        work.push(Err(v.as_object().unwrap()));
                    }
                }
            }
        }
        let expected_nodes: HashSet<NodeId> = nodes
            .iter()
            .copied()
            .filter(|&n| live_trees.contains(&tree_of(&w.doc, n)))
            .collect();
        let objects_before = w.agent.heap.object_count();
        let dead_objects = objects.iter().filter(|o| !live_objects.contains(o)).count();

        w.collect(&held);
        for &n in &nodes {
            assert_eq!(
                w.doc.node(n).is_ok(),
                expected_nodes.contains(&n),
                "seed {seed}: node liveness"
            );
        }
        assert_eq!(
            w.agent.heap.object_count(),
            objects_before - dead_objects,
            "seed {seed}: object count"
        );
        for &o in &live_objects {
            if let Some(&n) = wrapper_node.get(&o) {
                assert_eq!(w.dom.node_of(&w.agent, &w.doc, o), Some(n));
            }
            if let Some(v) = object_edges.get(&o) {
                assert_eq!(w.agent.get(o, link).unwrap(), v[0], "seed {seed}");
            }
        }
    }
}

#[test]
fn stress_mode_collects_at_every_safe_point_without_losing_held_values() {
    let mut w = World::new();
    w.agent.heap.set_gc_stress(true);
    let list = w.element("ul");
    let root = w.doc.root();
    w.doc.append_child(root, list).unwrap();
    let mut held: Vec<ObjectId> = Vec::new();
    // The agent collected once while it was created.
    let mut collections = w.agent.heap.collections();
    for i in 0..200 {
        // One "instruction": create an item, wrap it, give it a listener
        // that remembers the previous wrapper; only every tenth item is
        // attached, the rest are held by script or dropped.
        let item = w.element("li");
        if i % 10 == 0 {
            w.doc.append_child(list, item).unwrap();
        }
        let wrapper = w.wrap(item);
        let listener = w.agent.new_object().unwrap();
        if let Some(&previous) = held.last() {
            w.set(listener, "previous", Value::object(previous));
        }
        w.dom
            .add_edge(&w.doc, item, Value::object(listener))
            .unwrap();
        if i % 3 == 0 {
            held.push(wrapper);
        }
        // The safe point.
        assert!(w.agent.should_collect());
        w.collect(&held);
        collections += 1;
        for &h in &held {
            let node = w
                .dom
                .node_of(&w.agent, &w.doc, h)
                .expect("held wrapper keeps its node");
            assert!(w.doc.node(node).is_ok());
        }
    }
    assert_eq!(w.agent.heap.collections(), collections);
    // Attached items, held items and items whose listener chain is
    // reachable survive; the rest were collected along the way.
    let attached = 20;
    assert!(w.doc.node_count() >= 2 + attached);
    assert!(w.doc.node_count() < 2 + 200);
}
