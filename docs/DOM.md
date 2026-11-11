# Document storage and character data

`rustic-dom` owns the internal node arena, checked tree mutations and retained
payload accounting. It is usable without an HTML parser or JavaScript runtime.
The [implementation ledger](IMPLEMENTATION.md) records the supported hierarchy,
identity and traversal subset; [architecture](ARCHITECTURE.md) describes future
tracing and bindings. This document specifies the character-data storage contract.

## Checked text and comment edits

Only the owning `Document` mutates a text/comment payload. Node views and their
`DomString` values are read-only; cloning a value produces an independent owned
snapshot. Every operation checks document identity, occupied slot, generation
and node kind, including operations with empty input.

| Operation | Behavior |
| --- | --- |
| `set_character_data(id, value)` | Replace the whole owned value and return the previous value |
| `replace_character_data(id, offset, count, units)` | Replace a UTF-16 range with the borrowed units |
| `append_character_data(id, units)` | Append raw UTF-16 units without cloning the existing value |
| `append_utf8_character_data(id, text)` | Encode the supplied UTF-8 fragment directly into the node's UTF-16 storage |

Offsets and counts are **UTF-16 code units**, not bytes, Unicode scalar values or
graphemes. An offset beyond the current length returns `InvalidOffset`. A count
beyond the remaining length is clamped before arithmetic, even for `usize::MAX`.
Insertion at the end and empty edits are valid. Units may include NUL and lone
surrogates; replacement can split a surrogate pair. UTF-8 append preserves all
pre-existing units and does not normalize line endings or strip a BOM. Input
preprocessing belongs to the parser, not DOM storage.

These range semantics follow the storage portion of the DOM Standard's
[replace-data algorithm](https://dom.spec.whatwg.org/#concept-cd-replace), reviewed
2026-11-08 against the living standard last updated 2026-11-04. This is not a full
`CharacterData` implementation: Web IDL conversion/exception mapping, live-range
adjustment, mutation records/observers, text
splitting and normalization remain pending. No browser conformance suite has
been run for this storage subset.

## Reservation, accounting and ownership

Edits validate the handle/kind/offset, compute the clamped range and checked final
unit/byte lengths, and check the shared document budget before reserving space.
After reservation succeeds, accounting and the in-place edit complete without
callbacks or further allocation. Validation, budget and reservation errors leave
logical content, accounting, node identities and tree links unchanged.

`DomString` privately owns a growable UTF-16 vector. Growth reserves capacity
before moving a suffix; shrinking retains reusable capacity. UTF-8 append counts
the new fragment's units before reservation and encodes directly into the vector,
without an intermediate converted string. Repeated appends reuse capacity with
amortized growth; they do not clone the entire existing text on each call. This
supports a future HTML tree builder's adjacent-text coalescing without allowing
the parser to bypass document accounting. No parser/tree-builder integration or
performance benchmark is claimed here.

The document budget charges retained UTF-8 names and UTF-16 payload **lengths**
across all live nodes and attributes, including detached nodes. Each UTF-16 unit
costs two bytes. Edits charge only growth and release deleted payload bytes;
destruction releases the node's current charge. Capacity, allocator overhead,
caller buffers and returned/cloned values are outside this logical payload
budget. It is not a process memory bound. Fallible reservation is handled, but
process-wide allocation failure is not a recoverable guarantee for all DOM APIs.

An edit does not move, attach, detach, split or create a node. The parser will
choose the correct insertion location and adjacent text node through checked
document APIs. Future observers/ranges must be integrated at the mutation
boundary before these primitives are exposed as web APIs.

## Template contents

An HTML `template` element can own one `DocumentFragment` created by
`Document::create_template_contents`; `Node::template_host` names its template.
The fragment is a separate root, not a child: it accepts ordinary children but
cannot be detached or destroyed by itself, and `walk` does not enter it.
Inserting it moves its children out, like any fragment. Destroying the template
destroys its contents, including arbitrarily nested templates, without
recursion. `dump_tree` prints contents as `#template-contents` after the
template's attributes. Indentation stops at 64 levels; deeper lines (reachable
only through nested template contents) start with `@<depth> ` so dump size stays
linear in node count. Inert template owner documents are not implemented; see
[ADR 0010](adr/0010-template-contents.md) and [ADR 0011](adr/0011-fragments-cloning-adoption.md).

## Node kinds, fragments, cloning and adoption

| Operation | Contract |
| --- | --- |
| `create_document_fragment()` | Free-standing fragment root (`#document-fragment` in dumps); can be destroyed or adopted |
| `insert_before`/`append_child` with a fragment | Moves the fragment's children in order; all checks precede any move; the fragment stays, empty |
| `create_processing_instruction(target, data)` | Target must be an XML `Name` (`InvalidName`), data must not contain `?>` (`InvalidCharacterData`); allowed beside the document element |
| `create_cdata_section(data)` | Data must not contain `]]>`; text-like, so not a document child |
| `clone_node(node, deep)` | Detached copy with new IDs, names, ordered attributes and data; templates get their own contents (filled when deep); the root is rejected |
| `adopt_node(&mut source, node)` | Copies into this document, destroys in `source`, returns `Adopted { root, ids }` (old → new IDs) |
| `clone_document()` | New `Complete` document with the same limits and a deep copy of the tree |
| `ready_state()` / `advance_ready_state(next)` | `Loading` → `Interactive` → `Complete`, forward only (`InvalidState`) |

