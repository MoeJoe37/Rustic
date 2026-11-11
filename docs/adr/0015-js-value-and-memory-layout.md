# ADR 0015: JavaScript value and memory layout

Date: 2026-11-11. Status: accepted for JS-02; collection is ADR 0016 (JS-03).

## Context

JavaScript heaps are among the largest consumers of a browser's memory. A page
holds millions of values in properties, arrays, closures and strings, so the
size of one value, one property slot and one string code unit multiplies across
everything a page does. Rustic must beat Chrome and Firefox on measured memory
(ADR 0007, PERFORMANCE.md), stay safe Rust (no `unsafe`, no raw pointers) and
use a nonmoving traced heap with typed handles (ADR 0002). Those engines reach
small values with pointer tagging; Rustic cannot tag pointers in safe Rust, but
it does not need pointers at all.

## Decision

**Values are 8 bytes.** `Value` is a NaN-boxed `u64`. A Number is stored as its
IEEE-754 bits, with every NaN canonicalized to one bit pattern. Every other
value lives in the negative quiet-NaN space no Number uses: the top 16 bits are
a tag (undefined, null, boolean, string, symbol, BigInt, object, and the
internal "uninitialized binding" marker) and the low 32 bits carry a boolean or
a heap index. Encoding and decoding are integer operations (`f64::to_bits`,
`f64::from_bits`), so this is portable to every tier (big-endian s390x
included) and needs no `unsafe`. A 16-byte tagged enum would double the cost
of every slot.

**The heap is typed arenas indexed by `u32`.** Objects, strings, symbols and
BigInts each live in a `Vec`-backed arena owned by one `Heap`, and values name
them by index. Script cannot forge an index; host-facing handles add
generations in JS-03 (as `HostObjectId` does). There is no `Rc`, `RefCell` or
per-object allocation header. Every arena allocation is charged to a
`rustic_base::Budget`, so a page that exceeds its heap limit gets a catchable
failure, not process growth.

**Strings are Latin-1 when they can be.** A string whose code units are all
below 256 (most of the web: identifiers, markup, JSON keys, ASCII text) stores
one byte per unit; others store UTF-16. Semantics stay UTF-16 everywhere.
Property-name strings are interned as atoms, so `obj.width` on a thousand
objects shares one copy of `"width"`, and key comparison is a `u32` compare.
Array indices up to 2³²−2 are a separate key kind and never become strings.

**Objects share shapes.** An object stores a shape id, a prototype, flags,
named-property slots and indexed elements. A shape (hidden class) is a node in
a transition tree recording one key, its attributes and its slot. Objects
built the same way share the same chain, so per-object cost is one 8-byte slot
per property. An object that deletes properties or grows past a limit switches
to its own dictionary instead of growing the shared tree without bound.
Accessor properties use two consecutive slots (getter, setter). Indexed
elements are a dense `Vec<Value>` with holes, falling back to a sparse map
when an index would leave too many holes.

**Bindings use the same slots.** Declarative environments are slot vectors with
the TDZ marker as the "uninitialized" value, so a binding costs 8 bytes plus a
shared name table.

## Alternatives

- *16-byte enum `Value`*: simpler, but doubles every property, array element and
  binding.
- *`Rc<RefCell<Object>>` graph*: per-object refcount and borrow headers, cycle
  leaks, and no budget accounting; rejected by ADR 0002.
- *Always UTF-16 strings*: doubles memory for most real strings.
- *Per-object hash maps*: simple, but tens of bytes per property and no
  inline-cache path for the interpreter.

## Consequences

Values are `Copy` and compare by bits. Handles are only meaningful with their
`Heap`, so every operation takes the heap explicitly. Nothing is collected until
JS-03 adds mark/sweep over these arenas, with free lists that keep indices
stable (nonmoving). Shape transitions and dictionary mode add code paths that
the object tests must cover. The interpreter (JS-04) can cache a shape id and
slot per property access site.

## Validation and revisit trigger

Tests pin `size_of::<Value>() == 8`, the round trip of every special double
(±0, ±∞, NaN payloads, subnormals), the one-byte storage of Latin-1 strings,
shape sharing across many objects, dictionary fallback, budget failures, and
the ECMA-262 ordinary internal-method rules. Revisit with heap profiles from
real pages (JS-07 onward): if most objects are dictionaries or strings are
mostly non-Latin-1, or if 32-bit indices limit real heaps.
