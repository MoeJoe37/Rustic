# ADR 0036 — Window timers and animation callbacks in document turns

**Status** Accepted for WEB-04.

## Context

The headless host already offers timer turns and rendering opportunities.
Window callbacks need access to those task sources, the live document and
the same JavaScript agent. Callback registration and cancellation can occur
during another callback or a promise job.
[HTML timers](https://html.spec.whatwg.org/multipage/timers-and-user-prompts.html#timers),
[HTML animation frame callbacks](https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#animation-frames)

## Decision

The page exposes a scheduler interface through Window. The engine keeps
ownership of task ordering, callback handles and retention. Both host and
Window timers share the deadline heap. The pending task count applies to
timers, animation callbacks, queued scripts and input. The UTF-16 budget
also applies to timer source. The agent's host roots retain timer sources,
callback functions and arguments until cancellation or final invocation.
Scheduler borrows end before script or promise jobs run. Dropping a stopped
agent releases all remaining roots.

The host timer API keeps its exact monotonic deadlines and opaque,
document-scoped IDs. A Window timer's delay starts at the end of its
registering turn, so a long turn cannot cause premature delivery. This
keeps borrowed host clocks supported. The engine publishes new deadlines
after the task checkpoint, or after all callbacks and checkpoints of a
rendering turn. Equal deadlines follow registration order. Intervals keep
their ID and admission charge during execution. They repeat only if still
registered at the end of the handler, and rejoin the timer ordering by
their new registration sequence. Nested timer delays follow the HTML
four-millisecond clamp. Interval recurrence starts from the completed turn.

Animation callbacks use the selected opportunity's timestamp in
milliseconds. The pump records the last eligible handle when the frame
starts and pops callbacks in order, so cancellation can remove upcoming
callbacks while reentrant registrations wait for another frame. Each
callback completes its checkpoint before the next callback begins. A frame
shares one safe-point budget across all callbacks and runs the host
renderer only after their work finishes. Ordinary callback exceptions
increment the host's report count and keep the frame running.

TD-132 tracks registration-time clock sampling, lifecycle throttling,
string timer policy, Performance time origins and HTML error reporting.

## Verification

A manual clock tests deadline boundaries, repeat behavior, shared limits,
timer nesting and cancellation. Collection tests cover retained callback
arguments. Rendering tests check callback and job ordering, cancellation
within the same frame, deferral of reentrant registrations and visibility
of document mutations to the renderer.
