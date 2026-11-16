# ADR 0030 — DOM selectors and NodeList ownership

Date: 2026-11-16. Status: accepted for WEB-03.

## Context

Script can mutate the live document and keep detached nodes. Selector APIs
need the same CSS interpretation used by style matching, and collections
have distinct membership and lifetime rules.
[DOM Standard, old-style collections](https://dom.spec.whatwg.org/#old-style-collections)
[Selectors Level 4, scope](https://www.w3.org/TR/selectors-4/#the-scope-pseudo)
[Web IDL, legacy platform objects](https://webidl.spec.whatwg.org/#legacy-platform-object)

## Decision

The binding crate calls the repository's CSS parser and style matcher
through their read-only interfaces. String coercion finishes before the
query borrows the current DOM. An element scope can participate in matching
ancestors while the result walk visits only its descendants. A fragment
can participate as a featureless virtual scope. Parser quirks mode controls
ID and class selector matching. The CSS parser's limits and cancellation
token apply. A work allowance also meters selector traversal and rejects
the full query when exhausted.

NodeList uses read-only indexed host objects with script-visible property
descriptors, key enumeration, and the standard array-like iterator
operations. The JS heap owns the snapshot's boxed wrapper handles.
A live child list traces its source wrapper and reads the current sibling
chain on access. The DOM traces a cached child list from its source node
to preserve same-object identity. The cache itself has no independent
root and drops entries when the joint collection destroys the node.

## Alternatives and consequences

A second CSS matcher would duplicate syntax, case folding, and scoping
rules. Exposing arrays would allow scripts to rewrite snapshot membership
and would not provide live indices for child lists. Shared indexed
dispatch keeps the JS runtime independent of DOM types.

This change adds only dependencies on existing workspace crates. The
matcher and parser already participate in the same dependency and license
gates. It introduces no native library or I/O authority. The supported
selector grammar is the existing CSS subset. Live named HTMLCollection
properties remain a separate WEB-03 step, and TD-128 tracks positional
lookup cost.

## Validation

Tests cover element and fragment scopes, invalid selectors, quirks mode,
tree order, reentrant coercion, and query budget exhaustion. Indexed
reflection, iterators, and static versus live membership are checked
under collection stress. Releasing the last list reclaims detached DOM
and JS cycles.
