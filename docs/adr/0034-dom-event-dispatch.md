# ADR 0034 — Scripted DOM event delivery and listener ownership

**Status** Accepted for WEB-04.

## Context

The [DOM Standard's events chapter](https://dom.spec.whatwg.org/#events)
defines EventTarget registration and synchronous dispatch. Listeners can
mutate registrations, move nodes, and invoke another dispatch while the
outer event is active. A listener can hold its own target, including an
otherwise unreachable detached DOM tree.

## Decision

Keep Event and listener records in private JavaScript arrays with null
prototypes. Two page-owned weak maps hold the records by event and target
identity. Their ephemeron edges let each reachable target retain its
listeners without making the target a root. The joint collector already
traces reachable DOM wrappers, so connected nodes retain their listeners
and adoption keeps the same listener list. Window resolves to its
WindowProxy as an event target.

Collect the light-DOM path before invoking any listener and keep it alive
through the event record. Include Window when the path reaches the active
document, with the document load exception. Snapshot matching listener
records at the start of each phase for its current target. List mutations
unlink a record and mark it removed so a pending snapshot can skip it.
Once listeners unlink before invocation. Release DOM borrows before calling
script and use the agent's native work budget for path and listener traversal.

The dispatch clears temporary state after normal completion and fatal
interruption. Listener exceptions increment a host report count and leave
dispatch running. Event timestamps use the page host's monotonic origin.
Trusted scheduling, default actions, AbortSignal, shadow retargeting,
and report delivery to HTML error handlers are tracked as TD-131.

## Verification

Binding tests cover propagation order, cancellation, passive and once
listeners, option coercion order, same-event redispatch rejection, listener
mutation, path retention across document adoption, and cancellation cleanup.
An engine test verifies dispatch and promise checkpoints across script
turns and checks the resulting mutation in the renderer-owned tree.
