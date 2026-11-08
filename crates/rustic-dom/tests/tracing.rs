//! DOC-09 prototype: roots, traced host edges and collection of detached trees.

use rustic_dom::{
    Document, DocumentLimits, DomError, HostHeap, HostHeapError, HostHeapLimits, NodeId,
    QualifiedName, Traced,
};

fn document() -> Document {
    Document::new(DocumentLimits::default()).unwrap()
}

fn element(document: &mut Document, name: &str) -> NodeId {
    document
        .create_element(QualifiedName::html(name).unwrap())
        .unwrap()
}

fn heap(document: &Document) -> HostHeap {
    HostHeap::new(document, HostHeapLimits::default())
}

/// Build a detached `div > p > span` tree with a text leaf and return all IDs.
fn detached_tree(doc: &mut Document) -> [NodeId; 4] {
    let div = element(doc, "div");
    let p = element(doc, "p");
    let span = element(doc, "span");
    let text = doc.create_text("kept").unwrap();
    doc.append_child(div, p).unwrap();
    doc.append_child(p, span).unwrap();
    doc.append_child(span, text).unwrap();
    [div, p, span, text]
}

#[test]
fn reachable_detached_tree_survives_and_is_collected_once_unrooted() {
    let mut doc = document();
    let body = element(&mut doc, "body");
    doc.append_child(doc.root(), body).unwrap();
    let tree = detached_tree(&mut doc);
    let mut heap = heap(&doc);
    // A handle to the innermost node keeps its whole detached tree alive.
    heap.root(&doc, Traced::Node(tree[3])).unwrap();
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!(stats.nodes_destroyed, 0);
    for id in tree {
        assert!(doc.node(id).is_ok());
    }
    doc.check_invariants().unwrap();

    heap.unroot(&doc, Traced::Node(tree[3])).unwrap();
    assert_eq!(heap.edge_count(), 0);
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!(stats.nodes_destroyed, 4);
    for id in tree {
        assert_eq!(doc.node(id).unwrap_err(), DomError::InvalidNode);
    }
    // The connected tree is always a root.
    assert!(doc.node(body).is_ok());
    assert_eq!(doc.node_count(), 2);
    doc.check_invariants().unwrap();
}

#[test]
fn unreachable_node_object_cycles_disappear_and_rooted_ones_survive() {
    let mut doc = document();
    let mut heap = heap(&doc);
    let tree = detached_tree(&mut doc);
    // Wrapper identity: node <-> wrapper. Listener: node -> closure -> node.
    let wrapper = heap.allocate().unwrap();
    let listener = heap.allocate().unwrap();
    let closure = heap.allocate().unwrap();
    heap.add_edge(&doc, Traced::Node(tree[0]), Traced::Object(wrapper))
        .unwrap();
    heap.add_edge(&doc, Traced::Object(wrapper), Traced::Node(tree[0]))
        .unwrap();
    heap.add_edge(&doc, Traced::Node(tree[2]), Traced::Object(listener))
        .unwrap();
    heap.add_edge(&doc, Traced::Object(listener), Traced::Object(closure))
        .unwrap();
    heap.add_edge(&doc, Traced::Object(closure), Traced::Node(tree[1]))
        .unwrap();

    // Rooting the closure keeps the cycle and the tree.
    heap.root(&doc, Traced::Object(closure)).unwrap();
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!((stats.nodes_destroyed, stats.objects_freed), (0, 0));
    assert!(heap.contains(wrapper) && heap.contains(listener));
    assert!(doc.node(tree[0]).is_ok());

    heap.unroot(&doc, Traced::Object(closure)).unwrap();
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!(stats.nodes_destroyed, 4);
    assert_eq!(stats.objects_freed, 3);
    assert_eq!(stats.entries_pruned, 2, "two node-held edges");
    assert_eq!((heap.object_count(), heap.edge_count()), (0, 0));
    for object in [wrapper, listener, closure] {
        assert!(!heap.contains(object));
    }
    // A reused slot gets a new generation; the old identity stays dead.
    let reused = heap.allocate().unwrap();
    assert_ne!(reused, closure);
    assert!(!heap.contains(closure));
    assert_eq!(
        heap.root(&doc, Traced::Object(wrapper)).unwrap_err(),
        HostHeapError::InvalidObject
    );
    doc.check_invariants().unwrap();
}

