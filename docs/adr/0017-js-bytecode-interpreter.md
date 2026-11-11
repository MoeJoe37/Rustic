# ADR 0017: JavaScript bytecode and interpreter

Date: 2026-11-11. Status: accepted for JS-04.

## Context

JS-01 to JS-03 parse script and model its values in a collected heap
(ADR 0015, ADR 0016), but nothing ran. The interpreter decides how much memory
a function, a closure and a call cost, how fast ordinary loops and property
accesses are, and whether hostile script can exhaust the Rust stack or run
forever. ADR 0002 requires safe Rust and no third-party engine; ADR 0007
requires measured memory and speed.

## Decision

**Register bytecode, compiled once per function.** The compiler walks the AST
and emits fixed-size instructions (`Op`, 12 bytes) over `u16` registers. A
scope pass first decides where each binding lives: a binding no closure
captures is a register, and a captured one is a slot in a heap environment,
addressed by index. Names are searched at run time only for globals and inside
`with`. Compiled code lives in a collected arena and is shared by every
closure of the function; unreachable code is freed with its closures.

**Small closures.** A closure is its function object plus a 24-byte record
(code, environment, home `this` for arrows) in a typed arena, not a boxed
structure per function. Its `prototype` object is created on first read, and
scope environments share the name list owned by the compiled code.

**No Rust recursion for script calls.** Frames and registers live on two
vectors in the agent. A script-to-script call pushes a frame and continues the
same dispatch loop, so recursion depth is bounded by `VmLimits::max_frames`
(10,000) and `max_stack_values` (2²⁰), and exceeding either throws a
catchable `RangeError`. Script reached through native code (getters,
conversion methods, host callbacks) nests one Rust frame per level, bounded by
`max_native_depth` (96) and by measured Rust stack use (1 MiB).

**Safe points.** Every backward jump is a `Loop` instruction and every call
is a safe point. A safe point polls cancellation, counts down an optional step
budget, and collects garbage if the heap asks and no native code is on the
stack (the interpreter's registers, frames and code are roots). Cancellation
and out-of-memory are not catchable by script.

**Inline caches keyed by shape and one epoch.** Each named property site and
global site holds one cache entry: own data property (shape, slot), inherited
data property (receiver shape, receiver prototype, holder, slot), or property
add (old shape, new shape, slot). An entry is valid only for the epoch it was
written in. The epoch changes on every collection (ids are reused), on every
new global lexical binding, and on every change to the named properties or
prototype of an object that is used as a prototype. So an inherited hit needs
no walk of the chain, and no dependency lists are stored. Sites cost about 28 bytes
each and are per function, not per closure.

**Fused instructions where measurement showed a gain**: arithmetic with a
small integer constant, compare-and-branch, loop back edges combined with the
safe point, and array iteration without iterator result objects while
`%ArrayIteratorPrototype%.next` is untouched.

## Alternatives

- **Walking the AST.** No compile step, but each variable access searches
  scopes by name and each node costs a Rust call; several times slower and
  recursion follows script nesting.
- **Stack bytecode.** Smaller instructions, but about twice as many
  dispatches for the same code.
- **Rust recursion per call.** Simpler, but script recursion depth would
  depend on Rust frame sizes and build mode, like the parser guard (TD-90).
- **Per-shape dependency lists for prototype caches.** More precise
  invalidation than one epoch, at a cost in memory per shape and per site. To
  be reconsidered if profiles show epoch flushes (TD-103).
- **A JIT.** Excluded: it needs `unsafe` and executable memory (ADR 0002).

## Consequences

Programs that keep changing prototype objects after start-up flush every
cache each time. Polymorphic sites keep missing (one entry per site). The
value stack and compiled code are bounded by limits and budgets, but the
stack's bytes are not charged to the heap budget (TD-102). Syntax the compiler
does not handle yet is rejected with `CompileError::Unsupported` before
anything runs (TD-97).

## Validation and revisit trigger

`tests/interp.rs` runs about 270 scripts covering operators, scopes, control
flow, functions, closures, constructors, exceptions, iteration, cache
invalidation, limits, cancellation and collection during a run. The `js-run`
fuzz target runs each mutated script with caches on, with caches off and
with a collection at every safe point, and requires the same result. Revisit
when the Test262 runner (JS-07) or the benchmark gate (JS-09) shows a failure
class or a cost this design cannot fix locally.
