# ADR 0039: Host-supplied local external classic scripts

- Date: 2026-11-17
- Status: Accepted for the WEB-05 external classic slice

## Context

The parser and window already share one document and realm. External sources
can arrive out of order, and the headless host owns I/O and its arrival
schedule. The engine needs to decide when each source can run without taking
filesystem or network authority.

## Decision

The embedder opts in with a local document URL and explicit request and byte
limits. The engine resolves `src` against the first effective HTML base or
the document URL. It emits only local file URLs within the document's
directory, checked on normalized URL segments before dispatch. The host
supplies bytes or a failure for each document-scoped request identity.
A filesystem adapter must additionally enforce canonical path containment
when opening the file. The engine performs no filesystem or network I/O.

The parser stops after discovering an ordinary external classic and resumes
after the host completes it and its microtask checkpoint runs. Async classics
execute in completion order, including while a blocker is pending. Defer
classics wait for EOF and execute in parser order. Async takes precedence
over defer. The API exposes the finished document once all admitted classics
have settled. Hosts use the existing monotonic clock for deterministic
completion and timer tests. Readiness and element events remain a later slice.

The request count bounds work even for failed requests. Per-response and
cumulative byte limits bound source handling, and the existing UTF-16,
heap, step and job limits apply at evaluation. A blocked URL, fetch failure,
oversized response or recoverable evaluation error increments the external
script error count and releases the relevant queue position. The source text
inside a script with a `src` attribute is skipped. Pending script elements
remain joint collection roots, including elements detached while waiting.
Stale and foreign completions leave the state intact. Cancellation drops the
load and invalidates its outstanding requests.

This introduces a direct dependency on the existing `rustic-url` crate.
Module graphs, legacy source encodings, integrity and policy, browser
readiness events and CLI opt-in remain separate work.

## Validation

Drive the loader with a manual clock and deliver async and deferred sources in
every permutation. Exercise parser blocking, microtask order, early and late
EOF, detached elements under collection, URL and byte limits, recoverable
failures, cancellation and stale completion.

[WHATWG HTML script preparation](https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element)