Pre-insert validation follows the DOM Standard's
[ensure pre-insert validity](https://dom.spec.whatwg.org/#concept-node-ensure-pre-insertion-validity)
step order: parent kind, host-including ancestor (a template cannot enter its
own contents), reference child, then child kinds and document-child rules.
Processing-instruction and CDATA data are character data: the range and append
operations above apply, and the creation-only restrictions do not. Cloning
follows [clone a node](https://dom.spec.whatwg.org/#concept-node-clone) and the
HTML template cloning steps; adoption follows
[adopt](https://dom.spec.whatwg.org/#concept-node-adopt) except that IDs change
(TD-16). Reviewed 2026-11-08 against the living standards. Clone and adoption
work iteratively and roll back on any node, data, depth or allocation failure,
leaving both documents, their IDs and accounting unchanged. Live ranges,
mutation records and custom element adopting steps are not implemented.

### Shadow roots

`Document::attach_shadow` gives an HTML element a shadow root (open or closed)
as `attachShadow` does: the host must be a valid custom element name or one of
the HTML elements the DOM Standard lists (`WrongNodeKind` otherwise), and an
element has at most one (`InvalidHierarchy`). A shadow root is a parentless
`DocumentFragment` with `Node::shadow_host`, owned like template contents: it is
not a child, `walk` skips it, it cannot be inserted, detached, destroyed or
adopted on its own, the host-including ancestor check hops from it to its host,
it is destroyed and adopted with its host, clones drop it (no shadow root is
clonable yet), dumps print it as `#shadow-root` before the host's children, the
tracing collector keeps host and shadow tree alive together, and
`check_invariants` verifies the ownership links. Slot assignment, declarative
shadow roots and the custom element registry are not implemented. The custom
element name check approximates HTML's `PCENChar` ranges (TD-24).

## Garbage collection (DOC-09, JS-03)

Nodes are collected together with the JavaScript heap
([ADR 0012](adr/0012-dom-tracing-prototype.md),
[ADR 0016](adr/0016-js-garbage-collection.md)). The DOM owns the tree half:

| Operation | Behavior |
| --- | --- |
| `Document::tree_marks()` | Mark table for one collection, reserved up front |
| `mark_tree(&mut marks, node, visit)` | Marks the whole tree holding `node`, the trees its elements own (template contents, shadow roots) and the element owning it; calls `visit` once per newly marked node |
| `mark_document_tree(&mut marks, visit)` | Marks the document's own tree, which is always live |
| `sweep_unmarked_trees(&marks)` | Destroys every unmarked parentless tree through `destroy_subtree`; returns the nodes destroyed |

Reachability is per tree: holding any node keeps its whole tree, a template and
its contents keep each other, and so do a host and its shadow root. Marking and
destruction are iterative.

`rustic-bindings` joins this to the JS collector. `DomHeap` gives each node one
wrapper (a JS host object, the same object on every request), records the
values a node keeps alive (event listeners), and runs one collection over both
sides: a reachable wrapper marks its node's tree, a marked tree reports its
nodes' wrappers and node-held values, unreachable trees are destroyed and
their wrappers freed. A wrapper whose node the engine destroyed stays a plain
object that names no node. The heap is bound to one document (TD-17).
The DOC-09 prototype `HostHeap` that stood in for the JS heap is gone.

## Structural invariants and the conformance gate

`Document::check_invariants` checks the whole document in linear time: mirrored
parent/child/sibling links, acyclic parent chains, the depth limit, permitted
parent and child kinds, document child order, single ownership of parentless
template contents, and node/payload accounting. It is a test and fuzz oracle;
checked mutation APIs already maintain these rules.

`tests/conformance.rs` is the DOM conformance gate for insertion. It runs every
(parent, node, reference child) combination over a fixture with each node kind,
connected and detached trees and template contents, and compares the result with
an oracle transcribed from the DOM Standard's
[ensure pre-insert validity](https://dom.spec.whatwg.org/#concept-node-ensure-pre-insertion-validity)
and pre-insert steps (reviewed 2026-11-08): exception class and step precedence
(`InvalidHierarchy`, `Cycle` and `ProtectedRoot` map to `HierarchyRequestError`;
`InvalidReference` maps to `NotFoundError`), resulting child order, unchanged
trees after a rejection, and invariants after every call. Engine depth and node
limits are not spec behavior and are tested separately. No web-platform-tests
have been run; they need script bindings.

## Regression evidence

Run `cargo test --locked -p rustic-dom`. `tests/node_kinds.rs` and
`tests/adoption.rs` cover the fragment, node-kind, clone, adoption and lifecycle
contracts above. The character-data suite covers:

- Range growth/shrink/deletion, end insertion, count clamping, pair splitting,
  lone surrogates and snapshot isolation for text/comments.
- Exact shared budgets with names/attributes and detached nodes, rejected edits,
  released payload reuse and destruction after resizing.
- Wrong kinds, foreign/stale/forged/absent handles, invalid offsets and empty
  edits at zero/full budgets; pure checked-arithmetic overflow cases.
- 4,096 small UTF-8 appends compared with one whole append, preserving exact
  units and accounting in one node per run.
- 2,500 deterministic edits compared after every operation with an independent
  prefix/insert/suffix model for two nodes sharing a limited budget.

The existing tree-link model, deep-tree destruction and deterministic dump suites
also remain required. Allocation-failure injection and live web DOM tests are
future work.
