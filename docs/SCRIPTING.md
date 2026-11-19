# Scripted documents (M5)

This is the guide to how JavaScript and a document meet in Rustic. It ties
together the decision records for M5 ([ADR 0020](adr/0020-document-lifecycle.md)
to [ADR 0042](adr/0042-scripted-render-state.md)), which each
describe one step, and says plainly what is not built yet. The tested contract
of every finished unit is in [IMPLEMENTATION.md](IMPLEMENTATION.md) under
WEB-01 to WEB-05; the plan for the rest is
[ROADMAP §8](ROADMAP.md#8-m5--scripted-document-integration).

## What works today

A host can:

1. Parse a local HTML document through `Engine::begin_document`.
2. Turn the finished document into a `ScriptedDocument`, which owns one
   JavaScript agent with a `window` and a live `document`.
3. Queue scripts, trusted clicks and key presses, and timers, and offer
   rendering opportunities.
4. Pump the document one turn at a time with `run_next` or
   `run_next_with_render`, on a real or a manual clock.

A host can also use `Engine::begin_inline_scripted_document` to opt into
running parser-inserted inline classics. The parser pauses at each script.
The script sees markup already processed, can edit it, and completes its
promise checkpoint before the parser advances. At EOF the host receives
the same document and realm with any queued timers or animation callbacks.

Hosts can opt into external classics through
`Engine::begin_local_scripted_document`. Feed HTML, take available
`next_request()` values, and pass each result to `complete_script()`.
Call `close_input()` when the HTML ends and keep serving requests until
`take_document()` returns the document. A blocking script can pause discovery
of later requests. Async runs on arrival. Defer waits for EOF and preceding
defer scripts. The host opens only a requested local file within its canonical
grant and honors the request's byte limit.
[ADR 0039](adr/0039-local-external-classics.md)

Local `type="module"` elements use the same host API. External entry points
and static imports appear as `ScriptResourceKind::JavaScriptModule` requests.
Deliver dependency results as they appear, and call the load's `run_next()`
when a timer awaited by module code becomes due. Async graphs can start as
soon as their dependencies settle. Other modules start in document order
with deferred classics after EOF. Module exceptions and failed static graphs
are counted by `take_module_script_error_count()`.
[ADR 0040](adr/0040-local-module-scripts.md)

`document.readyState` starts at `loading`, becomes `interactive` before
deferred scripts start, and becomes `complete` before Window `load`.
`DOMContentLoaded` bubbles from Document once the deferred script queue
has drained. Pending async sources and module evaluations can delay
Window `load`. External script elements receive a trusted `load` when their
source is acquired and evaluation starts, or an `error` on acquisition
or module graph failure.
[ADR 0041](adr/0041-scripted-document-readiness.md)

Use `rustic render --run-scripts --output page.png page.html` to render a
scripted page. The option also works for dumps, hit testing and reftests.
The CLI services local script requests at parser barriers and EOF, advances
the manual clock for up to 256 timer turns and offers one animation frame
before capturing the output. Scripting defaults to off.

Inside a turn, script can walk and change the tree, create nodes and whole
detached documents, query with CSS selectors, hold live collections, add
listeners, dispatch events and schedule timers and animation callbacks.
Reading `Node.textContent` preserves UTF-16 code units. Writing it replaces
children or updates character data and can update a live stylesheet.
`document.body` resolves the current HTML body or frameset. Assignment
uses the checked insertion and adoption path to replace that child.
[HTML DOM tree accessors](https://html.spec.whatwg.org/multipage/dom.html#dom-tree-accessors)
During parsing, scripts can move completed subtrees to another document.
Subtrees that contain parser or pending script identities stay with the
original document while those identities are in use.

The local loader chooses the encoding before it runs author code. A
tentative parse can discover a legacy charset later in the retained prefix
and replay it without executing earlier scripts twice. The Document
encoding getters expose the selected decoder. [ADR 0044](adr/0044-local-html-encoding-replay.md)

With `--run-scripts`, `HTMLElement.offsetWidth` reads the rounded,
untransformed border-fragment width through the same layout pipeline as
the output renderer. `Element.getBoundingClientRect()` returns a new
`DOMRect` with the transformed bounds in the initial viewport. Elements
without layout boxes read as zero. Unprojectable bounds return
`InvalidStateError` in the headless renderer.
[CSSOM View](https://drafts.csswg.org/cssom-view/#dom-htmlelement-offsetwidth)

A script may hold `getComputedStyle(element)` across DOM changes. Its
`CSSStyleDeclaration.getPropertyValue(name)`, `item(index)`, `length`,
numeric indexed properties (`style[0]`, `Object.keys`, property descriptors),
and supported longhand named getters (both CSS spelling and camelCase)
refresh from the same revision-keyed snapshot as geometry and respect the
same flush budget. Custom names remain case-sensitive; unknown names return
an empty value. Nonempty pseudo-element arguments are explicitly unsupported.
If the host has no applied renderer, a connected computed-style read reports
`InvalidStateError` rather than silently fabricating empty values; detached
nodes use the defined empty result. The `cssText` getter is empty, while
`setProperty`, `removeProperty` and `cssText` assignment throw
`NoModificationAllowedError`.
An attached renderer must provide an entry for every connected element it can
measure: if a connected element is omitted from the computed-style snapshot,
style reads throw `InvalidStateError`, rather than silently returning an
empty declaration. Detached elements still read as empty.
The CLI derives properties from the renderer's existing style cascade, not
from a separate binding-side parser. Hosts may publish a staged stylesheet,
image or font completion through `ScriptedDocument::apply_resource_completion`:
a successful application bumps the applied-input revision, invalidates old
frame tickets and offers the next rendering opportunity; a rejected completion
does none of those. The supplied closure must apply only a fully validated
result and must not run JavaScript. The scripted local CLI now keeps a document-scoped, bounded snapshot of
stylesheet and image bytes as they are first fetched. Later DOM-triggered
layout flushes reuse those bytes even if the underlying files change or are
removed; missing and rejected resources remain missing within that snapshot.
The local fetcher can also stage a replacement using the normal file-security
policy; the applied cache rechecks its entry and byte budgets when the host
commits it through `apply_resource_completion`. Per-URL versions reject late
stale completions without blocking updates to unrelated resources. A
completion published while an initial file fetch is running also takes
precedence over that fetch's older bytes. A CLI regression fixture
checks staged CSS and PNG replacements, layout values, and stale frame tickets.
There is **no automatic asynchronous resource delivery** or parser task
integration yet. Static dump commands still read local files directly. This is a partial CSSOM surface and its
regressions have not yet been run on CI. [ADR 0045](adr/0045-live-computed-style-reads.md)

What does **not** work yet:

| Missing | Consequence today | Planned in |
| --- | --- | --- |
| Remaining CSSOM and resource ordering | Live `getComputedStyle` has read-only methods, named and numeric indexed properties and `cssText`, but pseudo-elements, CSSOM serialization details and wiring real stylesheet/image completions into the page pump remain open ([ADR 0045](adr/0045-live-computed-style-reads.md)). | WEB-06 |
| Mutation observers, `ResizeObserver`, `IntersectionObserver` | Not exposed | WEB-07 |
| Focus, text input, form submission and validation | Only button and checkbox activation exist | WEB-07 |
| Error events, `unhandledrejection`, console | Errors are counted for the host, not reported to script | WEB-07 |
| Frames, navigation, `location`, `history` | One top-level document per agent (TD-130) | M6 |
| Network, storage, workers | Not present | M6, M9 |
| Expanded WPT coverage | The initial Window baseline covers nodes, events, collections, timers and animation callbacks. New APIs need their own coverage as they land. | WEB-09 |

A property that is missing is absent, not stubbed: `typeof window.fetch` is
`"undefined"`. That rule is part of the M5 gate.

## Crates

| Crate | Role here |
| --- | --- |
| `rustic-js` | The agent, heap, interpreter and job queue. It knows nothing about the DOM; it offers host objects, an execution-host hook at safe points and a native work budget |
| `rustic-dom` | The tree. It knows nothing about JavaScript; it exposes checked mutation and marking for collection |
| `rustic-bindings` | Everything that knows both: Web IDL conversions (`webidl`), the wrapper table and joint collector (`heap`), and the page interfaces (`page`: window, documents, creation, attributes, character data, selectors, collections, events) |
| `rustic-engine` | The host-facing owner: document admission and shutdown (`lifecycle`), the turn pump (`scripting`), timers and task sources (`tasks`, `window_tasks`) and frame tickets (`frames`) |

## Ownership and lifetime

- The `Engine` admits a bounded number of documents and gives each a
  cancellation token. `Engine::shutdown` cancels all of them and closes
  admission ([ADR 0020](adr/0020-document-lifecycle.md)).
- A `ScriptedDocument` owns the parsed tree, one agent and the pending work.
  The tree is shared between the bindings and the renderer through a checked
  borrow that is held for one binding operation and released before any
  script callback runs, so script that reenters the DOM from a getter or
  listener cannot hit a borrow failure
  ([ADR 0028](adr/0028-scripted-document-collection.md)).
- Every node has at most one wrapper per realm, so `a === b` holds for the
  same node, and properties script puts on a wrapper survive as long as the
  node is reachable ([ADR 0026](adr/0026-realm-dom-wrappers.md)).
- **One collector traces both graphs.** At a safe point the joint collector
  marks from the agent's roots and from every registered document. A
  reachable wrapper keeps its node's whole tree alive; a node keeps its
  wrappers and listeners alive. A detached tree that only script holds stays;
  a detached tree in a cycle with its own listeners is freed. There is no
  JavaScript-only fallback: if joint collection fails, the turn fails.
- Removing a node detaches it; destruction happens only when the collector
  finds the tree unreachable. Adoption into another document keeps wrapper
  identity, expandos, listeners and live collections
  ([ADR 0033](adr/0033-detached-documents-and-adoption.md)).
- During a classic load, the parser's pending node identities, pending
  external scripts and the current script remain visible to collection.
  Outward adoption raises `NotSupportedError` until the load settles.
  Same-document edits and inward adoption
  are available throughout construction. [ADR 0038](adr/0038-parser-inserted-inline-classics.md)
- A fatal stop (cancellation, heap exhaustion, a turn over budget) drops the
  agent and all pending work and invalidates outstanding frame tickets. The
  tree stays readable for inspection. Script cannot catch a fatal stop.

## Turns

All page-visible ordering is decided in one place, the pump in
`rustic-engine`. Nothing runs between the host's calls.

| Source | Queued by | Order |
| --- | --- | --- |
| Immediate tasks | `queue_script`, `queue_input` | One FIFO shared by scripts and input |
| Timers | `schedule_script` (host), `setTimeout`/`setInterval` (script) | By deadline on the injected monotonic clock; ties by admission order |
| Rendering | `offer_rendering_opportunity` | Offers coalesce to the latest timestamp |

`run_next` alternates between ready immediate work and ready timers so neither
starves the other; `run_next_with_render` also alternates rendering with
script work. Each turn ends with a microtask checkpoint that drains promise
jobs, including jobs queued by jobs, under a job-count limit
([ADR 0021](adr/0021-document-script-turns.md),
[ADR 0022](adr/0022-document-task-sources-and-timers.md)).

A rendering turn runs the due `requestAnimationFrame` callbacks first, each
followed by its own checkpoint, then calls the host's render function and
returns its output with a frame ticket. `accept_rendered_frame` tells the host
whether that output is still the newest for its document, so a late frame is
never presented over a newer one
([ADR 0023](adr/0023-document-rendering-turns.md),
[ADR 0036](adr/0036-window-timers-and-animation-callbacks.md)). DOM mutations
and applied rendering-input changes can also invalidate a frame.
`rendering_inputs_changed` publishes the latter at a host turn boundary.

A script layout read reuses its snapshot while the DOM and rendering-input
revisions match. A turn can flush up to eight dirty snapshots, including
reads made during its promise jobs. An additional dirty read throws
`QuotaExceededError`. The host can observe the number of flush attempts
through `take_layout_flush_count`.

Window timers follow the HTML rules that tests can observe: a delay starts
when the registering turn completes, and nested timers are clamped to four
milliseconds after six levels. Remaining differences are TD-132.

### Limits

Every queue and every turn is bounded, and a rejected admission changes
nothing. `TaskLimits::default()` is:

| Limit | Default | Exceeding it |
| --- | --- | --- |
| Pending tasks (scripts, input and timers together) | 128 | The queue call returns an error |
| Pending source text and each parser-inserted inline classic | 512 × 1024 UTF-16 code units | Admission fails or the inline load stops |
| External classic preparation attempts | 64 per document | Later requests fail and parsing continues |
| External classic response bytes | 512 KiB each, 2 MiB across the load | The result fails and its queue position is released |
| Safe points per turn (script and its jobs share them) | 1,000,000 | Fatal stop |
| Jobs per microtask checkpoint | 4,096 | Fatal stop |
| JavaScript heap | The agent default | Fatal stop |
| Live detached documents per agent | 16 | Creating another one fails |

Native work that script can make arbitrarily large (selector matching,
subtree copies, collection scans, event path collection) is charged to the
same turn budget, so a single call cannot run unmetered.

## Bindings

- **Conversions** ([ADR 0024](adr/0024-web-idl-conversions.md)). Web IDL
  string and 32-bit integer conversions reuse the engine's `ToString` and
  `ToNumber`, keep their input rooted while user code runs, and pass user
  exceptions through unchanged. All of an operation's arguments are converted
  before the tree is touched, even when a conversion calls back into the DOM
  ([ADR 0029](adr/0029-scripted-node-creation.md)).
- **Generated metadata** ([ADR 0025](adr/0025-pinned-dom-idl-metadata.md)).
  `tools/generate_dom_idl.py` reads the pinned DOM Standard IDL and emits
  metadata for `CharacterData`, `Text`, `CDATASection` and `Comment`. The
  other interfaces are bound by hand (TD-126). Extending the generator before
  binding each new interface family is the plan, because hand-written
  conversions are where bindings usually go wrong.
- **Exceptions** ([ADR 0027](adr/0027-host-exception-bindings.md)). DOM
  failures become `DOMException` objects of the right name. Running out of
  memory or identities stays fatal. Extras are TD-124.
- **Interfaces exposed.** `EventTarget`, `Node`, `Document`, `DocumentFragment`,
  `DocumentType`, `Element`, `CharacterData`, `Text`, `CDATASection`, `Comment`,
  `ProcessingInstruction`, `XMLDocument`, `DOMImplementation`, `NodeList`,
  `HTMLCollection`, `Window`, `Event`, `CustomEvent`, `UIEvent`, `MouseEvent`,
  `PointerEvent`, `KeyboardEvent`, `DOMException` and the checkbox state of
  `HTMLInputElement`. IMPLEMENTATION.md lists the members of each; treat that
  list, not this one, as the contract.
- **Selectors** ([ADR 0030](adr/0030-dom-selectors-and-node-lists.md)).
  `querySelector`, `querySelectorAll`, `matches` and `closest` use the same
  matcher as the cascade, so a selector means the same thing to script and to
  style.
- **Window** ([ADR 0032](adr/0032-top-level-window-realm.md)). The global
  `this` is a `WindowProxy` distinct from the `Window` that stores global
  properties, as navigation will later require.
- **Classics.** `document.currentScript` identifies the element
  during evaluation and returns null during promise jobs. The host can clear
  the recoverable-error counts with `take_inline_script_error_count` and
  `take_external_script_error_count`.
  HTML `document.write` and `document.writeln` raise `NotSupportedError`.

## Events and input

Dispatch collects the path before any listener runs, snapshots the listeners
of each target at the start of each phase, and honors `capture`, `once`,
`passive`, `stopPropagation`, `stopImmediatePropagation` and `preventDefault`.
A listener that throws does not stop the dispatch; the host can read how many
did ([ADR 0034](adr/0034-dom-event-dispatch.md)).

`document.createEvent()` supports the aliases for Event, CustomEvent, UIEvent,
MouseEvent and KeyboardEvent. The returned event must be initialized before
dispatch. The headless host exposes its active document as visible, and
detached documents are hidden. [DOM event factory](https://dom.spec.whatwg.org/#dom-document-createevent)
[HTML page visibility](https://html.spec.whatwg.org/multipage/interaction.html#page-visibility)

Trusted input comes only from the host, which names a target node it has
hit-tested: a primary click or a key press or release. The target is checked
again at delivery and the task is dropped if the node left the document.
Events that script creates or redispatches are never trusted. Trust records
where an event came from; it is not yet a permission signal
([ADR 0035](adr/0035-trusted-input-and-activation.md)). Activation behavior
exists for buttons and checkboxes, including keyboard activation and the
checkbox's cancel-and-restore rule. Shadow-tree retargeting, more input types
and `AbortSignal` listener removal are TD-131.

## Evidence

| Area | Tests |
| --- | --- |
| Turn order, checkpoints, budgets, shutdown | `crates/rustic-engine/tests/scripting.rs`, `lifecycle.rs` |
| Timers and clamping | `crates/rustic-engine/tests/timers.rs` |
| Rendering turns, animation callbacks, frame tickets and layout flushes | `crates/rustic-engine/tests/frames.rs` |
| Script reads of CLI layout geometry | `crates/rustic-cli/src/loader.rs` |
| HTML script suspension and scripting-enabled trees | `crates/rustic-html/tests/document.rs`, `fragment.rs`, `tree_construction.rs` |
| Parser-inserted inline classic execution | `crates/rustic-engine/tests/inline_scripts.rs` |
| Local external script results and execution order | `crates/rustic-engine/tests/classic_scripts.rs` |
| Local modules, shared imports and top-level await | `crates/rustic-engine/tests/module_scripts.rs` |
| Trusted input and activation | `crates/rustic-engine/tests/input.rs` |
| Conversions and generated metadata | `crates/rustic-bindings/tests/webidl.rs`, `metadata.rs` |
| Wrapper identity, adoption, joint collection | `crates/rustic-bindings/tests/dom_gc.rs`, `page.rs` |
| Events | `crates/rustic-bindings/tests/events.rs` |
| `DOMException` | `crates/rustic-bindings/tests/dom_exception.rs` |

These tests use a manual clock and run on CI tiers including the 32-bit and
big-endian ones.

The smoke job runs two scripted targets. `script-event-pump` checks trusted
bubbling input, checkpoint order, timer order and cancellation against a
Rust oracle. `script-mutation-render` checks frame invalidation, then compares
the updated pixels to an independently parsed document containing the expected
markup. Both targets replay 128 deterministic seeds before mutations.
The scripted reftests cover the counter, timer, event bubbling and stylesheet
and tree updates at device scales one and two. The form-validation fixture
depends on WEB-07.

### Pinned Window web-platform-tests

The headless CLI runs upstream `testharness.js` unchanged at WPT revision
`cb97c571c3b1947e08b8615eeb4028a2794b23a4`. The selection covers
`dom/nodes`, `dom/events`, `dom/collections`, `html/webappapis/timers`
`html/webappapis/animation-frames` and `css/cssom-view`. Linux CI is
configured to require an exact match to the checked-in **21 files,
78 subtests, 67 passes and 11 expected failures**. The expectations record the names and diagnoses
so changes in either direction require review.
[Manifest and digests](../crates/rustic-cli/tests/fixtures/wpt-web/manifest.json)
[Exact results](../crates/rustic-cli/tests/fixtures/wpt-web/expectations.json)

The adapter maps the two standard harness URLs to local resources and
wraps selected `.any.js` files in their Window variant. A completion
callback records results for the runner. The CLI supplies the manual
clock and one rendering opportunity. The candidate
`ParentNode-querySelectorAll-removed-elements.html` depends on
`innerHTML`, and `clearinterval-from-callback.any.js` leaves an interval
running after completion. Both remain outside the initial selection.
The CSSOM View cases cover detached elements, fixed positioning with a
transform and rectangle dimensions. The two new expected failures concern
the coordinates of an empty inline in a phantom line box.

```sh
cargo build --locked -p rustic-cli
python3 tools/run_wpt_web_tests.py --rustic target/debug/rustic
```

## Open debt

| Entry | What | Due |
| --- | --- | --- |
| TD-31 | Form owners ignore the parser's form pointer | WEB-07 |
| TD-118 | A realm is never freed while its agent lives | Before frames (M6) |
| TD-124 | `DOMException` extras | M9 |
| TD-126 | Hand-bound subset of interfaces | Each new interface family |
| TD-127 | UTF-8 storage for element and attribute names | M9 |
| TD-129 | Collection reads scan live nodes | First profile that shows it |
| TD-130 | One top-level Window | M6 navigation |
| TD-131 | Extended event delivery and error reporting | WEB-07, M9 |
| TD-132 | Timer and frame integration details | WEB-06, WEB-07 |
| TD-133 | Adoption of retained parser nodes and dynamic markup | M9 HTML lifecycle |
| TD-134 | Classic-script ordering with stylesheet and timer tasks | WEB-06 |
| TD-135 | Dynamic page imports, import maps and data module types | M6 and M9 |

See [TECH_DEBT.md](TECH_DEBT.md) for each entry.

## Adding an interface

1. Find the interface in the pinned IDL; extend `tools/generate_dom_idl.py`
   if its family is not generated yet, and commit the regenerated metadata.
2. Bind it in `rustic-bindings::page`. Convert every argument first, borrow
   the tree for one operation, and release the borrow before calling script.
3. Charge any loop whose length script controls to the native work budget.
4. Make every failure atomic: validate, reserve, then commit.
5. Test identity, reentrancy from a getter, failure atomicity, collection of
   a detached cycle, and the exact budget boundary.
6. Add the members to IMPLEMENTATION.md, update this file's interface list,
   and record anything left out as a TD entry.
