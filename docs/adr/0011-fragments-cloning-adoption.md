# ADR 0011: General fragments, cloning and cross-document adoption

- Date: 2026-11-08
- Status: Accepted internal contract; script bindings pending (DOC-09, WEB-02)

## Context

ADR 0010 added template contents as the only `DocumentFragment` and deferred
general fragments, DOM insertion of fragments, cloning and adoption. M1's DOC-04
needs all of them, and the HTML parser needs same-document cloning for
`selectedcontent`. Node IDs embed their owning document and slot generation
(ADR 0002), so a node cannot keep its ID when it moves to another arena.

## Decision

- `Document::create_document_fragment` creates a free-standing fragment. A
  fragment's template ownership is recorded on the node (`Node::template_host`),
  not in a separate kind: only template contents are protected from detach and
  destroy, and only they skip adoption.
- `insert_before`/`append_child` with a fragment child (including template
  contents) moves the fragment's children in order, as the DOM's insert
  algorithm does. Every check runs for all children before anything moves.
- Pre-insert validation follows DOM step order, and the ancestor check walks
  host-including ancestors (from template contents to their template).
- `NodeKind` gains `ProcessingInstruction` (validated XML `Name` target, data
  without `?>`) and `CdataSection` (data without `]]>`). Both are character data;
  edits keep a processing instruction's target and its byte accounting.
- `clone_node(node, deep)` and `adopt_node(source, node)` share one engine: an
  iterative preorder snapshot of the source (a flat plan with parent indexes),
  then materialization into the target, which rolls back everything it created
  on a node, data, depth or allocation failure. Template copies always get their
  own contents fragment; a deep copy fills it.
- Adoption copies the subtree with new IDs, then destroys it in the source. It
  returns `Adopted { root, ids }`, an old-to-new ID map, so a future binding
  layer can retarget wrappers (TD-16). `clone_document` deep-copies a whole tree.
- `ReadyState` (`Loading` → `Interactive` → `Complete`) only moves forward.
  `Document::new` is `Complete` like a script-created document;
  `Document::new_for_parsing` starts `Loading`.

## Alternatives and consequences

Moving slots between arenas would keep IDs but break the per-document slot and
generation invariants and share budgets across documents. Pre-counting budgets
before cloning would avoid the rollback cost, but rollback already gives exact
atomicity for every failure, including allocation, with less duplicated logic.
Rollback costs time proportional to what was created; that only happens on
failure.

Template contents still belong to the same document; the HTML Standard's inert
template contents owner document is not modelled. Live ranges, mutation records
and the adopting steps of custom elements are not run.

## Validation and revisit trigger

`crates/rustic-dom/tests/node_kinds.rs` covers fragment moves, rejected fragment
insertions moving nothing, step order, host-including cycles, processing
instruction and CDATA validation and accounting, and dumps.
`crates/rustic-dom/tests/adoption.rs` covers deep/shallow clones, template
contents, clone and adoption failures at node/data/depth limits leaving both
documents unchanged, ID maps, invalidated source IDs, `clone_document` and
`ReadyState`. Revisit when JS wrappers exist (DOC-09/WEB-02), when live ranges
or mutation observers are added, or when an inert template document is needed.
