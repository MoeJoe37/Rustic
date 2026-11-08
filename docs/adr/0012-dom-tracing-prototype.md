# ADR 0012: DOM/host tracing prototype and DOM conformance gate

- Date: 2026-11-08
- Status: Accepted prototype contract (DOC-09); the real heap arrives with JS-03

## Context

ADR 0002 requires a nonmoving traced JS/host graph and asks for a prototype of
detached-node retention and listener/wrapper cycles before bindings. Until now
detached nodes lived until explicit engine destruction, which cannot model
script: a detached subtree held by a variable must survive, and a
node ↔ wrapper ↔ listener cycle with no outside reference must disappear.
There is no JavaScript runtime yet, so the DOM side of the contract needs a
stand-in host heap to be tested against.

## Decision

`rustic-dom` adds `HostHeap`, a stop-the-world, nonmoving mark/sweep heap bound
to one `Document`:

- **Identities.** `HostObjectId` carries the document, slot and generation like
  `NodeId`. Freed slots get a new generation and retire at exhaustion, so a
  stale or forged identity never names a live object.
- **Reachability unit.** Holding any node keeps its whole tree alive, because
  parent/sibling/child links are script-visible. A template and its contents
  keep each other alive. The document's own tree is always a root.
- **Roots.** Counted node roots and counted object roots model host handles.
  Each distinct rooted node is charged against the edge budget.
- **Edges.** Node → object (wrapper, expando, listener), object → node and
  object → object edges are registered explicitly. Node → node edges belong to
  the tree and are rejected.
- **Collection.** Marking walks whole trees with the existing stack-free walk
  and uses a worklist whose capacity is reserved up front from
  roots + edges + 2 × nodes. All temporary storage is reserved before anything
  changes. Unreachable parentless trees are destroyed with
  `Document::destroy_subtree`, so their IDs go stale; unreachable objects are
  freed. Roots and edges naming nodes the engine destroyed explicitly are pruned.
- **Limits.** `HostHeapLimits` bounds objects and edges + rooted nodes.

The gate half of DOC-09 adds `Document::check_invariants`, a linear whole-
document structural checker (link symmetry, acyclic parents, depth limit,
child kinds, document child order, template ownership and accounting), and a
conformance test that enumerates every (parent, node, reference) combination
over a fixture holding each node kind, connected and detached trees, template
contents and free fragments, against an oracle transcribed from the DOM Standard's
"ensure pre-insert validity" and "pre-insert" steps. It checks the exception
class and step precedence, resulting child order, unchanged trees after a
rejection, and invariants after every call. On first run it found four
`insert_before` gaps: host-including ancestors through template contents, step
2/3 precedence, the document node rejected before steps 2 and 3, and
re-insertion of the existing document element or doctype accepted as a reorder.
All four are fixed (the first two in DOC-04c's insertion rework).

## Alternatives and consequences

Reference counting with weak parent links cannot collect node ↔ object cycles.
Tracing per node rather than per tree would need parent/sibling edges in every
host object and gives the same answer at a higher cost. Putting the heap behind
a trait now would invent an interface with one implementation; the concrete
prototype is the smallest thing that tests the contract.

The prototype heap lives in the DOM crate only because no runtime exists yet.
ADR 0002 says the runtime traces registered host objects without depending on
the DOM crate, so JS-03 will own the object heap, and the DOM will keep the
tree-closure marking and the sweep through `destroy_subtree`. The heap is bound
to one document; realms spanning documents need a shared marking phase across
documents, decided with JS-03 and WEB-02. Adoption reissues node IDs (TD-16), so
the prototype prunes the old IDs' entries; a real host must retarget its edges
through `Adopted::ids` in the same operation (TD-17).
Collection is stop-the-world and linear in slots, edges and roots; incremental
or generational collection needs barriers and measured pause evidence first.
The edge registry is a flat vector sorted per collection; it is not tuned.

## Validation and revisit trigger

`crates/rustic-dom/tests/tracing.rs` covers a rooted detached tree surviving and
being collected once unrooted, wrapper and listener cycles collected while
rooted cycles survive, connected nodes keeping objects (and what those hold),
template/contents liveness in both directions, engine-destroyed nodes leaving no
dangling entries, rejected foreign/stale/node-to-node edges, exact limits, a
100,000-object ring through a 256-deep chain collected without recursion, and
200 seeded random graphs compared with an independent fixpoint model.

`crates/rustic-dom/tests/conformance.rs` runs all 25,230 pre-insert combinations.
Revisit when JS-03 provides the real heap, when adoption or shadow trees add
cross-tree edges, or when a measured pause budget is set.