#[test]
fn connected_nodes_keep_their_objects_and_what_those_objects_hold() {
    let mut doc = document();
    let html = element(&mut doc, "html");
    doc.append_child(doc.root(), html).unwrap();
    let tree = detached_tree(&mut doc);
    let orphan = element(&mut doc, "orphan");
    let mut heap = heap(&doc);
    // An event listener on a connected node captures a detached node.
    let listener = heap.allocate().unwrap();
    heap.add_edge(&doc, Traced::Node(html), Traced::Object(listener))
        .unwrap();
    heap.add_edge(&doc, Traced::Object(listener), Traced::Node(tree[2]))
        .unwrap();
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!(stats.nodes_destroyed, 1, "only the orphan");
    assert!(doc.node(orphan).is_err());
    assert!(tree.iter().all(|id| doc.node(*id).is_ok()));
    assert!(heap.contains(listener));

    // Removing the listener edge releases both.
    heap.remove_edge(&doc, Traced::Node(html), Traced::Object(listener))
        .unwrap();
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!((stats.nodes_destroyed, stats.objects_freed), (4, 1));
    // Detaching the connected element makes it collectable too.
    doc.detach(html).unwrap();
    assert_eq!(heap.collect(&mut doc).unwrap().nodes_destroyed, 1);
    assert_eq!(doc.node_count(), 1);
    doc.check_invariants().unwrap();
}

#[test]
fn template_and_contents_keep_each_other_alive() {
    let mut doc = document();
    let mut heap = heap(&doc);
    let wrapper = element(&mut doc, "div");
    let template = element(&mut doc, "template");
    doc.append_child(wrapper, template).unwrap();
    let contents = doc.create_template_contents(template).unwrap();
    let inner = element(&mut doc, "b");
    doc.append_child(contents, inner).unwrap();
    let nested = element(&mut doc, "template");
    doc.append_child(inner, nested).unwrap();
    let nested_contents = doc.create_template_contents(nested).unwrap();
    let deepest = doc.create_text("deep").unwrap();
    doc.append_child(nested_contents, deepest).unwrap();

    // Holding the deepest contents node keeps the outer template's tree.
    heap.root(&doc, Traced::Node(deepest)).unwrap();
    assert_eq!(heap.collect(&mut doc).unwrap().nodes_destroyed, 0);
    assert!(doc.node(wrapper).is_ok());
    doc.check_invariants().unwrap();

    // Holding the outer element keeps all contents.
    heap.unroot(&doc, Traced::Node(deepest)).unwrap();
    heap.root(&doc, Traced::Node(wrapper)).unwrap();
    assert_eq!(heap.collect(&mut doc).unwrap().nodes_destroyed, 0);
    assert!(doc.node(deepest).is_ok());

    heap.unroot(&doc, Traced::Node(wrapper)).unwrap();
    assert_eq!(heap.collect(&mut doc).unwrap().nodes_destroyed, 7);
    assert_eq!(doc.node_count(), 1);
    doc.check_invariants().unwrap();
}

