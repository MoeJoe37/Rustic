# ADR 0028 — Joint collection during document script execution

Date: 2026-11-16. Status: accepted for the first WEB-03 interfaces.

## Context

The explicit collector from ADR 0026 already traces wrapper, tree, and
script edges together. The interpreter's automatic collection path must
use it once page script can hold a wrapper. Rendering also needs the
same DOM that script mutates.
[DOM Standard, Node](https://dom.spec.whatwg.org/#interface-node)
[DOM Standard, Document](https://dom.spec.whatwg.org/#interface-document)

## Decision

An agent can attach one execution host. At a VM safe point, the host
performs joint collection or returns a fatal failure. There is no
JavaScript-only fallback on a failed joint collection. Standalone
ECMAScript agents still use their usual collector. Promise jobs pass
through the same VM safe points with the active job rooted.

The scripted document owner and its binding host share the original
parsed tree through a thread-local checked borrow. The inert document
and renderer retain their existing read API. A scripted document exposes
a scoped read guard when a host inspects it between turns. Node binding
operations resolve receivers through the agent's wrapper table, borrow
the tree for one operation, and return before any script callback.
A terminal script failure releases the agent and binding cache. The
document owner retains its tree for inspection until it is dropped.

The first exposed interfaces are the live document, base Node and
Element. Child insertion and removal use existing transactional storage
operations. Removal detaches. The joint reachability sweep determines
when the removed tree can be destroyed.

## Alternatives and consequences

A second copy of the tree would require synchronization before rendering
and would split node identity. Keeping GC at task boundaries would miss
allocation pressure within a script or a promise job. Passing document
references through every VM call would couple shared native and module
execution to browser-only state. An agent-local host keeps those layers
independent.

The initial page host registers one document. Additional documents and
specialized node prototypes are TD-126. Styling and mutation observers
will consume changes through their own WEB-04 and WEB-06 integration.

## Validation

The binding test runs garbage collection at every safe point, keeps a
detached node alive through a queued job, reattaches it during the
checkpoint, and verifies that an unreachable detached cycle is destroyed.
The engine integration test verifies wrapper brands, rejected mutations,
and the mutated tree as seen by the next renderer call. Revisit the
borrow contract when a binding needs to invoke script during a mutation.
