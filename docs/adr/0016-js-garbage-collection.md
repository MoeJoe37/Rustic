# ADR 0016: JavaScript garbage collection

Date: 2026-11-11. Status: accepted for JS-03 (collector core); weak references
and DOM host-object tracing follow in the second part of JS-03.

## Context

ADR 0015 put every JavaScript value in typed arenas indexed by `u32` and
charged to a byte budget, but nothing was freed (TD-91). ADR 0002 requires a
nonmoving traced heap with typed handles and no `unsafe`. The collector decides
how much memory a long-lived page holds, how long script pauses, and whether a
bug in rooting can become memory corruption. Rustic must beat other engines on
measured memory (ADR 0007), so the collector's own overhead matters as much as
what it frees.

## Decision

**Stop-the-world mark/sweep, nonmoving.** A collection marks everything
reachable from the roots and frees the rest. Live cells keep their indices, so
handles, shape ids and atoms stay valid without forwarding or write barriers.

**No per-cell header.** Mark bits live in bit sets allocated for one collection
(one bit per arena index, about 1/64 byte per 8-byte value slot's owner), not
in every object. An empty object stays 72 bytes.

**Explicit worklist.** Marking pushes objects and environments on a `Vec` and
never recurses, so a prototype or scope chain of any depth cannot overflow the
native stack. Strings, symbols and BigInts have no outgoing edges worth a
worklist entry and are marked in place.

**Free lists that keep arenas compact.** Sweep clears unmarked cells and
rebuilds each arena's free list so the lowest index is reused first. A free
tail is truncated and its allocation returned, so a heap that shrinks gives
memory back to the process rather than only to its own free lists.

**Shapes and atoms are collected too.** A shape is live when a live
non-dictionary object uses it or a descendant of it; dead shapes are freed and
removed from their parents' transition lists. An interned string is live when
something names it (a shape, a dictionary key, a binding, a value); the atom
table is rebuilt without dead entries. Without this, a page that builds
objects with generated property names would leak shapes and names forever.
An object that becomes a dictionary drops its shape for the same reason.

**The budget restarts from the live footprint.** Between collections each
allocation is charged an estimate. After sweep the collector recomputes the
exact charge of everything live and sets the budget to it, so estimates cannot
drift and "bytes used" after a collection is a real measurement.

**Collection happens at safe points, not inside allocation.** A handle in a
Rust local is invisible to the collector (there is no stack scanning in safe
Rust), so an allocation never collects by itself. The embedder polls
`Agent::should_collect` where it can enumerate what it holds and passes those
handles as `Roots`. The interpreter (JS-04) does this between instructions with
its registers and frames. Roots are: the agent's names, well-known symbols and
intrinsics; the caller's `Roots`; and the host root table.

**Host roots are generation-checked.** `Agent::root` returns a `HostRoot`
(index plus generation). After `unroot`, the old handle reads as `None` even
when its slot is reused, so a stale host handle can never name a different
value.

**Use of a freed cell panics.** Arena cells are `Option`s. A rooting bug shows
up as a deterministic panic at the first use, never as silent aliasing of
another object. In safe Rust it cannot be memory corruption in any case.

**Policy.** Collect when charged bytes reach a threshold: 1 MiB at first, then
twice the live footprint after each collection (at least 1 MiB above it), and
never above seven eighths of the heap limit, so a heap near its limit collects
before an allocation fails.

## Alternatives

- *Reference counting*: frees promptly, but needs a count per cell, cannot free
  cycles (ubiquitous in DOM/JS graphs) and was rejected by ADR 0002.
- *Moving/compacting or generational collection*: better locality and shorter
  pauses, but every handle held by Rust code would need updating, which safe
  Rust cannot do without handle indirection on every access. Revisit with
  measurements.
- *Collecting inside allocation with handle scopes*: lets allocation trigger
  collection anywhere, at the cost of registering every temporary in every
  runtime function. Safe points give the same bound on growth for interpreter
  code at no per-operation cost; long-running native functions are TD-94.
- *Mark bits in the object header*: no side allocation, but adds a byte (padded
  to eight) to every object or steals a flag bit and forces a full clearing
  pass.

## Consequences

Pauses are proportional to the live heap plus arena extents; incremental or
generational marking is future work once the interpreter can measure real
pauses (TD-95). Rust code must not hold unrooted handles across a
`collect_garbage` call. Free-list reuse means an index observed before a
collection may name a different value afterwards, which the panic-on-freed
rule and generation-checked host roots contain.

## Validation and revisit trigger

`tests/gc.rs` checks each root kind, every reference kind inside an object,
cycles, 200,000-deep prototype and scope chains, atom and shape reclamation,
index reuse without disturbing survivors, host root generations, the policy
and exact budget return to the bare-agent baseline. The `js-heap` fuzz target
drives random allocation, mutation, linking, rooting and collection against a
shadow model and requires every modeled read to match and the budget to return
to baseline. Revisit when JS-07 heap profiles show pause times over the
PERFORMANCE.md budget, fragmentation from interleaved long-lived cells, or
native functions that allocate far past the threshold between safe points.