#[test]
fn engine_destroyed_nodes_leave_no_dangling_roots_or_edges() {
    let mut doc = document();
    let mut heap = heap(&doc);
    let tree = detached_tree(&mut doc);
    let object = heap.allocate().unwrap();
    heap.root(&doc, Traced::Object(object)).unwrap();
    heap.root(&doc, Traced::Node(tree[1])).unwrap();
    heap.add_edge(&doc, Traced::Object(object), Traced::Node(tree[2]))
        .unwrap();
    heap.add_edge(&doc, Traced::Node(tree[3]), Traced::Object(object))
        .unwrap();
    assert_eq!(heap.edge_count(), 3);
    doc.destroy_subtree(tree[0]).unwrap();
    // The stale node can no longer be unrooted or linked.
    assert_eq!(
        heap.unroot(&doc, Traced::Node(tree[1])).unwrap_err(),
        HostHeapError::Node(DomError::InvalidNode)
    );
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!(stats.entries_pruned, 3);
    assert_eq!((stats.nodes_destroyed, stats.objects_freed), (0, 0));
    assert_eq!(heap.edge_count(), 0);
    assert!(heap.contains(object));
}

#[test]
fn invalid_edges_roots_and_foreign_handles_are_rejected_atomically() {
    let mut doc = document();
    let mut other = document();
    let mut heap = heap(&doc);
    let mut foreign_heap = HostHeap::new(&other, HostHeapLimits::default());
    let node = element(&mut doc, "a");
    let foreign_node = element(&mut other, "a");
    let object = heap.allocate().unwrap();
    let foreign_object = foreign_heap.allocate().unwrap();

    let cases = [
        (
            heap.add_edge(&doc, Traced::Node(node), Traced::Node(node)),
            HostHeapError::NodeToNodeEdge,
        ),
        (
            heap.add_edge(&doc, Traced::Object(object), Traced::Node(foreign_node)),
            HostHeapError::Node(DomError::WrongDocument),
        ),
        (
            heap.add_edge(&doc, Traced::Object(object), Traced::Object(foreign_object)),
            HostHeapError::WrongDocument,
        ),
        (
            heap.add_edge(&other, Traced::Node(foreign_node), Traced::Object(object)),
            HostHeapError::WrongDocument,
        ),
        (
            heap.remove_edge(&doc, Traced::Object(object), Traced::Node(node)),
            HostHeapError::MissingEdge,
        ),
        (
            heap.unroot(&doc, Traced::Object(object)),
            HostHeapError::NotRooted,
        ),
        (
            heap.unroot(&doc, Traced::Node(node)),
            HostHeapError::NotRooted,
        ),
    ];
    for (result, expected) in cases {
        assert_eq!(result.unwrap_err(), expected);
    }
    assert_eq!(heap.edge_count(), 0);
    assert_eq!(
        heap.collect(&mut other).unwrap_err(),
        HostHeapError::WrongDocument
    );
    assert!(
        other.node(foreign_node).is_ok(),
        "foreign document untouched"
    );
}

#[test]
fn object_and_edge_limits_are_exact_and_recover_after_collection() {
    let mut doc = document();
    let node = element(&mut doc, "a");
    doc.append_child(doc.root(), node).unwrap();
    let mut heap = HostHeap::new(
        &doc,
        HostHeapLimits {
            max_objects: 2,
            max_edges: 2,
        },
    );
    let first = heap.allocate().unwrap();
    let second = heap.allocate().unwrap();
    assert_eq!(heap.allocate().unwrap_err(), HostHeapError::ObjectLimit);
    heap.add_edge(&doc, Traced::Node(node), Traced::Object(first))
        .unwrap();
    heap.root(&doc, Traced::Node(node)).unwrap();
    // A second count on the same node is not another distinct root.
    heap.root(&doc, Traced::Node(node)).unwrap();
    assert_eq!(
        heap.add_edge(&doc, Traced::Object(first), Traced::Object(second))
            .unwrap_err(),
        HostHeapError::EdgeLimit
    );
    assert_eq!(heap.edge_count(), 2);
    assert_eq!(heap.collect(&mut doc).unwrap().objects_freed, 1);
    assert!(heap.contains(first) && !heap.contains(second));
    let third = heap.allocate().unwrap();
    heap.unroot(&doc, Traced::Node(node)).unwrap();
    heap.unroot(&doc, Traced::Node(node)).unwrap();
    heap.add_edge(&doc, Traced::Object(first), Traced::Object(third))
        .unwrap();
    assert_eq!(heap.edge_count(), 2);
}

