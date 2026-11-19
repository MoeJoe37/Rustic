# ADR 0048 — Error reporting, event handler attributes and the console sink

Date: 2026-11-19
Status: Accepted for WEB-07 (error reporting part of TD-131); implemented

## Context

Until now an exception thrown by a script, listener, timer or animation
callback only incremented a host-side counter, and unhandled promise
rejections were dropped at the checkpoint. Pages (and `testharness.js`)
depend on HTML's error reporting: a cancelable `ErrorEvent` at Window,
`window.onerror` with its five-argument form, `unhandledrejection` and
`rejectionhandled` events, and messages a developer can read. Reporting
also has to stay safe: describing a thrown value must not run page code,
and an error while reporting must not recurse.

`window.onerror` needs event handler IDL attributes, which did not exist.
Content attributes (`onclick="…"`) additionally need compilation with the
element, form and document scopes.

## Decision

1. **Report an exception.** `rustic_bindings::page::report_exception`
   fires a trusted, cancelable `ErrorEvent` (`message`, `filename`,
   `lineno`, `colno`, `error`) at Window. If no handler cancels it, the
   message goes to the console sink as an error. The engine calls it for
   uncaught classic and host scripts, parse and compile failures (as a
   `SyntaxError`), timer and animation callbacks and rejected module
   evaluations; the bindings call it for listener, observer and
   `queueMicrotask` callbacks. A flag turns a report made while reporting
   into a console message only.
2. **Rejections.** After each checkpoint the engine takes the rejection
   tracker's events, roots their promises and queues one task. The task
   fires `unhandledrejection` for each promise still unhandled and
   `rejectionhandled` for a promise reported earlier that gained a handler.
   A handler attached during the event keeps the promise out of the
   outstanding set. The loader runs queued notifications before
   `DOMContentLoaded` and before `readyState` becomes `complete`, since they
   share the DOM manipulation task source. If the task queue is full the
   checkpoint notifies at once.
3. **Describing values.** `Agent::describe_value` formats primitives and
   reads only `name` and `message` data properties of ordinary objects and
   their ordinary prototypes. It never calls getters, `toString` or proxy
   traps.
4. **Event handler IDL attributes.** A subset of `GlobalEventHandlers` is
   on Window, Document and HTMLElement, and Window also has
   `onunhandledrejection` and `onrejectionhandled`. Assigning an object
   appends one listener record to the target's ordered list; the record
   points at a handler entry whose current value is read at dispatch.
   Assigning `null` or a non-object removes the record. A handler's `false`
   return cancels the event. On Window, an `error` ErrorEvent calls the
   handler with five arguments and a `true` return cancels it.
5. **Console sink.** `console.log`, `info`, `warn`, `error`, `debug` and
   `assert` append `ConsoleMessage { level, text, truncated }` to a bounded
   page buffer (1,024 messages of up to 8 KiB each). Further messages
   are counted as dropped; a cut message is marked. Hosts drain it with
   `ScriptedDocument::take_console_messages`; the CLI prints it to standard
   error.

## Alternatives considered

- Firing `unhandledrejection` inside the checkpoint (simpler, and the first
  version did) puts it before tasks it must follow, such as the readiness
  steps of a parsed document (WPT `promise-rejection-event-during-parse`).
- Formatting with `ToString` matches some console output more closely but
  runs page code from the reporter, which an exception during reporting
  could exploit to recurse.
- Content attribute compilation was deferred: it needs a scoped function
  compiler entry point in `rustic-js` and its own review.

## Consequences

- Uncaught errors are now visible to the page. WPT files that used to stop
  silently at a missing API now report a harness error, which exposed and
  led to the `Node` members added alongside this change.
- `filename` is empty and `lineno`/`colno` are zero, `location` does not
  exist, console format specifiers are not interpreted, and only the listed
  handler names exist ([TD-152](../TECH_DEBT.md#td-152--error-reports-lack-source-locations-and-console-formatting),
  [TD-153](../TECH_DEBT.md#td-153--event-handler-content-attributes-and-remaining-handler-names)).

## Validation

`crates/rustic-bindings/tests/errors.rs` covers handler arguments, return
values and cancellation, recursion, side-effect-free description, the
console bound, rejection events and their constructors, and handler
ordering. `crates/rustic-engine/tests/error_reporting.rs` checks script,
syntax, timer and job-rejection reports in real turns. Upstream files for
`window.onerror`, runtime and compile errors, rejection events,
`queueMicrotask` and the event handler processing algorithm are in the
pinned Window selection.

## Revisit

When script URLs and line numbers are tracked (M6 navigation), when
content attributes are compiled, or when a DevTools protocol (DEV-01)
replaces the console sink.
