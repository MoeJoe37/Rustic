# ADR 0032 — Top-level Window and global realm identity

**Status** Accepted for WEB-03.

## Context

The scripting host attaches the original parsed tree to one JavaScript
agent. The [HTML Window and WindowProxy algorithms](https://html.spec.whatwg.org/multipage/nav-history-apis.html#the-window-object)
distinguish the Window global object from the script-facing browsing
context handle. ECMAScript's global environment already stores its global
object and its global this binding separately.

## Decision

Keep global bindings and intrinsics on the same ordinary Window object.
Install a traced WindowProxy as the realm's global this binding and the
initial value of `globalThis`. Scripts, eval and sloppy function calls
read the binding from the realm, so replacing the property cannot alter
`this`. Global identifier accessors receive the proxy as their receiver.
Standalone realms set both environment fields to the ordinary global.

The proxy's same-realm operations forward ordinary property storage,
preserve the receiver, reserve array indices for child contexts, and
keep the prototype fixed and the proxy extensible. The current target
and the proxy use the same fixed prototype, so ordinary prototype cycle
checks can follow this link. Window attributes are own properties of the
global object. Its document accessor returns the original document, and
`Document.defaultView` returns the realm's proxy.

The proxy and the ordinary global are traced through the global
environment. The rooted document's wrapper stays live through the
existing joint collector. Related contexts, origin-sensitive access,
navigation, Window named access and EventTarget inheritance are TD-130.

## Verification

Binding tests cover property descriptors, receiver and brand rules,
global script and eval semantics, replaceable attributes, realm
isolation, and liveness under collection stress. An engine test checks
that the window remains stable through promise jobs and separate
document turns while mutating the original parsed tree.