#[test]
fn long_cycles_and_deep_trees_are_collected_without_recursion() {
    let mut doc = Document::new(DocumentLimits {
        max_nodes: 300_000,
        max_depth: 256,
        max_data_bytes: 1 << 20,
    })
    .unwrap();
    let mut heap = HostHeap::new(
        &doc,
        HostHeapLimits {
            max_objects: 200_000,
            max_edges: 400_000,
        },
    );
    // A 100,000-object ring threaded through a 256-deep detached chain.
    let mut chain = vec![element(&mut doc, "d")];
    for _ in 0..256 {
        let next = element(&mut doc, "d");
        doc.append_child(*chain.last().unwrap(), next).unwrap();
        chain.push(next);
    }
    let objects: Vec<_> = (0..100_000).map(|_| heap.allocate().unwrap()).collect();
    for pair in objects.windows(2) {
        heap.add_edge(&doc, Traced::Object(pair[0]), Traced::Object(pair[1]))
            .unwrap();
    }
    heap.add_edge(
        &doc,
        Traced::Object(*objects.last().unwrap()),
        Traced::Node(chain[256]),
    )
    .unwrap();
    heap.add_edge(&doc, Traced::Node(chain[0]), Traced::Object(objects[0]))
        .unwrap();
    heap.root(&doc, Traced::Object(objects[50_000])).unwrap();
    assert_eq!(heap.collect(&mut doc).unwrap().nodes_destroyed, 0);
    heap.unroot(&doc, Traced::Object(objects[50_000])).unwrap();
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!((stats.nodes_destroyed, stats.objects_freed), (257, 100_000));
    assert_eq!((doc.node_count(), heap.edge_count()), (1, 0));
    // Collecting an empty heap again is a no-op.
    assert_eq!(heap.collect(&mut doc).unwrap(), Default::default());
}

