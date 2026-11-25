# Scripted documents and Window APIs

This guide describes how a document, its JavaScript agent and the host work
together. The original M5 integration is recorded in
[ADRs 0020–0042](adr/README.md). Later milestones added network and storage
services, a live desktop host and more Window APIs. The
[implementation ledger](IMPLEMENTATION.md) records tested contracts and the
[roadmap](ROADMAP.md#12-m9--compatibility-expansion-program) tracks compatibility work.

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
an empty value. Indexed reads, `item()` and JavaScript iteration list supported
longhands in lexicographical order, followed by custom properties.
For rendered boxes whose sizing properties apply, `width` and `height` use the
untransformed content or border dimensions selected by `box-sizing`. Ordinary
inlines and elements without boxes expose their computed dimensions. A unitless
`line-height` resolves against the element's computed font size, and `normal`
remains a keyword. Block and atomic inline boxes also expose resolved margins
and padding. Percentages use the layout containing block, and auto block
margins reflect distribution by the layout algorithm. `getPropertyValue()`
combines the four sides of `margin`, `padding`, `border-width`, `border-style`
and `border-color` into the shortest equivalent shorthand.
[CSSOM §9](https://drafts.csswg.org/cssom/#resolved-values)
`getComputedStyle` uses the element for a pseudo argument without a leading
colon. Colon-prefixed pseudo-element requests remain unsupported.
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
result and must not run JavaScript. Hosts may alternatively enqueue a
staged result using `queue_resource_completion`. The bounded document task
pump publishes that result automatically at a later turn boundary, after
prior JS microtasks have run; `ScriptTurn::ResourceCompletion { applied }`
reports success or rejection. If the completion is the next eligible task,
it publishes before an already-offered frame can use older inputs.
Queued completions share the normal pending-task limit and are discarded on
shutdown. The scripted local CLI now keeps a document-scoped, bounded snapshot of
stylesheet and image bytes as they are first fetched. Later DOM-triggered
layout flushes reuse those bytes even if the underlying files change or are
removed; missing and rejected resources remain missing within that snapshot.
The local fetcher can also stage a replacement using the normal file-security
policy; image replacements additionally pass through the existing bounded
image decoder before they may be applied, rejecting broken images or excessive
decoded pixel sizes without replacing a working image. The applied cache
rechecks its entry and byte budgets when the host commits it through
`apply_resource_completion`. Per-URL versions reject late
stale completions without blocking updates to unrelated resources. A
completion published while an initial file fetch is running also takes
precedence over that fetch's older bytes. A CLI regression fixture now queues actual staged CSS and PNG replacements
through the document pump and checks layout values and stale frame tickets.
At the end of local HTML parsing, the scripted CLI now discovers enabled
linked stylesheets and image sources in DOM order, resolves the first base URL,
and stages eligible files using its existing local-file grant. Each validated
resource is queued and delivered before the next one is staged, avoiding a
second pending-byte cache. Duplicate URLs are coalesced per resource kind and
candidate counts and URL lengths are bounded. The original timer-turn limit
is checked before starting an interleaved timer. Scripted dumps and final
rasterization retain the applied resources after the JS realm closes.
This describes local-file resource delivery. In the network host, navigated
body bytes stream to the parser, eligible subresources load concurrently,
and parser-blocking and deferred scripts wait for earlier stylesheets.
The CLI services due timers while waiting. See
[TD-134](TECH_DEBT.md#td-134--classic-completion-and-the-page-event-loop-paid)
and the [NET-02 ledger](IMPLEMENTATION.md#net-02--documents-and-subresources-over-http-in-the-cli).
CSSOM adds read-only stylesheet rules and declarations in
[CSS-09](CSS.md#transitions-and-animations).

`new MutationObserver(callback)` observes child-list, attribute and
character-data changes with `subtree`, `attributeFilter` and old values.
Records from script edits, host edits and the HTML parser reach the
callback in one microtask, in the specified order relative to promise jobs,
and `takeRecords()` and `disconnect()` follow the DOM steps. A removed node
stays observed through its former ancestors' subtree registrations until
the next notification. The scripted document performs a microtask
checkpoint before each parser-inserted classic, so a script sees records
for the markup before it. Callback exceptions are counted like listener
errors. `queueMicrotask` uses the same queue. Registrations are edges held
by observed nodes: an observer lives as long as a node it watches, follows
adopted nodes and is collected with them.
[ADR 0047](adr/0047-mutation-observers.md)

Uncaught exceptions from scripts, parse failures, listeners, timers,
animation and observer callbacks and module evaluation are reported the
HTML way: a cancelable `ErrorEvent` at Window (`window.onerror` gets the
five-argument form and cancels with `true`), then a console message unless
canceled. After each checkpoint the document queues a task that fires
`unhandledrejection` for still-unhandled rejections and `rejectionhandled`
for ones handled later; the loader runs it before the readiness steps.
Event handler IDL attributes (`onclick`, `onerror`, `onload`, … on Window,
Document and HTML elements) keep their position among listeners. `console`
methods fill a bounded buffer the host drains with
`take_console_messages`; `rustic … --run-scripts` prints it to standard
error. Describing a value never runs page code. `Node` also has
`nodeName`, `nodeValue`, `parentElement`, `lastChild`, `previousSibling`,
`isConnected`, `hasChildNodes`, `getRootNode`, `contains`, `isSameNode`,
`compareDocumentPosition`, `replaceChild` and `normalize`.
[ADR 0048](adr/0048-error-reporting-and-event-handlers.md)

Focus follows HTML: `focus()`, `blur()`, `click()`, `tabIndex`,
`document.activeElement` and `hasFocus()`; trusted clicks focus the nearest
focusable area, and an unhandled Tab or Shift+Tab moves through the tab
order with `blur`, `focusout`, `focus` and `focusin`. Typing into a focused
text input or textarea appends text (or deletes with Backspace) between
`beforeinput` and `input`, and an edited control fires `change` when it
loses focus. `value`, `defaultValue`, `type` and the reflected attributes
follow HTML value sanitization. Form owners come from the `form` attribute,
the nearest form or the parser's form element pointer. `ValidityState`,
`checkValidity`, `setCustomValidity`, `requestSubmit`, submit buttons and
implicit submission validate, fire `invalid` and `SubmitEvent`, and give
the host a `FormSubmission` with the entry list
(`take_form_submissions`); `reset()` restores defaults. Nothing navigates.
[ADR 0049](adr/0049-focus-and-forms.md)

`ResizeObserver` reports content and border box sizes in each rendering
turn, after animation frame callbacks and before the host renders,
looping deeper as the Resize Observer specification requires and
reporting the loop error for skipped observations. Sizes come from the same
layout snapshot as other script reads. [ADR 0050](adr/0050-resize-observer.md)

### Current limits

| Area | Remaining work |
| --- | --- |
| CSSOM and scrolling | Pseudo-element computed style, mutable CSSOM and parts of CSSOM View remain open. See [CSS-09](CSS.md#transitions-and-animations) and [the debt ledger](TECH_DEBT.md). |
| Observers | `IntersectionObserver` and remaining `ResizeObserver` lifetime and geometry details are tracked in TD-155. |
| Controls and event handlers | The forms subset still lacks controls such as `select` and label association. Handler attributes in markup need CSP-aware compilation. Error reports still lack full source locations (TD-152–154). |
| Browsing contexts | The desktop host has navigation and out-of-process frames and popups. Window-side `location`, history and cross-context scripting remain separate work (TD-130, TD-178). |
| Browser services | `fetch()` and Web Storage are available in the supported page host. Streams, workers and IndexedDB remain M9 work. |

Unimplemented Web APIs are absent from the exposed interface.

## Window encoding, URL, Fetch and WebAssembly

| Area | Implemented Window surface | Scope and evidence |
| --- | --- | --- |
| Text encoding | `TextEncoder.encode()` and `encodeInto()`, and `TextDecoder.decode()` with streaming state, BOM handling and fatal mode | [Encoding guide](ENCODING.md), [ADR 0075](adr/0075-web-text-encoding.md) |
| URL | `URL`, `URL.canParse()`, `URL.parse()` and live `URLSearchParams`, including component setters on `URL` | Structured URL record and pinned Window tests. [URL guide](URL.md), [ADR 0076](adr/0076-window-url-and-search-params.md) |
| Fetch and Blob | Buffered string, unshared fixed-length buffer, `URLSearchParams` and Blob bodies. Body readers include `text()`, `json()`, `arrayBuffer()`, `bytes()` and `blob()`. The `Blob` constructor, slicing, repeatable buffered reads and `Response.json()` are exposed | Headers and bodies follow the supported Fetch policy and consumption rules. Stream bodies, `File` and `FormData` remain open. [NET-03 ledger](IMPLEMENTATION.md#net-03--fetch-headers-request-and-response) |
| Wasm compilation | `validate`, `compile`, `Module` reflection, `instantiate` and the Fetch-response entry points | Buffered `compileStreaming` and `instantiateStreaming` check Fetch status and Wasm MIME. Compilation follows document CSP. [Compilation evidence](IMPLEMENTATION.md#wasm-06--window-module-compilation-and-mimecsp) |
| Wasm state and linking | Numeric function and global imports/exports, shared identity for linked numeric globals, and unshared 32-bit and 64-bit memories with fixed or resizable buffers. Standalone `WebAssembly.Table` supports `anyfunc` and `externref`. Module-defined function tables have live JS exports. Both interfaces support both address widths | Table imports, module-defined external-reference exports, shared Wasm memories, reference-typed host calls and progressive streaming remain open. [Runtime evidence](IMPLEMENTATION.md#wasm-0102--module-decoding-and-core-validation), [live table exports](IMPLEMENTATION.md#wasm-06--live-function-table-exports) |

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

A rendering turn runs the scroll steps, measures scroll-driven timelines and
updates animations (dispatching their events), runs the due
`requestAnimationFrame` callbacks, each followed by its own checkpoint, the
ResizeObserver steps and the view transition steps, then calls the host's
render function and
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
  `PointerEvent`, `KeyboardEvent`, `ErrorEvent`, `PromiseRejectionEvent`,
  `FocusEvent`, `InputEvent`, `SubmitEvent`, `MutationObserver`,
  `MutationRecord`, `ResizeObserver`, `ResizeObserverEntry`,
  `ResizeObserverSize`, `ValidityState`, `HTMLInputElement`,
  `HTMLTextAreaElement`, `HTMLButtonElement`, `HTMLFormElement`,
  `Animation`, `AnimationEffect`, `KeyframeEffect`, `AnimationTimeline`,
  `DocumentTimeline`, `ScrollTimeline`, `ViewTimeline`, `CSSAnimation`,
  `CSSTransition`, `TransitionEvent`, `AnimationEvent`,
  `AnimationPlaybackEvent`, `ViewTransition`, `StyleSheetList`,
  `CSSStyleSheet`, `CSSRuleList`, `MediaList`, the `CSSRule` family and
  `CSSStyleProperties`, `DOMException` and the `console` namespace. IMPLEMENTATION.md lists the members of each; treat that
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
| Error reporting, handlers and console | `crates/rustic-bindings/tests/errors.rs`, `crates/rustic-engine/tests/error_reporting.rs` |
| Node tree members | `crates/rustic-bindings/tests/node_tree.rs` |
| ResizeObserver in rendering turns | `crates/rustic-engine/tests/resize_observers.rs` |
| Web Animations, scroll timelines and view transitions | `crates/rustic-engine/tests/animations.rs` |
| CSSOM | `crates/rustic-engine/tests/cssom.rs` |
| Focus, editing, validation and submission | `crates/rustic-engine/tests/forms.rs`; parser form association in `crates/rustic-style/tests/invalidation.rs` |
| Mutation journal, observers and `queueMicrotask` | `crates/rustic-dom/tests/journal.rs`, `crates/rustic-bindings/tests/mutation_observer.rs`, `crates/rustic-engine/tests/mutation_observers.rs` |
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
checks `requestSubmit`, `invalid` events and `checkValidity` against a
static reference.

### Pinned Window web-platform-tests

The headless CLI runs upstream `testharness.js` at revision
`cb97c571c3b1947e08b8615eeb4028a2794b23a4`. The checked-in Linux CI baseline
contains **132 files and 1,368 subtests**, with **1,280 passing**
and **88 expected failures**. It includes DOM nodes/events/collections,
HTML timers and animation frames, CSSOM and CSSOM View, text encoding, URL,
Blob and Fetch, and Wasm constructors, reflection, globals, memories and tables.
See the [manifest and digests](../crates/rustic-cli/tests/fixtures/wpt-web/manifest.json)
and [per-subtest expectations](../crates/rustic-cli/tests/fixtures/wpt-web/expectations.json)
for exact coverage and failure reasons.

`tools/run_wpt_web_tests.py` wraps supported `.any.js` files as top-level
Window pages, serves the pinned helper scripts under their expected paths,
and reads results through `rustic automate`. The runner supplies a manual
clock for scripted turns. Use `--driver dump-dom` when diagnosing the legacy
report path. When a behavior change alters results, review the diagnostic
and update the expectation with the code that accounts for it.

```sh
cargo build --locked -p rustic-cli
python3 tools/run_wpt_web_tests.py --rustic target/debug/rustic
```

## Open debt

The [technical debt ledger](TECH_DEBT.md#payoff-schedule) is the source of
current targets for remaining DOM, HTML, CSSOM and Window work. Key items
include realm lifetime (TD-118), cross-context scripting (TD-130 and TD-178),
dynamic markup and modules (TD-133 and TD-135), error locations and handler
attributes (TD-152 and TD-153), and Streams (TD-169). Progressive Wasm
compilation is tracked in TD-200.

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
