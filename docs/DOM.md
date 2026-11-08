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
adjustment, mutation records/observers, processing-instruction nodes, text
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

## Regression evidence

Run `cargo test --locked -p rustic-dom`. The character-data suite covers:

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
