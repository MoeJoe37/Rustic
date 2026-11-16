# ADR 0022 — Document task sources and timers

Date: 2026-11-16
Status: Accepted for WEB-01 host task selection and timers

## Context

ADR 0021 introduced a host-driven turn on each scripted document. WEB-01
also needs selection among ready task sources, delayed work and deterministic
test clocks.

## Decision

Each document owns an immediate script queue and a one-shot script timer
queue. A timer is eligible when its monotonic deadline is reached. Immediate
scripts retain FIFO order. Timers run by deadline and then in admission
order for equal deadlines. When both sources are ready, the source that
didn't run in the preceding turn gets priority. With no preceding turn,
immediate scripts have priority.

This is the headless host's selection policy for the two implemented task
sources. The HTML event loop selects a runnable task queue and performs a
microtask checkpoint after running its task. Future Web timer bindings will
define callback registration, nesting and repeat behavior.
[HTML event loop](https://html.spec.whatwg.org/multipage/webappapis.html#event-loop-processing-model),
[HTML timers](https://html.spec.whatwg.org/multipage/timers-and-user-prompts.html#timers)

`into_scripted` uses the base system monotonic clock.
`into_scripted_with_clock` also accepts the existing manual clock and other
host clocks. The clock governs eligibility and wake delays. ECMAScript
civil time remains on the agent's wall-clock path. The host advances a
manual clock explicitly and can ask for the delay to the earliest timer.

The same pending count and UTF-16 source budgets cover both queues.
Cancellation of a pending timer releases its slot and source charge.
A timer identity includes its owning document ID and a non-reusable
sequence. The document's cancellation scope and fatal turn cleanup apply
to both queues.

## Alternatives and consequences

A task queue sorted solely by posting order would let an ineligible timer
block immediate work. Separate ready selection keeps immediate work
runnable and prevents sustained readiness in one source from starving the
other. The timer heap orders deadlines without re-sorting the queue on
each turn. Cancellation searches that heap within the document's configured
task limit.

## Validation and revisit trigger

Tests check deadline boundaries, equal-deadline ordering, the checkpoint
between timer and script turns, readiness fairness, document-scoped
identities, queue limits, overflow and shutdown. Extend the selection
policy when rendering opportunities and page timer APIs join the scheduler.
