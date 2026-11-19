# ADR 0045 — Live computed-style reads in scripted documents

Date: 2026-11-18
Status: Accepted design for WEB-06; initial read-only method slice implemented, gate pending

## Context

At the time of this decision, WEB-06 gave scripts `offsetWidth` and
`getBoundingClientRect()` through one bounded geometry snapshot. The CLI
constructs these values from the same DOM, CSS, viewport, images and font
inputs as its CPU renderer. `getComputedStyle()` was still absent. Returning
a plain copied map of CSS values would be misleading: computed-style
declarations are read-only but live, and a script can hold one across DOM
mutations, microtask checkpoints and resource deliveries.

The page bindings must not run the cascade independently of the renderer.
That would introduce a second notion of applied style and bypass the
existing per-turn flush limit. Detached nodes and declarations held across
garbage collections also need the same ownership rules as existing DOM
wrappers.

## Decision

1. The shared, document-local layout snapshot will contain resolved
   declarations for the supported CSS longhands and custom properties,
   constructed from the existing `rustic-style` cascade with the same
   viewport, stylesheets, font and resource inputs as the geometry.
   The HTML/CSS source pipeline remains authoritative; page bindings do
   not parse `style` attributes or invoke a second cascade.

2. `Window.getComputedStyle(element, pseudoElement)` returns a read-only
   `CSSStyleDeclaration` backed by the element wrapper, not a copied
   object of strings. Its `getPropertyValue`, indexed access, `item`,
   `length` and supported camel-cased longhand getters read the current
   snapshot when accessed, so an object held across script turns reflects
   mutations. Unrecognized properties return the empty string; custom
   property names preserve their case. Only computed values for supported
   properties may be exposed. Explicit pseudo-element support must have
   a real selector/cascade source before it is exposed.

3. The declaration-to-element association uses a GC-traced private weak
   map keyed by the declaration object with the element wrapper as its
   value. A declaration retained by script keeps its element reachable;
   an unreachable declaration does not keep the element alive. Lookups
   validate the wrapper and document identity after any adoption. No
   global, permanently growing `NodeId` registry is permitted.

4. All argument conversions, including a potentially user-defined
   `ToString` for the property name, finish before a DOM/layout borrow.
   The read then uses `LayoutReads` and its existing
   `(document mutation revision, applied inputs revision)` key. A
   repeated same-revision read incurs no new flush; a dirty read shares
   the eight-flush-per-turn budget with geometry, including jobs. The
   snapshot is an owned value when returned to JavaScript, with no live
   Rust borrow across callbacks.

5. A host must publish stylesheet, image, font and viewport **completion**
   through the rendering-input revision when the completed result enters
   the document pump. The host must not publish an uncommitted fetch result
   or silently change the resources underneath a cached snapshot. A
   relevant completion also invalidates an outstanding frame ticket.
   Local fixture loading remains inside the existing file grant; this
   decision does not add network access.

6. Unavailable style/layout data reports the established
   `InvalidStateError`; exhausting dirty flush attempts reports
   `QuotaExceededError`. Do not expose a CSSOM method before these
   semantics exist merely to pass property-exposure checks.

## Alternatives rejected

- **Copied `getComputedStyle` maps:** cheap to create but go stale while
  the JavaScript object remains reachable.
- **A separate style-only flush:** could disagree with geometry and evade
  the shared budget.
- **A strong native `NodeId` map:** would leak detached trees and break
  wrapper identity after adoption.
- **Publishing resource arrival before application:** would invalidate
  frames without guaranteeing the next layout read sees the new data.

## Validation and completion

WEB-06's implementation must exercise:

- The same held declaration before and after attribute, text, subtree
  and stylesheet mutations, including changes from promise jobs.
- Property serialization for every exposed longhand; custom properties,
  inherited and initial values; unknown properties; missing boxes; and
  supported pseudo-elements once implemented.
- `offsetWidth`, `getBoundingClientRect` and computed-style reads
  sharing one snapshot and one budget (including a ninth dirty read).
- Arrival/application ordering for linked stylesheets and images, and
  rejection of old frame tickets on resource completion.
- Cross-document isolation, adoption, detached-node collection and
  callback reentry during argument conversion.
- A pinned WPT expectation update rather than a property-presence-only
  claim. No capability completion credit is assigned by this ADR alone.

Revisit when WEB-06 introduces a separate incremental layout cache (M9),
or when CSSOM pseudo-element and stylesheet mutation support requires
additional snapshot inputs.

## Implementation status (2026-11-18)

The first implementation exposes `getComputedStyle(element)` and a live,
read-only `CSSStyleDeclaration` with `getPropertyValue`,
`getPropertyPriority`, `item`, `length` and an empty `cssText` getter.
`setProperty`, `removeProperty` and `cssText` assignment throw
`NoModificationAllowedError`. Generated prototype getters expose all supported
longhands in canonical CSS spelling and camelCase, with the `cssFloat` alias;
they are live and reject assignment. The property catalog and getter table
have matching compile-time lengths. Numeric indexed properties use the
runtime's `LiveIndexAccess` with live own keys and read-only descriptors;
index, `item`, named getter and geometry reads share the same flush quota.
A connected declaration without an installed renderer reports `InvalidStateError`, as does a connected element missing
from its host's computed-style entries; detached declarations return empty
values. Supported longhands and custom properties come from the same CLI cascade pass that builds layout;
reads share the existing revision key and eight-flush budget. Declarations
hold their element wrappers through the page's rooted weak map and the
indexed host object's GC-traced source. Nonempty
pseudo-element arguments fail explicitly rather than return fabricated styles.

The host-facing `apply_resource_completion` API publishes a staged resource
result before advancing the applied-input epoch and offering a frame. Failed
applications leave the epoch and frame ticket unchanged. A dedicated engine
fixture tests both stylesheet and image changes against geometry and held
computed style in the same snapshot. This is not yet wired to the local CLI's
synchronous resource loader, nor does it implement asynchronous fetch tasks.

This is **not** the full decision: CSSOM serialization edge cases,
pseudo-elements, DOM adoption/GC stress,
WPT expansion, immutable applied-resource snapshots and real stylesheet/image
completion delivery still need implementation and passing tests. Until those arrive WEB-06 remains open.
