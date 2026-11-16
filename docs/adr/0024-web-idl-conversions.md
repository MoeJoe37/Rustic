# ADR 0024 — Web IDL conversions at the host boundary

Date: 2026-11-16
Status: Accepted for the initial WEB-02 conversion layer

## Context

Upcoming DOM bindings need consistent conversion of JavaScript arguments
before performing DOM operations. Reading an object's conversion methods
can execute JavaScript, including getters that throw or reenter the host.
The DOM stores UTF-16 code units, while different IDL string types have
different requirements for those units.

## Decision

`rustic-bindings::webidl` uses the agent's existing `ToString` and
`ToNumber` operations. It roots the input object across coercion and releases
the root on either result. Callers root any other retained JavaScript values
and finish coercion before borrowing the DOM for mutation. User exceptions
propagate with their original value. IDL validation failures use the agent's
TypeError constructor, and failed host buffer reservations return the
runtime's out-of-memory completion.

`DOMString` makes an owned UTF-16 copy with all code units preserved. It
supports nullable and legacy null-to-empty conversion as separate
policies. `USVString` produces scalar text with a replacement character
for each unpaired surrogate. `ByteString` checks that each code unit fits
in a byte. `long` and `unsigned long` share 32-bit wrapping, clamping with
ties to even, and range enforcement.
[Web IDL ECMAScript binding](https://webidl.spec.whatwg.org/#ecmascript-binding)

## Alternatives and consequences

Rust numeric casts by themselves saturate out-of-range inputs. Using the
runtime's modulo operations for wrapping keeps those calls consistent with
ECMAScript. The explicit string policies keep the choice of replacement
at the binding boundary. Returning owned text releases the JavaScript heap
borrow before the caller reaches the DOM.

## Validation and revisit trigger

Tests cover string code units, nullable and legacy null behavior, integer
boundaries, catchable TypeErrors and exception identity through native
JavaScript calls. A host getter also triggers a collection before coercion
falls back to the same object's second conversion method. Revisit these
contracts when the subset generator emits argument conversions or when
realm-aware DOM wrappers and DOMException bindings are installed.
