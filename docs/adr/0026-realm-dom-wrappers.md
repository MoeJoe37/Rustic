# ADR 0026 — Realm-scoped DOM wrappers and adoption

Date: 2026-11-16. Status: accepted for the WEB-02 host bindings.

## Context

ADR 0016 defines one marking phase across JavaScript host objects and one
document. WEB-02 needs an agent with several documents and realms. The DOM
adoption algorithm preserves the node across changes of owner document.
Rustic's document arenas assign new internal handles when that node moves
(ADR 0011), including handles for its owned trees.
[DOM Standard, adopting a node](https://dom.spec.whatwg.org/#concept-node-adopt)
[Web IDL Standard, relevant realms](https://webidl.spec.whatwg.org/#dfn-relevant-realm)

## Decision

Use one `DomHeap` per agent. Cache a host wrapper for each accessed
node and realm. The wrapper starts with its creation realm's
`Object.prototype` until the interface prototypes are installed.
A wrapper lookup checks the agent, the host object and its embedder
data key, and the node's current document and generation.

Start marking from the root of every registered document and from the
agent's roots. A reachable wrapper marks its node's whole tree. Each
newly marked node returns all its realm wrappers and node-held values
to the same JS mark phase. Collection accepts the entire registered
set so a cross-document edge cannot be dropped by a partial trace.
The DOM sweep visits occupied slots and frees unmarked trees without
allocating a list of roots.

Adoption reserves capacity to move affected cache records before the DOM
commits the move. The binding applies the full old-to-new ID map, including
shadow trees and template contents. The wrapper object, its prototype and
its properties keep their identity. Node-held edges follow the same map.

## Alternatives and consequences

A single wrapper across all realms would share one set of host properties
and one prototype. Realm-scoped wrappers let the binding choose interface
prototypes in the creation realm. Independent per-document collection
would need an additional mechanism for cycles through JavaScript and across
documents. The agent already supplies a shared mark phase.

The coordinator is an explicit host API. WEB-03 must connect automatic
interpreter safe points before page interfaces expose node wrappers (TD-123).
Document unregistration and reclamation of detached realms belong to
the page host lifecycle (TD-118 for realm reclamation).

## Validation and revisit trigger

The binding tests exercise realm-specific prototypes and expandos, two-way
cross-document tracing, wrapped shadow descendants after adoption, allocation
limits during adoption and matching raw host IDs in different scopes. The
existing seeded graph model, weak-map cases and safe-point stress tests
continue to exercise wrapper reachability. Revisit the host lifetime when
page scripts first receive DOM wrappers.
