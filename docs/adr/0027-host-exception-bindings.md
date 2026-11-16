# ADR 0027 — Realm-owned host exception bindings

Date: 2026-11-16. Status: accepted for WEB-02.

## Context

Rustic's JavaScript runtime supports standard Error objects. Document
scripts additionally need Web IDL's base DOMException interface. The
constructor and DOM failures must keep their UTF-16 string state and their
realm's interface prototype through garbage collection.
[Web IDL exceptions and error names](https://webidl.spec.whatwg.org/#idl-exceptions)
[Web IDL DOMException](https://webidl.spec.whatwg.org/#idl-DOMException)

## Decision

`rustic-bindings` implements the base constructor, its descriptors, the
legacy code table and the conversion from `DomError`. The document script
host installs the interface during realm setup. The standalone ECMAScript
agent keeps its existing global surface.

`rustic-js` provides a host interface installer that publishes the
constructor after its prototype and constants are complete. The heap
charges and traces the prototype entry per realm. The entry remains valid
if script deletes the global constructor or replaces its properties.
A custom `newTarget.prototype` is honored. A non-object prototype falls
back to the target realm's installed interface, with the active interface
as fallback for a standalone ECMAScript realm.

A host exception is an Error object with private traced string slots and
an interface brand. Ordinary properties and prototype changes can affect
property lookup without changing the private state. The ordinary
`Error.prototype.toString` uses JavaScript string concatenation to preserve
lone surrogates in name and message.

DOM validation failures map to named exceptions with fixed engine messages.
JavaScript coercion errors propagate unchanged. Allocation and exhausted
arena identity remain fatal.

## Alternatives and consequences

Looking up the global constructor during an engine throw would let script
intercept the exception path. Storing name and message in a Rust side table
would require a separate finalization route for data already owned by the
JavaScript heap.

The base constructor works before page node bindings because its instance
references stay within the JavaScript heap. Structured cloning, native
stack information and the derived quota exception are tracked in TD-124.

## Validation

Rustic tests cover constructor descriptors and constants, delegation to
Error, UTF-16 through string coercion and formatting, subclassing, forged
receivers, two installed realms, a failed global installation, the
collection of private strings, and a document failure caught in JavaScript.
A document scheduling test verifies installation through the normal host
entry point. Revisit prototype lifetime with realm reclamation.
