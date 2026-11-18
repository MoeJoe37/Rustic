# ADR 0038: Run inline classic scripts during HTML construction

- Date: 2026-11-17
- Status: Accepted for the WEB-05 inline classic slice

## Context

An inline classic needs the partial document and the window agent at its
insertion point. The parser also holds identities that ordinary DOM wrapper
tracing cannot see.

## Decision

An opt-in engine load owns the incremental parser and a single window agent.
The page bindings borrow the parser's own document during the suspension from
[ADR 0037](0037-parser-script-boundary.md). The engine releases its parser
borrow before evaluation, host calls or promise jobs. At EOF the same document
and realm move into the normal turn pump. Default loads still use inert parsing.

Parser-held IDs are roots of the joint collector while construction is active,
including open elements, formatting entries and form, head and select state.
A script may detach or reparent nodes in the same document. Outward adoption
uses the checked cross-document path unless the subtree contains an identity
retained by the parser, a running classic or a pending script. The engine checks
the retained nodes' ancestor chains, including template and shadow hosts, before
either document changes. EOF retires construction roots. Pending script roots
stay live until their work settles. Once a
script can see the tree, parser-driven selected-content replacement detaches old
children so their wrappers remain usable.

At each eligible inline classic, the engine copies bounded UTF-16 source and
invokes the existing evaluator in the same realm. The script identity is a
collection root during evaluation and is exposed through `document.currentScript`.
The engine clears it before the microtask checkpoint. Timers registered during
either phase enter the normal scheduler once that turn ends. Parse, compile and
uncaught JavaScript errors increment the host's bounded error count and leave
later scripts eligible. Fatal limits and cancellation discard the load and
release admission. `document.write` and `document.writeln` raise an explicit
exception.

The engine prepares inline classics using the HTML type and language rules.
External and module loading, script and readiness events, dynamic scripts,
encoding restart and CLI opt-in are subsequent slices.

## Validation

Exercise trees at each input split, edits to the partial document, wrapper
identity before and after EOF, detached parser roots under joint collection,
cross-document adoption of closed and retained nodes, `currentScript` in
evaluation and jobs, recoverable
failures, source and turn limits, and shutdown.

[WHATWG HTML script preparation](https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element)