/// Random graphs compared against an independent reachability model.
#[test]
fn seeded_graphs_match_an_independent_reachability_model() {
    const NODES: usize = 24;
    const OBJECTS: usize = 24;
    let mut seed = 0xbb67ae8584caa73bu64;
    let mut random = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 33) as usize
    };
    let (mut survived, mut collected) = (0usize, 0usize);
    for round in 0..200 {
        let mut doc = document();
        let mut heap = heap(&doc);
        let nodes: Vec<_> = (0..NODES).map(|_| element(&mut doc, "n")).collect();
        let objects: Vec<_> = (0..OBJECTS).map(|_| heap.allocate().unwrap()).collect();
        // Model: tree parent per node (None = root/detached), edges, roots.
        let mut parent: Vec<Option<usize>> = vec![None; NODES];
        let mut connected = [false; NODES];
        for child in 1..NODES {
            match random() % 4 {
                0 => {}
                1 => {
                    doc.append_child(doc.root(), nodes[child]).ok();
                    if doc.node(nodes[child]).unwrap().parent() == Some(doc.root()) {
                        connected[child] = true;
                    }
                }
                _ => {
                    let target = random() % child;
                    doc.append_child(nodes[target], nodes[child]).unwrap();
                    parent[child] = Some(target);
                }
            }
        }
        let mut node_edges = Vec::new();
        let mut object_edges = Vec::new();
        for _ in 0..random() % 40 {
            let (node, object) = (random() % NODES, random() % OBJECTS);
            if random() % 2 == 0 {
                heap.add_edge(
                    &doc,
                    Traced::Node(nodes[node]),
                    Traced::Object(objects[object]),
                )
                .unwrap();
                node_edges.push((node, object));
            } else if random() % 2 == 0 {
                heap.add_edge(
                    &doc,
                    Traced::Object(objects[object]),
                    Traced::Node(nodes[node]),
                )
                .unwrap();
                object_edges.push((object, Traced::Node(nodes[node]), node));
            } else {
                let to = random() % OBJECTS;
                heap.add_edge(
                    &doc,
                    Traced::Object(objects[object]),
                    Traced::Object(objects[to]),
                )
                .unwrap();
                object_edges.push((object, Traced::Object(objects[to]), to));
            }
        }
        let rooted_node = (random() % 3 != 0).then(|| random() % NODES);
        let rooted_object = (random() % 3 != 0).then(|| random() % OBJECTS);
        if let Some(node) = rooted_node {
            heap.root(&doc, Traced::Node(nodes[node])).unwrap();
        }
        if let Some(object) = rooted_object {
            heap.root(&doc, Traced::Object(objects[object])).unwrap();
        }

        // Independent fixpoint: a node is live when any node of its tree is
        // reached; tree identity is the topmost ancestor in the model.
        let top = |mut node: usize| {
            while let Some(up) = parent[node] {
                node = up;
            }
            node
        };
        let mut live_tree = [false; NODES];
        let mut live_object = [false; OBJECTS];
        for node in 0..NODES {
            if connected[top(node)] {
                live_tree[top(node)] = true;
            }
        }
        if let Some(node) = rooted_node {
            live_tree[top(node)] = true;
        }
        if let Some(object) = rooted_object {
            live_object[object] = true;
        }
        loop {
            let mut changed = false;
            for &(node, object) in &node_edges {
                if live_tree[top(node)] && !live_object[object] {
                    live_object[object] = true;
                    changed = true;
                }
            }
            for &(object, to, index) in &object_edges {
                if !live_object[object] {
                    continue;
                }
                match to {
                    Traced::Node(_) if !live_tree[top(index)] => {
                        live_tree[top(index)] = true;
                        changed = true;
                    }
                    Traced::Object(_) if !live_object[index] => {
                        live_object[index] = true;
                        changed = true;
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }

        heap.collect(&mut doc).unwrap();
        doc.check_invariants().unwrap();
        survived += doc.node_count() - 1;
        collected += NODES - doc.node_count() + 1;
        for node in 0..NODES {
            assert_eq!(
                doc.node(nodes[node]).is_ok(),
                live_tree[top(node)],
                "round {round} node {node}"
            );
        }
        for object in 0..OBJECTS {
            assert_eq!(
                heap.contains(objects[object]),
                live_object[object],
                "round {round} object {object}"
            );
        }
        // A second collection finds nothing more.
        assert_eq!(heap.collect(&mut doc).unwrap(), Default::default());
    }
    // The seed exercises both outcomes heavily, not only one.
    assert!(survived > 500 && collected > 500, "{survived} {collected}");
}

#[test]
fn free_fragments_follow_the_same_rules_and_adoption_prunes_old_ids() {
    let mut doc = document();
    let mut heap = heap(&doc);
    let kept = doc.create_document_fragment().unwrap();
    let dropped = doc.create_document_fragment().unwrap();
    let child = element(&mut doc, "li");
    doc.append_child(kept, child).unwrap();
    let pi = doc.create_processing_instruction("x", "y").unwrap();
    doc.append_child(dropped, pi).unwrap();
    heap.root(&doc, Traced::Node(child)).unwrap();
    assert_eq!(heap.collect(&mut doc).unwrap().nodes_destroyed, 2);
    assert!(doc.node(kept).is_ok() && doc.node(pi).is_err());

    // Adoption copies into the target with new IDs and destroys the source
    // nodes, so the source heap prunes its entries. A host that wants identity
    // to survive must re-register edges using `Adopted::ids` (ADR 0012).
    let mut target = document();
    let adopted = target.adopt_node(&mut doc, kept).unwrap();
    assert_eq!(adopted.ids.len(), 2);
    let stats = heap.collect(&mut doc).unwrap();
    assert_eq!((stats.nodes_destroyed, stats.entries_pruned), (0, 1));
    doc.check_invariants().unwrap();
    target.check_invariants().unwrap();
}
