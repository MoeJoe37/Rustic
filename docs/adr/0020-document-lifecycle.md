# ADR 0020 — Headless document lifecycle

Date: 2026-11-15
Status: Accepted

## Context

The CLI created independent cancellation tokens for HTML parsing and later
stages of the same document. A host could neither close document admission
nor signal all the work associated with a document. The M5 scheduler also
needs a document lifetime that is independent of platform event loops.

## Decision

`Engine` owns a bounded admission policy and a registry of weak cancellation
scopes. A pending load owns its parser and scope. Finishing a parse transfers
the scope and the completed document to `EngineDocument`. A failed or dropped
owner releases its slot and signals its token. Workers receive a cloned token
that does not retain the tree or the admission slot.

Shutdown is permanent and idempotent. It closes admission before signalling
each live scope. Dropping the coordinator performs the same operation. Each
host owns the timing of worker joins and the release of its document owners.
File handles, local resource policy and render outputs remain owned by the
host and its stages. Parser, DOM and render budgets apply in addition to the
limit on simultaneous documents.

## Alternatives and consequences

A strong registry would retain abandoned document resources until explicit
unregistration. Keeping independent tokens would require each stage to
propagate shutdown by hand. A weak registry allows the document owner to
release its resources directly and one token to cross the stage boundaries.
Admission prunes dead entries with a scan bounded by the document limit.
Shutdown signals cooperative checkpoints. It does not join workers or
interrupt blocking host I/O.

## Validation and revisit trigger

Lifecycle tests cover pending and completed slots, failure and abandonment,
cross-thread token observation and repeated shutdown. The CLI checks document
admission and rendering after shutdown. Revisit how the host drains active
work when WEB-01 introduces document task queues and in-flight jobs.
