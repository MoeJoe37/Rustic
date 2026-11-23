# ADR 0076 — Window URL records and live search parameters

Date: 2026-11-23
Status: Accepted for M9 `url.setters`

## Context

The URL parser operates on validated records. JavaScript exposes a mutable
URL whose `searchParams` property returns the same object after every update.
URL setters have state-specific recovery rules. For example, `host` can
replace the hostname before an out-of-range port is rejected.

## Decision

Implement component overrides inside the original parser and return a new
validated record under the parser's byte, segment and cancellation limits.
Window `URL` and `URLSearchParams` share one private snapshot of the URL and
its ordered decoded pairs. Complete argument coercions before reading the
current snapshot, then publish both parts in one slot write. Iteration walks
the current pairs by index, including changes made by a `forEach` callback.

Keep links between wrappers and iterators in the engine's existing traced
weak-map records. The immutable Rust snapshot uses the native binding storage
from ADR 0075, with retained capacity charged to the agent. After mutation,
an old snapshot stays live only while an active binding holds it or until
collection. Parsing and mutation perform no navigation or resource access.

## Alternatives and consequences

Reparsing a serialized URL on every property access would lose parser state
in some setter sequences and duplicate work. A record update preserves the
parsed components and applies the relevant parser state directly. Mutations
copy the bounded native snapshot before publishing it, which preserves the
previous state on failure. Measure copy costs before choosing a different
representation for large query lists.

## Validation and revisit trigger

The bindings test replays all 278 cases in the pinned WPT setter dataset and
checks object identity, reentrant updates, and live iteration. The URL crate
checks that failed overrides leave the source record intact. Revisit exposure
and ownership when workers gain Window-independent Web IDL installation, or
when blob URL registries add origin and lifetime state to URL parsing.
