# ADR 0021 — Document script turns

Date: 2026-11-16
Status: Accepted for the initial WEB-01 scheduler

## Context

The ECMAScript runtime owns the promise-job queue. The headless engine owns
document lifetimes and cancellation. WEB-01 needs a host turn that joins these
two contracts before parser-driven script loading and Web API callbacks.

## Decision

The host converts an inert `EngineDocument` into a `ScriptedDocument`.
The new owner holds one agent and one FIFO queue of host-supplied UTF-16
scripts. The engine reuses the document's cancellation token. The pending
task count, pending source units, JS heap, VM safe points per turn and jobs
per checkpoint have independent limits. The agent runs in non-blocking mode.

After each script task the host turn drains promise jobs, including jobs that
those jobs enqueue. Ordinary script errors still reach the checkpoint. The
turn shares one VM safe-point budget across script and jobs and returns a
failure category for a top-level thrown value. Stopping on cancellation,
heap exhaustion or a turn budget discards queued scripts and drops the agent.
An unfinished checkpoint ends script execution for this document, which
prevents a later task from running ahead of its pending microtasks. The
checkpoint order follows the HTML event-loop model.
[HTML Standard](https://html.spec.whatwg.org/multipage/webappapis.html#perform-a-microtask-checkpoint)

The host owns admission and calls `run_next` at a safe point. The parser and
CLI keep their inert route. Task sources, timers and the host clock, rendering
opportunities, parser-scheduled scripts, and dispatch of rejection events
remain follow-up work.

## Alternatives and consequences

Keeping the agent in a separate handle would let the handle outlive its
document owner. The explicit conversion keeps both lifetimes together and
leaves existing users of inert documents with the same type. An initial
single-source FIFO is sufficient for supplied scripts. Additional task
sources will need an explicit readiness and selection policy.

## Validation and revisit trigger

Integration tests cover FIFO order, nested checkpoints, ordinary exceptions,
admission limits, VM and checkpoint budgets, and engine shutdown. Extend
this decision when WEB-01 adds sources and clocks or when WEB-05 gives the
parser authority to schedule script execution.
