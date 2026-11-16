# ADR 0031 — Live HTMLCollections and direct-child indexing

**Status** Accepted for WEB-03.

## Context

The [DOM Standard's collections section](https://dom.spec.whatwg.org/#interface-htmlcollection)
defines live element views with indexed and named reads. The applicable
[Web IDL rules](https://webidl.spec.whatwg.org/#legacy-platform-objects)
require named properties to respect ordinary own and inherited properties.
The collection must remain live on a detached subtree and while prototype
lookup executes JavaScript.

## Decision

Use the existing indexed host object for both collection interfaces.
`children` retains the source wrapper and uses the same per-node and
per-realm identity cache as `childNodes`. A descendant HTMLCollection
retains the source wrapper and its UTF-16 filter parameters through a
private traced JS object. Both follow the light DOM in tree order and use
the same node wrapper cache as other DOM reads.

Store the current direct-child root and its node and element IDs in a
single page-local index. Rebuild after a child-link edit or a change of
root. Drop the cached IDs when their root is collected. The revision
saturates and disables caching rather than allowing a stale value to
compare equal. These native IDs carry no tracing edges.

The DOM host supplies names and the runtime applies Web IDL visibility.
Native queries finish their DOM borrows before any Proxy prototype trap.
After visibility checks, the value lookup reads the current DOM. Names in
flight during enumeration are held as GC roots across prototype queries.
Name properties are read-only, non-enumerable, and configurable. The
`namedItem` method checks the collection directly.

## Verification

Binding tests cover tree edits, detached ownership, UTF-16 names, qualified
names and namespaces, class filters, property visibility, and prototype trap
reentry under GC stress. The engine test checks class matching in quirks and
standards modes. Filtered descendant reads and name lookup still perform
metered scans (TD-129).
