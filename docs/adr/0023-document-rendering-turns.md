# ADR 0023 — Document rendering turns

Date: 2026-11-16
Status: Accepted for WEB-01 rendering opportunities

## Context

The document pump already runs script and timer tasks with a microtask
checkpoint. A host also needs to deliver rendering opportunities and
order their outputs without embedding a display server in the engine.

## Decision

The host offers a rendering opportunity at its clock's current time. One
pending slot per document coalesces multiple offers to the most recent
timestamp. The renderer-aware pump alternates between ready script work
and rendering when both are available. With no preceding turn, script
work has priority. Each selected script or timer completes its checkpoint
before the pump can select a frame.

The host supplies the rendering function and receives the output and a
ticket. A ticket carries the owning document, a checked increasing
sequence and the opportunity timestamp. Before presentation, the host
can accept the most recently started frame once. Starting a newer frame
supersedes earlier tickets. A queued opportunity leaves an outstanding
ticket valid until that opportunity starts. Shutdown clears pending and
outstanding frames at the next safe point. If cancellation occurs during
the rendering call, the pump drops the output before returning.

HTML models rendering updates as work on a rendering task source and
allows queued opportunities to coalesce. Rustic's headless host chooses
the timing and uses a deterministic ordering policy for the supported
sources.
[HTML Standard](https://html.spec.whatwg.org/multipage/webappapis.html#update-the-rendering)

## Alternatives and consequences

An engine-owned display loop would couple document execution to a
particular platform. Passing the renderer through the pump places the
ordering contract in the engine while the host chooses the rasterization
and presentation path. The script-only pump remains available to hosts
that only drive scripts and timers.
[ADR 0036](0036-window-timers-and-animation-callbacks.md) adds page animation
callbacks before rendering. WEB-06 covers layout invalidation and resource
completion ordering.

## Validation and revisit trigger

Engine tests cover timestamp coalescing, script and timer checkpoints,
interleaving, cross-document and superseded tickets, sequence exhaustion,
and cancellation during rendering. The headless CPU renderer produces
the same PNG through the scheduled turn as through the direct path.
Revisit the interface when WEB-06 adds layout and resource epochs.
