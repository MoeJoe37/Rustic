# Technical debt ledger

This ledger records known shortcuts in committed code, why each exists, and
when it must be paid. It is reviewed against everything merged to `main`; the
first full review covered `main` at `b9595a5` (2026-11-08), and the open
entries were rescheduled after M4 closed (2026-11-17, see the
[payoff schedule](#payoff-schedule)).

Debt is not automatically bad. Rustic takes **deliberate debt** to ship small,
tested slices: a documented limit, an unsupported feature that fails honestly,
or a simple algorithm with a known cost. **Bad debt** is a shortcut that hides
behavior, breaks a stated contract, or will cost far more later than now:
silent truncation, unchecked casts on content-derived values, contracts that
only hold because a distant caller happens to filter input, undocumented
drift between duplicated settings, and anything that makes the web less
loadable without saying so.

## Rules that keep debt from going bad

These are enforced by CI where marked; the rest are review rules in
[AGENTS.md](../AGENTS.md).

1. **Record it or don't take it.** A `TODO`, `FIXME`, `HACK` or `XXX` in code,
   tools or workflows must cite a `TD-NN` entry below, and every cited entry
   must exist. *(CI: `tools/check_repository.py`)*
2. **No placeholder panics or debug leftovers.** `todo!`, `unimplemented!` and
   `dbg!` are rejected. *(CI: workspace Clippy lints)*
3. **No unexplained suppressions.** Any `#[allow(...)]` needs
   `reason = "..."`. *(CI: `clippy::allow_attributes_without_reason`)*
4. **No unreviewed dependencies.** A non-path crate in any manifest or the
   lockfile fails until it has a [dependency review](DEPENDENCIES.md), an ADR
   and an explicit update to the check. *(CI)*
5. **One pinned toolchain.** Every workflow's `RUST_VERSION` must equal
   `rust-toolchain.toml`, and every action is pinned to a full SHA. *(CI)*
6. **Invariants stay local.** A function must be correct for every input its
   signature admits. Narrowing casts (`as u8`, `as u32`, `as usize`) on
   content-derived values need a local bound or `try_from`; an `expect` or
   `unreachable!` must name the invariant it relies on and must never guard
   content-controlled data.
7. **Parse errors are not failures.** Web content is error-tolerant. A limit on
   content may stop work only to protect resources (memory, time, stack), and
   that must be visible to the caller, never a silent truncation of the tree.
8. **Counts in prose go stale.** When a change alters a test or corpus count,
   search the docs for the old number in the same change.

## Payoff schedule

The ledger below is in the order debt was taken. This table is the same open
debt in the order it comes due, so a milestone's entry and exit reviews can
check one row instead of reading every entry. Each entry's own trigger still
applies and wins if it fires earlier. An entry that reaches its row unpaid is
either paid then or moved to a later row in the same change, with the reason
added to the entry. Entries are never left on a row whose milestone has closed.

Scheduled 2026-11-17; the M5 rows were rescheduled by the 2026-11-19 M5 completion review, and the M7 rows moved to M8 on 2026-11-22 when M7 closed and Android/iOS device work became M14 ([ADR 0067](adr/0067-mobile-devices-last.md)). The M8 rows moved to M9, M10 and M12 on 2026-11-23 when M8 closed ([M8 completion review](reviews/2026-11-23-m8-completion.md)).

| Due | Entries | Why then |
| --- | --- | --- |
| With hosted CI | TD-07, TD-12, TD-14, TD-22, TD-145 | Process and platform-evidence debt that cannot be paid from a sandbox. TD-12's desktop half is done; its Android and iOS device runs moved to M14 (ADR 0067) |
| M9, CSS, layout and paint | TD-209, TD-210, TD-211, TD-201, TD-202, TD-204, TD-190, TD-23, TD-24, TD-25, TD-27, TD-36, TD-37, TD-38, TD-39, TD-41, TD-45, TD-48, TD-49, TD-54, TD-55, TD-56, TD-57, TD-58, TD-70, TD-71, TD-72, TD-119, TD-147, TD-149, TD-179, TD-192, TD-195, TD-177, TD-46, TD-52, TD-53, TD-59 | Declared-scope approximations of the M2/M3 subset, paid by the M9 CSS/layout workstream and its WPT reftests |
| M9, text and codecs | TD-47, TD-50, TD-80, TD-82, TD-84, TD-86, TD-87, TD-42, TD-85, TD-51, TD-81 | Locale resources, advanced shaping cases, rare codec modes and animation |
| M9, language and web APIs | TD-203, TD-200, TD-187, TD-188, TD-189, TD-169, TD-167, TD-11, TD-89, TD-99, TD-105, TD-106, TD-107, TD-113, TD-115, TD-117, TD-120, TD-121, TD-122, TD-124, TD-126, TD-127, TD-129, TD-131, TD-132, TD-133, TD-141, TD-143, TD-151, TD-153, TD-154, TD-155, TD-163, TD-180, TD-108, TD-110, TD-118, TD-135, TD-137, TD-138, TD-139, TD-140, TD-194, TD-152, TD-130, TD-178, TD-173 | Test262 full-rate work (`js.conformance`), Annex B block functions, function source text, Intl data and tailorings, `blob:` URLs, workers and DOM name storage |
| M10 (beta) | TD-197, TD-15, TD-166, TD-175, TD-191, TD-199 | Platform input methods (IBus or Fcitx, TSF, macOS text input) driving pages in CI; DevTools parse diagnostics, real-profile locking, packaging, per-frame accessibility and the AccessKit upgrade before the beta ships |
| M11 (parity) | TD-205, TD-206, TD-207 | Anchor positioning, scroll-driven animation and view transition parity with other engines, measured by their WPT directories |
| M12 (performance leadership) | TD-170, TD-168, TD-95, TD-103, TD-104, TD-112, TD-146, TD-156, TD-162, TD-193, TD-198, TD-184, TD-185, TD-181 | Collector pauses, inline-cache and shape-lookup design, BigInt algorithms; GPU shared memory, presentation from the GPU process and finer compositor layers |
| M14 (Android and iOS devices, ADR 0067) | None yet; the device and simulator half of the hosted-CI row's mobile entry is paid here | Device, simulator and Apple-entitlement work deferred to the last milestone; mobile compile checks and the Android emulator job stay in CI meanwhile |
| Standing (guarded by a rule or a test, no payoff date) | TD-04, TD-08, TD-13, TD-34, TD-35, TD-73, TD-90, TD-114, TD-144 | Reviewed whenever the guarded code changes |

The largest risks in that list are not the long M9 rows, which are honest
scope limits, but the short ones before them. The "before remote browsing is on by default" row
(formerly "M7 entry"), whose entries were meant to be paid before hostile
remote content, is empty since 2026-11-23: its last entries (TD-161,
TD-165, TD-134, and TD-111's exponential regular expressions) were paid by
M8 workstream 5 and the row was removed. Hosted CI is active after earlier
billing interruptions. TD-12's remaining device runs are scheduled for M14.

## Ledger

Status values: **Open** (accepted, has a payoff trigger), **Fixing** (in
progress), **Paid** (kept for history, with the change that paid it).

| ID | Kind | Area | Summary | Status |
| --- | --- | --- | --- | --- |
| TD-01 | Bad | HTML parser | Diagnostic count limits made error-heavy pages fail | Paid |
| TD-02 | Deliberate | DOM / parser | Depth and node budgets abort the whole parse | Paid |
| TD-03 | Watch | DOM | Every insert walks the moved subtree to measure height | Paid |
| TD-04 | Deliberate | All crates | Invariant `expect`/indexing in library code | Open, governed by rule 6 |
| TD-05 | Bad | HTML tokenizer | `char as u8` truncation in end-tag matching | Paid |
| TD-06 | Bad | CI | Fuzz workflow toolchain/action pins were unchecked | Paid |
| TD-07 | Deliberate | Process | Progress bot commits to `main`, so `feature` drifts | Open |
| TD-08 | Watch | Docs | Hard-coded corpus and test counts repeated across docs | Open, rule 8 |
| TD-09 | Deliberate | Workspace | `rustic-engine` holds only `Viewport` | Paid |
| TD-10 | Deliberate | HTML parser | Processing-instruction tree construction, scripting-on trees and `selectedcontent` are covered. | Paid |
| TD-11 | Deliberate | URL | `blob:` URLs return unsupported (`file:` paid by DOC-02b, IDNA paid by `url.idna`) | Open, `url.blob` (M9) |
| TD-12 | Deliberate | CI / platforms | Hosted desktop tests run on x64 and ARM64. Android has compile checks and an isolation emulator job. iOS is compile-checked. Two cross-checks omit the crates that need `ring` | Open |
| TD-13 | Deliberate | Tokenizer / tree builder | Five spec state-machine functions exceed 100 lines | Open |
| TD-14 | Watch | CI | Branch pushes and pull requests both run the full CI | Open |
| TD-15 | Deliberate | HTML parser / URL / base | HTML and URL diagnostics not yet routed through `rustic-base` diagnostics | Open, M10 |
| TD-16 | Deliberate | DOM | Adoption reissues node IDs instead of keeping node identity | Paid with wrapper retargeting |
| TD-17 | Deliberate | DOM / JS | DOM/JS collection was bound to one document and adoption discarded wrapper identity | Paid |
| TD-18 | Bad | DOM | Tree dumps grew quadratically with nested template contents | Paid |
| TD-19 | Watch | CI | Runners without rustup download its installer unpinned | Paid |
| TD-20 | Watch | Style | `:nth-*` selector matching recounts siblings on every test | Paid |
| TD-21 | Bad | Style | Nested `:is()`/`:where()`/`:not()` matching is exponential in nesting | Paid |
| TD-22 | Watch | CI | Emulated tests run serially to dodge a rare QEMU crash | Open |
| TD-23 | Deliberate | Style | Light children of a shadow host inherit from the host, not a slot | Open, pay with slot assignment |
| TD-24 | Watch | DOM | Custom element names approximate HTML's `PCENChar` ranges | Open, pay with the custom element registry |
| TD-25 | Deliberate | Style | Layers declared in a non-matching `@media` still take a place in the layer order | Open, pay before media-dependent layer tests |
| TD-26 | Deliberate | Style | `url()` values stay unresolved until stylesheets carry a base URL | Paid |
| TD-27 | Deliberate | CSS | `white-space` is a longhand instead of a shorthand for the CSS Text 4 longhands | Open |
| TD-28 | Deliberate | Style / CLI | External stylesheets decode as UTF-8 only; no `@charset` or legacy encodings | Paid |
| TD-29 | Deliberate | Style / URL | `:dir()` reads Bidi_Class from the IDNA tables in `rustic-url` | Paid |
| TD-30 | Watch | Style | `:has()` with multi-compound arguments walks the subtree per anchor | Paid |
| TD-31 | Deliberate | Style | Form owners ignore the parser's form element pointer | Paid |
| TD-32 | Watch | Style | `:has()` and element states make related mutations restyle the whole document | Paid |
| TD-33 | Bad | Style | `var()` substitution budget is per element, so a tiny sheet costs budget × elements | Paid |
| TD-34 | Watch | Style | Container conditions were tested before selectors; nearest-container lookup is still per rule | Partly paid |
| TD-35 | Watch | CSS / tests | The CSS conformance denominator follows property support | Guarded |
| TD-36 | Deliberate | Style | Filtered `:nth-child(... of S)` mutations use full restyles | Open |
| TD-37 | Deliberate | Layout | Normal flow normalization paid; flattened and pseudo trees pending | Partly paid, REN-01c |
| TD-38 | Deliberate | Layout | Static block subset; margins (REN-02a), positioning/floats/overflow (REN-02b) paid | Partly paid |
| TD-39 | Deliberate | Layout | Normal flow normalization before other formatting contexts and line layout | Open, REN-01c |
| TD-40 | Bad | Layout | Every text box copied a full computed style (about 5 KB each) | Paid |
| TD-41 | Deliberate | Paint | First paint slice: gamut clipping, stored-only PNG, conflated corner joins | Open |
| TD-42 | Deliberate | Text | CFF/WOFF and discovery paid; advanced fonts/native services remain (TD-85) | Partly paid, REN-03, M9 |
| TD-43 | Deliberate | CSS | Media and container query `ex`/`ch`/`cap`/`ic` ignore loaded fonts | Paid |
| TD-44 | Watch | Layout | Float placement and formatting-root retries are superlinear; metered by a fixed work budget | Paid |
| TD-45 | Deliberate | Layout | Spec-permitted approximations in floats, static positions and scroll geometry | Open, REN-02b |
| TD-46 | Deliberate | Paint | Opacity groups allocate a full surface each instead of their bounds | Open, M9 |
| TD-47 | Deliberate | Text | CSS policies and bidi scopes paid; locale resources/full text conformance remain (TD-87) | Partly paid |
| TD-48 | Deliberate | Style | Children of a `display:contents` child of a flex container are not blockified | Open, REN-08 |
| TD-49 | Deliberate | Layout | Flex and replaced sizing approximations pending text baselines and layout WPT | Open, REN-02c/REN-08 |
| TD-50 | Deliberate | Layout | OpenType/complex scripts paid; cross-source ligatures/break reshaping remain (TD-86) | Partly paid |
| TD-51 | Deliberate | Codecs | PNG ignores color management; inflate decodes Huffman codes bit by bit | Open, M9 |
| TD-52 | Deliberate | Paint | First text painting: approximate decoration metrics, no hinting or exact nonzero fill (`--font` paid) | Partly paid, M9 |
| TD-53 | Deliberate | Paint | First image painting: fill-only fit, bilinear only, no alt text (inline images and formats paid) | Partly paid, M9 |
| TD-54 | Deliberate | Paint | First outline painting: solid/double only, `auto` as solid, per-fragment inline outlines | Open, REN-04e |
| TD-55 | Deliberate | Paint | First `border-radius`: single-axis overflow ignores radii, per-fragment inline curves, unsnapped side joins | Open, REN-04f |
| TD-56 | Deliberate | Paint | First `clip-path`: no `url()`/SVG, no `evenodd`, no `xywh()`/`rect()`/`path()`/`shape()`, full-surface layers | Open, REN-04g |
| TD-57 | Deliberate | Paint | First transforms: `preserve-3d` flattened, layout unaware of transformed containing blocks, approximate projected curves | Open, REN-04h |
| TD-58 | Deliberate | Paint | First shadows, blending and filters: sRGB filters, full-surface groups, no ink overflow, approximate blur under transforms | Open, REN-04i |
| TD-59 | Deliberate | Paint | CPU tile culling skips irrelevant commands. Filter-margin changes still repaint everything, damage uses a common-prefix/suffix diff and resize relayouts from scratch. The CI frame-time gate covers GPU compositing on lavapipe | Open, REN-04j, M9 |
| TD-70 | Deliberate | Layout | Atomic inlines and block-in-inline splitting: baseline, relative-offset and paint-order approximations | Open, M3 layout |
| TD-71 | Deliberate | Layout | Sticky positioning: no overflow adjustment, no nested sticky composition, inline sticky boxes stay static | Open, M3 layout |
| TD-72 | Deliberate | Layout | css-flexbox WPT comparison: 51 of 91 static pages fail (writing modes, images, 6 undiagnosed) | Open, REN-08 |
| TD-73 | Watch | Base | Layout keeps its own `CssPx`/`CssRect` beside the unit-typed `rustic-base` geometry | Guarded |
| TD-80 | Deliberate | Codecs | JPEG: no arithmetic coding, lossless, hierarchical, 12-bit or `DNL`; color management ignored | Open, paint.images |
| TD-81 | Deliberate | Codecs | WebP and PNG ignore ICC/EXIF metadata; only JPEG applies EXIF orientation | Open, paint.images, M9 |
| TD-82 | Deliberate | Paint | Animated GIF/WebP decode every frame but paint only the first | Open, paint.images |
| TD-83 | Watch | Codecs | Image store budget bounds retained pixels, not outstanding `Arc` copies or the transient decode peak | Paid |
| TD-84 | Deliberate | Codecs | Decoder speed: bit-serial VP8L/JPEG slow paths, per-pixel loops; simple VP8 filter and multi-partition frames lack reference images | Open, paint.images |
| TD-85 | Deliberate | Fonts | Backend caches and temporary memory and handle-based traversal. System fonts by fontconfig, DirectWrite and Core Text (ADR 0078), per-OS tables and collection members are in (2026-11-23, ADR 0074) | Partly paid, M9 |
| TD-86 | Deliberate | Text | Cross-source optional ligatures, selected-break reshaping and advanced carets | Open, M9 |
| TD-87 | Deliberate | Text | English-only auto resources, SA segmentation and Level 4 phrases | Open, M9 |
| TD-88 | Deliberate | JS | RegExp literal patterns are not validated at parse time (167 Test262 parse files) | Paid |
| TD-89 | Deliberate | JS | Decorators, explicit resource management and import phases are unsupported syntax | Open, per ECMAScript edition (M9) |
| TD-90 | Watch | JS | The parser's stack guard makes the deepest accepted nesting depend on the build | Guarded |
| TD-91 | Deliberate | JS | The JS heap never frees: arenas grow until the budget runs out | Paid |
| TD-92 | Deliberate | JS | BigInt has storage, printing and comparison only; `ToBigInt` and the other numeric conversions are missing | Paid |
| TD-93 | Watch | JS | Environment bindings are found by a linear name search | Paid |
| TD-94 | Deliberate | JS | Collection runs only at caller safe points; a native function cannot collect mid-run | Paid |
| TD-95 | Deliberate | JS | Collection is stop-the-world and full-heap: no incremental or generational marking | Open, `perf.gc` (M12); measured by JS-09 |
| TD-96 | Deliberate | JS | No weak references, and DOM host objects are not traced by the JS heap yet | Paid |
| TD-97 | Deliberate | JS | Syntax the bytecode compiler rejected; asynchronous module execution, dynamic imports and host-provided module metadata (what remains is TD-105 and TD-110) | Paid |
| TD-98 | Deliberate | JS | Every `function` allocates its `prototype` object eagerly, so a closure costs 200 bytes | Paid |
| TD-99 | Deliberate | JS | Annex B function-in-block semantics are missing (mapped `arguments` objects paid) | Open, `js.conformance` (M9) |
| TD-100 | Deliberate | JS | Array destructuring does not close the iterator when a target throws | Paid |
| TD-101 | Watch | JS | String concatenation copies both operands (no ropes): building a string in a loop is quadratic | Paid |
| TD-102 | Watch | JS | The interpreter's value stack, frames and the AST being compiled are not charged to the heap budget | Paid |
| TD-103 | Watch | JS | Inline caches hold one shape per site and share one epoch, so a prototype write or a collection flushes all of them | Open, `perf.interpreter` (M12) |
| TD-104 | Watch | JS | Shape lookups walk the transition chain | Open, `perf.interpreter` (M12) |
| TD-105 | Deliberate | JS | The Test262 module gate has known failures, its `$262` has no `gc` or `agent`, and it runs `test/language` plus the installed areas of `test/built-ins` | Open, `js.conformance` (M9) |
| TD-106 | Deliberate | JS | `Function.prototype.toString` prints every function in the native form (eval paid) | Open, `js.conformance` (M9) |
| TD-107 | Deliberate | JS | CLDR collation still needs full tailorings, implicit weights and conformance evidence | Open, `js.conformance` (M9) |
| TD-108 | Deliberate | JS | `Date` local time is one fixed offset: no time zone database, no daylight saving time | Open, M9 language and web APIs |
| TD-109 | Deliberate | JS | Unhandled promise rejections are not reported to the host (no HostPromiseRejectionTracker) | Paid |
| TD-110 | Deliberate | JS | RegExp Unicode property escapes (`\p{…}`, `\P{…}`, v-mode string properties) parse but cannot run | Open, M9 language and web APIs |
| TD-111 | Deliberate | JS | The RegExp matcher backtracks, so some patterns take exponential time (bounded only by the step budget and cancellation) | Paid |
| TD-112 | Deliberate | JS | BigInt results are capped at 2²⁰ bits and multiplication and division are schoolbook (quadratic) | Open, `perf.interpreter` (M12) |
| TD-113 | Deliberate | JS | `for-in` over a proxy collects every key before the loop runs, so traps fire earlier than the specification's lazy enumeration | Open |
| TD-114 | Deliberate | JS | `caller` and `arguments` of a sloppy function read as `null` through `%ThrowTypeError%` instead of throwing | Open, by design |
| TD-115 | Deliberate | JS | A `SharedArrayBuffer` is shared within one agent only (no cross-agent byte store until workers); typed array elements are read through the generic property path | Open, M9 workers / `perf.interpreter` |
| TD-116 | Deliberate | JS | A rope's flat copy is made behind `&self` and charged late, so the heap can pass its limit until the next allocation or safe point | Paid |
| TD-117 | Deliberate | JS | Intl list formatting uses fixed templates and best fit uses likely-subtag fallback | Open |
| TD-118 | Deliberate | JS | A realm is never freed while its agent lives, and its intrinsics are created eagerly (about 119 KB each) | Open, M9 language and web APIs |
| TD-119 | Deliberate | Style | Scoped styles use the full-restyle path after a DOM mutation | Open, CSS-09 |
| TD-120 | Deliberate | JS | Intl embeds 8 MB of all-locale data packs, keeps a second copy of the plural rules and approximates ICU's range and currency-spacing rules | Open |
| TD-121 | Deliberate | JS | Intl.Segmenter uses the untailored UAX #29 rules: no dictionary words for Chinese, Japanese or Thai, no sentence suppressions, and a second grapheme implementation beside rustic-text | Open |
| TD-122 | Deliberate | JS | DateTimeFormat uses a limited calendar set, Gregorian pattern selection and GMT fallbacks for zone names | Open, `js.conformance` (M9) |
| TD-123 | Deliberate | DOM / JS | Interpreter safe points used JS-only collection | Paid |
| TD-124 | Deliberate | JS / Web IDL | DOMException has the base interface, with native stacks, derived quota exceptions and structured cloning deferred | Open |
| TD-125 | Deliberate | JS | M4 used QEMU. M8 recorded native ARM64 timings on all three desktop OS families | Paid |
| TD-126 | Deliberate | Web bindings | The page host exposes a subset of HTML and DOM interfaces | Open, DOM and HTML integration |
| TD-127 | Deliberate | DOM | Element and attribute names use scalar UTF-8 storage | Open, DOM names and selectors |
| TD-128 | Deliberate | Web bindings | Live NodeList length and positional access walked child links | Paid |
| TD-129 | Deliberate | Web bindings | Descendant and named collection reads scan live nodes and attributes | Open, DOM collection performance |
| TD-130 | Deliberate | Web bindings | Window is scoped to one top-level document, with named properties, navigation and origin checks deferred | Open, M9 |
| TD-131 | Deliberate | DOM events | Initial click and keyboard delivery leaves extended input events, signal-driven removal, shadow retargeting and HTML error reporting | Open, M9 |
| TD-132 | Deliberate | Window scheduling | Headless timers bind at the turn boundary and frame callbacks use host time and exception counts | Open, M9 |
| TD-133 | Deliberate | HTML scripting | Adoption of subtrees containing retained parser or script nodes and dynamic document markup have explicit exceptions | Open, M9 HTML lifecycle |
| TD-134 | Deliberate | HTML scripting | External classics follow explicit host completion order. Stylesheet dependencies and task interleaving remain open. | Paid |
| TD-135 | Deliberate | HTML modules | Static local JavaScript modules share a map. Dynamic imports, import maps and data module types are pending. | Open, M9 language and web APIs |
| TD-136 | Bad | JS | The four URI functions and the omitted standard globals are covered by the pinned Test262 gate | Paid |
| TD-137 | Deliberate | JS | Web-legacy library surface is missing: Annex B `escape`/`unescape` and String HTML methods, `__defineGetter__` and its siblings, legacy RegExp statics and `Error.prototype.stack`; `test/annexB` is not run | Open, M9 language and web APIs (`js.conformance`) |
| TD-138 | Bad | JS | Some statement completion values are wrong, and parameter defaults and `for-in`/`for-of` heads miss their TDZ errors | Open, M9 language and web APIs (`js.conformance`) |
| TD-139 | Bad | JS | Reference evaluation order deviates in compound, update and logical assignment, `with` over proxies, destructuring targets and `super` properties; `delete super.x` does not throw | Open, M9 language and web APIs (`js.conformance`) |
| TD-140 | Bad | JS | Array destructuring assignment steps the iterator before evaluating a target and does not close it when the target reference throws (TD-100 paid the binding form only) | Open, M9 language and web APIs (`js.conformance`) |
| TD-141 | Bad | JS | Class, function and direct-eval edge semantics: derived-constructor return override in `catch`/`finally`, inferred names for symbol keys, private names on non-extensible objects, `super` and private names inside direct eval in class bodies | Open, `js.conformance` (M9) |
| TD-142 | Bad | JS | Built-in edge bugs: weak keys, the String Iterator tag, `Object.fromEntries`, `Object.prototype`'s prototype, array `length` order and buffer checks | Paid |
| TD-143 | Deliberate | JS | Recent-edition and stage 3 library features are missing (iterator chunking, zip, `includes` and `join`, immutable `ArrayBuffer`, `ShadowRealm`, Temporal) | Open, per ECMAScript edition (M9) |
| TD-144 | Deliberate | JS | Proper tail calls are not implemented, as in V8 and SpiderMonkey | Open, by design |
| TD-145 | Deliberate | Process / JS | The Test262 execution gates run only by hand on an x86-64 host: no hosted CI job and no 32-bit, ARM64 or big-endian run | Open, CI portability evidence |
| TD-146 | Watch | Encoding | Legacy multi-byte decoders and encoders run one byte or scalar value at a time, with no vectorized or table-driven fast path beyond ASCII runs | Open, M12 |
| TD-147 | Deliberate | DOM / parser / layout | The default depth limit (256 edges) flattens deep pages earlier than Chromium's cap (514) | Open, M9 |
| TD-148 | Watch | Web bindings / CLI | Every layout flush serializes all supported longhands for every styled element, and each computed-style read clones the element's whole record | Paid |
| TD-149 | Deliberate | Web bindings | `getComputedStyle` throws a `TypeError` for any colon-prefixed pseudo-element argument instead of returning pseudo-element or empty styles | Open, M9 CSS, layout and paint |
| TD-150 | Watch | Web bindings | Disconnected and expired mutation-observer registrations stay on their node until a later mutation inspects it, and in a live observer's list until `disconnect` | Paid |
| TD-151 | Deliberate | Web bindings | Records for parser and host edits find interested observers when the journal is drained at the next binding call or checkpoint, not at the instant of the edit | Open, M9 HTML lifecycle |
| TD-152 | Deliberate | Web bindings | Error reports carry no script URL or line and column, `location` is absent, and the console does not interpret format specifiers | Open, M9 |
| TD-153 | Deliberate | Web bindings | Event handler content attributes are not compiled (their compilation must apply CSP `script-src-attr`, see TD-164), body and frameset do not forward Window handlers, only a subset of handler names exists, cross-document `replaceChild` queues two records and `normalize()` can stop part-way at the data budget | Open, M9 HTML lifecycle |
| TD-154 | Deliberate | Web bindings / style | Focus and validity are not style states, control values are not painted, and `select`, labels, selection, caret, `pattern`, `step` and `badInput` are missing | Open, M9 HTML lifecycle |
| TD-155 | Deliberate | Web bindings | ResizeObserver keeps observers and targets alive while observing, uses one device pixel per CSS pixel and horizontal sizes, and IntersectionObserver is absent | Open, M9 |
| TD-156 | Deliberate | Network | One thread and one connection per exchange, `Connection: close`, no pooling, HTTP/2 or HTTP/3, and a connect in progress cannot be cancelled | Open, M12 |
| TD-157 | Deliberate | Network | No DNS: the transport reached only allowlisted loopback origins | Paid 2026-11-22 (TLS 2026-11-20; DNS and public addresses, ADR 0066) |
| TD-158 | Deliberate | Network | No content-coding decoding: no `Accept-Encoding` is sent and `Content-Encoding` bodies are delivered still encoded | Paid 2026-11-20 |
| TD-159 | Deliberate | Network | No Public Suffix List is bundled: without a host-supplied list every domain's last label is its public suffix, so `co.uk`-style suffixes count as registrable | Paid 2026-11-20 |
| TD-160 | Deliberate | Network / storage | The cookie jar and CORS-preflight cache are in memory and not keyed by network partition; one `Fetcher` is one profile | Paid 2026-11-20 |
| TD-162 | Deliberate | Network / codecs | `gzip` and `deflate` response bodies are buffered whole (up to `max_body_bytes`) and inflated at the end of the body; only brotli decodes incrementally | Open, M12 |
| TD-161 | Deliberate | Network | `Fetcher` blocks per hop, takes whole request bodies and has no Lax-allowing-unsafe window (`Referer`, `Sec-Fetch-*`, `Accept-Language` and the HTTP cache paid 2026-11-20) | Paid |
| TD-163 | Deliberate | Network | CSP violation reports are returned to the caller (the console) but never sent: no `report-uri` or `report-to` delivery, no `securitypolicyviolation` event | Open, M9 web APIs |
| TD-164 | Deliberate | Network / engine | CSP is enforced for scripts, `eval` and subresource URLs, but not for inline `<style>`, `style` attributes or inline event handlers in the engine | Paid 2026-11-20 |
| TD-165 | Deliberate | Network / CLI | CLI network loads read each body whole and fetch subresources one at a time on the calling thread; the quirks-mode same-origin stylesheet MIME exception is missing | Paid |
| TD-166 | Deliberate | Storage | A persistent profile's `LOCK` file is not checked for a dead owner; after a crash the user removes it | Open, M10 |
| TD-167 | Deliberate | Storage / web APIs | Web Storage changes fire no `storage` event in other documents with the same storage key | Open, M9 web APIs |
| TD-168 | Deliberate | Network / storage | The HTTP cache is in memory only: persistent profiles do not keep it, and there is no `Cache-Control: immutable`, `stale-while-revalidate` or range caching | Open, M12 |
| TD-169 | Deliberate | Web bindings / network | `fetch()` blocks the script thread until the whole response arrives. Bodies remain buffered. `ReadableStream`, `formData()` and `AbortSignal` are pending. The CLI's API base URL ignores `<base>`. | Partly paid, M9 web APIs |
| TD-170 | Deliberate | IPC | Broker/renderer IPC copies every body through a pipe: no shared memory, one outstanding request per renderer | Open, M12 |
| TD-171 | Deliberate | Isolation | The renderer was split but not confined, and `reftest` ran single-process | Paid 2026-11-22 (watchdog 2026-11-20, Linux and Windows sandboxes 2026-11-21, macOS and `reftest` 2026-11-22) |
| TD-172 | Deliberate | Isolation | The Linux sandbox had no seccomp-bpf syscall filter | Paid |
| TD-173 | Deliberate | Permissions | The browser strip (tabs, address, find, permission prompt, download notice; ADR 0074) and the permission and clipboard rules exist, but no page API (Notifications, Geolocation, async Clipboard) asks them yet; downloads are buffered whole and get no quarantine attribute or Mark of the Web | Open, M9 |
| TD-174 | Deliberate | Isolation | macOS renderers were not confined by default | Paid 2026-11-22 (ADR 0063 Accepted, verified on native macOS x64 and ARM64; follow-ups TD-183) |
| TD-175 | Deliberate | Isolation | The Linux sandbox binds all of `/usr` read-only and needs bubblewrap on `PATH` | Open, M10 |
| TD-176 | Deliberate | Isolation / network | The network process (ADR 0061) was not OS-confined | Paid 2026-11-22 (Linux 2026-11-21, macOS and Windows 2026-11-22; Windows loopback fixture runs stay unconfined, ADR 0066) |
| TD-177 | Deliberate | Isolation / paint | Frames are composited by linear coverage (ADR 0062): exact under clips, opacity and overlays, approximate under non-`normal` blend modes and color filters, not at all under CSS transforms (they paint white); frame documents have an opaque white canvas | Open, M9 |
| TD-178 | Deliberate | Isolation / web bindings | Only `render` in a split run loads frames; `--single-process`, `reftest`, dump commands and `hit-test` show frames white or empty. Every frame has its own renderer, so no `contentWindow`/`contentDocument`/`parent`/`postMessage` or frame `load` events, and frames render once. Popups likewise: `window.open()` returns `null`, popups have no `window.opener`, their output is not shown, and `_self`/`_parent`/`_top` targets do not navigate | Open, M9 |
| TD-179 | Deliberate | Style / HTML rendering | HTML presentational hints cover dimensions and table attributes only (2026-11-27): `body`/`font` colors, `bordercolor`, `rules`/`frame`, `hspace`/`vspace`, list and `hr` attributes are not mapped | Partly paid, M9 CSS/layout |
| TD-180 | Deliberate | CSSOM View | Script scrolling covers positions, the scroll methods and `scroll` events only: no `scrollWidth`/`scrollHeight`/`clientWidth`/`clientHeight`, `scrollIntoView`, smooth `behavior`, `scrollend` or scroll anchoring, and `getBoundingClientRect` does not re-solve sticky boxes for script positions | Open, M9 language and web APIs |
| TD-181 | Deliberate | Compositor | Scenes layer the viewport, fixed/viewport-sticky boxes and top-level scroll containers only: nested containers, containers in groups or with rounded clips repaint their damage, the overlay is re-rasterized every frame, pages with iframes and `dump-paint`/`hit-test` skip the scene (no device-pixel snapping there), and overlays not painted last fall back to one-pass painting | Open, M12 |
| TD-182 | Deliberate | Desktop host | Paid 2026-11-23: `rustic view` renders through one long-lived renderer per page with clicks and keys routed to it, the UI-01 browser strip, and window tests on native Windows, macOS and Linux (ADR 0069, ADR 0074); IME composition into pages moved to TD-197 | Paid |
| TD-183 | Deliberate | Isolation | Paid 2026-11-23: macOS children start from the windowless `rustic-renderer` (no AppKit before the profile applies), and the profiles hide the machine's names and the process and network `sysctl` families (ADR 0074) | Paid |
| TD-184 | Deliberate | Compositor / IPC | The GPU process receives the page and frame pixels over its pipe and returns the composited page for every composited frame (two full copies), because there is no shared memory yet | Open, M12 |
| TD-185 | Deliberate | Compositor / desktop host | `rustic view` presents frames composited in the broker; the GPU process does not own the window surface yet | Open, M12 |
| TD-186 | Deliberate | Compositor / isolation | On Windows and macOS the confined GPU process uses the renderer AppContainer and Seatbelt profiles, which grant no GPU driver services, so the hardware compositor (ADR 0071) may only open there with `--no-sandbox`; confined runs composite on the CPU | Paid 2026-11-23 (`Rustic.Gpu` AppContainer, Seatbelt GPU profile, CI requires the confined adapter) |
| TD-187 | Deliberate | JS debugger | Code from `eval`, `new Function`, class field initializers, generated constructor bodies and module scripts carries no debugger steps, and code compiled before a hook attaches has none, so breakpoints there never hit (ADR 0072) | Open, M9 (language and web APIs) |
| TD-188 | Deliberate | DevTools | Inline script lines are mapped to document lines by finding the script text in the document bytes, which works only for UTF-8-compatible documents; others report script-relative lines | Open, M9 (language and web APIs) |
| TD-189 | Deliberate | DevTools / network | `network.requests` lists only requests of network documents (`--allow-origin`); local-file loads and frames' requests are not logged, and the log keeps no timing or bodies | Open, M9 (language and web APIs) |
| TD-190 | Deliberate | DevTools | Automation protocol 1 has no CSS matched-rule or cascade inspection, no key or pointer-move input (only clicks), no DOM editing and no event timeline (profiling gives statement counts and turn metrics) | Open, M9 (CSS, layout and paint) |
| TD-191 | Deliberate | Accessibility / desktop host | `rustic view` exposes the live page and routes assistive actions. Browser chrome and separate frame trees still need platform accessibility integration | Partly paid, M10 |
| TD-192 | Deliberate | Accessibility / layout | Pages with form controls cannot be laid out yet (form controls are special elements the block reference does not support), so their accessibility tree comes from the box tree without bounds, and `rustic view` cannot show them | Open, M9 CSS, layout and paint |
| TD-193 | Deliberate | Accessibility | The whole tree is rebuilt, encoded and sent for every request and, while an assistive technology listens, for every `rustic view` frame (since 2026-11-23, ADR 0074, none while nobody listens); there are no incremental updates | Open, M12 (performance leadership) |
| TD-194 | Deliberate | Editing | Text controls move the caret by code point (no grapheme or word movement, no Ctrl+Arrow), have no undo/redo or clipboard, fire no `select`/`selectionchange` events, and `contenteditable` is not editable | Open, M9 language and web APIs |
| TD-195 | Deliberate | Browser shell | Find in page searches within one text fragment (a phrase across lines or elements is not found) and estimates a match's horizontal extent from its share of the fragment's characters | Open, M9 (editing and selection) |
| TD-196 | Deliberate | Browser shell | Paid 2026-11-23: every tab keeps a live page (up to six, least recently shown discarded first), background pages get at most one frame a second, and one opened profile serves every page thread (ADR 0074) | Paid |
| TD-197 | Deliberate | Desktop host | **Partly paid 2026-11-23 (ADR 0074):** composition reaches the focused page field as well as the browser's own. Remaining: CI exercises it with synthetic events through the host's handler, not with the platforms' input methods (IBus, TSF, macOS text input) | Partly paid, M10 (beta input-method runs) |
| TD-198 | Deliberate | Network / CLI | Frame and popup documents are read whole in the broker before their renderer parses them; only top-level documents stream | Open, M12 |
| TD-199 | Deliberate | Dependencies / accessibility | The AT-SPI graph of AccessKit 0.25 carries quick-xml 0.38 (RUSTSEC-2026-0194/0195, reached only by atspi-common's own tests) and the unmaintained zvariant, zvariant_derive and zbus_names (RUSTSEC-2026-0370..0372); `deny.toml` ignores them with reasons | Open, M10 |
| TD-200 | Deliberate | WebAssembly | The progressive decoder pulls response chunks and compiles in an agent promise reaction | Partly paid by ADR 0094, M9 |
| TD-208 | Deliberate | WebAssembly | Table imports and independently linked function references needed shared allocations and traced function owners | Paid by ADR 0091 and ADR 0092 |
| TD-209 | Deliberate | Layout | Grid subset: no masonry, `horizontal-tb` only (inline-axis baseline alignment falls back to `start`), no baseline sharing across subgrids, and auto repetitions in intrinsic sizing ignore percentage min/max sizes | Open, `layout.grid` (M9) |
| TD-210 | Deliberate | Layout | css-grid WPT comparison: 89 of 168 static pages fail, every one reviewed (writing modes, masonry, subgrid baselines, logical properties, Ahem text) | Open, `layout.grid` (M9) |
| TD-211 | Deliberate | Layout | Table subset: separated borders only; `visibility: collapse`, `empty-cells`, column boxes' own geometry and relative positioning of table parts ignored; css-tables WPT 18 of 35 static pages pass, the rest reviewed | Open, `layout.tables` (M9) |

### TD-01 — Diagnostic count limits made error-heavy pages fail (Paid)

`TokenizerLimits::max_diagnostics` and `ParserLimits::max_tree_diagnostics`
(both 1,024) were fatal: the 1,025th recorded parse error discards the tree
(`HtmlError::DiagnosticLimit`, `ParseError::TreeDiagnosticLimit`). Parse
errors never stop a conforming HTML parser, and ordinary pages routinely have
thousands, so this limit rejected valid-to-load content to protect a vector of
diagnostics that the tree does not depend on.

**Why bad:** it made the web less loadable for no resource benefit beyond the
diagnostic list itself. **Paid:** the tokenizer and tree builder keep the first
N diagnostics and count the rest; `HtmlTokenizer::dropped_diagnostics`,
`ParsedDocument::dropped_diagnostics` and `ParsedFragment::dropped_diagnostics`
expose the count, and both fatal error variants are gone. A test parses a page
with over 3,000 recoveries at default limits and checks that the tree does not
depend on the retention limit.

### TD-02 — Depth and node budgets abort the whole parse (Paid)

**Paid 2026-11-18** by [ADR 0046](adr/0046-parse-budget-degradation.md).
Past `max_depth` the tree builder inserts higher up, as Chromium, WebKit
and Gecko do; at a 514-edge limit the trees equal Chromium 157's on the
recorded cases (`depth_cap_places_nodes_like_chromium`). At the node or
payload budget a document parser truncates: the tree built so far is the
document, `ParsedDocument::truncated()` says so and the CLI warns. Fragment
parsers still fail, so DOM APIs can throw. The default depth stays 256
(TD-147). The original entry is kept below.

`DocumentLimits` (100,000 nodes, 256 edges deep, 16 MiB payload) are fatal for
parser-built documents ([HTML_TREE.md](HTML_TREE.md#ownership-and-failure)).
This is deliberate, documented and resource-motivated: it bounds memory and
keeps every algorithm iterative. It is still debt, because browsers keep
loading deep documents by capping insertion depth (Blink reparents at 512
levels) and large pages can exceed 100,000 nodes.

**Pay:** decide a compatible degradation strategy (for example, insert at the
depth cap instead of failing) in an ADR, with html5lib and real-page evidence.
**Trigger:** before the loader accepts network content or a real-page
compatibility corpus is added.

### TD-03 — Every insert walks the moved subtree to measure height (Paid)

`Document::insert_before` (`crates/rustic-dom/src/document.rs`) computes the
moved node's subtree height with a full preorder walk on every call. Inserting
a new leaf is constant work, but reparenting a large subtree costs its size,
and repeated reparenting (adoption agency, foster parenting, template moves)
could become quadratic. A probe with 200 nested distinct `<b>` elements
around 5,000 children parsed in 0.14 s (release build), so no blowup has been
observed.

**Pay:** cache subtree heights or bound the walk by the remaining depth
allowance, with a timing regression case in the fuzz smoke corpus.
**Trigger:** a fuzz timeout, a profile showing the walk, or DOM APIs that let
script move subtrees.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): every node caches an upper bound on its subtree height (`Node::height_bound`), raised along the ancestors when a node is linked and left high on removal. An insertion checks the bound in constant time and recomputes the exact heights of the moved subtree only when the bound would fail the depth limit, which leaves them exact for the next check. Tests: `moving_a_large_subtree_does_not_walk_it` (20,000 moves of a 20,000-node subtree) and `stale_height_bounds_never_reject_an_insertion_that_fits` in `crates/rustic-dom/tests/tree.rs`. What remains is a watch item, not debt: a script that alternately removes a deep branch and moves a large subtree near the depth limit pays one exact walk per such pair.

### TD-04 — Invariant `expect` and indexing in library code

Library code has about 58 `expect`/`unreachable!` sites and 87 indexing or
slicing sites (Clippy pedantic counts on `b9595a5`). They encode internal
invariants (validated node links, occupied arena slots, matched parser
state), each with a message, and the required fuzz smoke job exercises them.
This is acceptable while rule 6 holds.

**Pay:** convert a site to a typed error whenever its input can be influenced
by content, or when a public API exposes it. **Trigger:** any fuzz panic.

### TD-05 — `char as u8` truncation in end-tag matching (Paid)

`extend_end_tag_probe` and `script_word_char` in
`crates/rustic-html/src/tokenizer/text.rs` compared `value.to_ascii_lowercase()
as u8` against tag bytes. The cast truncates non-ASCII scalars (U+0174 becomes
`t`), so correctness depended on every caller filtering to ASCII first. Callers
did, so there was no observable bug, but the invariant lived elsewhere.
Paid by `ascii_lowercase_byte`, which returns `None` for non-ASCII scalars, with
a unit test.

### TD-06 — Fuzz workflow toolchain and action pins were unchecked (Paid)

`tools/check_repository.py` verified `RUST_VERSION` and SHA-pinned actions only
in `ci.yml`; `fuzz.yml` duplicates both and could drift unnoticed. Paid by
checking every workflow (rule 5).

### TD-07 — Progress bot commits to `main`, so `feature` drifts

After green CI on `main`, `tools/publish_progress.sh` commits regenerated
`README.md` and `docs/progress/*` straight to `main`. Work lands through the
shared `feature` branch, so `feature` is behind `main` after nearly every merge
and threads hit merge commits or conflicts in generated files.

**Convention until paid:** before pushing `feature`, run
`git fetch origin main && git merge origin/main`. If generated progress files
conflict, take either side and run `python3 tools/progress.py --write`; never
hand-edit them. **Pay:** have the bot also fast-forward `feature`, or stop
committing generated output and publish it as a CI artifact. **Trigger:** when
the branch workflow changes or conflicts start costing reviews.

### TD-08 — Hard-coded counts repeated across docs

Figures such as "1,580/1,584 records", "685 pinned cases" and "53 focused tests"
appear in several of `docs/README.md`, `docs/IMPLEMENTATION.md`,
`docs/ROADMAP.md`, `docs/STANDARDS.md` and `docs/TESTING.md`. Fixture manifests are
the source of truth and are digest-checked; the prose copies are not.

**Pay:** derive counts in `tools/check_repository.py` from the manifests and
fail on stale prose, or link to the manifest instead of repeating numbers.
**Trigger:** the first stale count found in review.

### TD-09 — `rustic-engine` holds only `Viewport` (Paid)

The crate is nearly a placeholder, which `AGENTS.md` discourages. It is kept as
the seam between hosts (CLI, desktop, mobile) and the engine. Do not grow it
with unrelated helpers. **Pay:** fill it with the engine facade (navigation,
document lifecycle) or fold `Viewport` into its first real consumer.
**Trigger:** the first host-facing engine API (M2).

**Paid:** [ADR 0020](adr/0020-document-lifecycle.md) defines the engine's
bounded document admission and shutdown contract. The CLI uses the facade
from HTML loading through rendering.

### TD-10 — Scripting-on trees and processing instructions (Paid)

Document and fragment parsing pass all 1,792 html5lib tree records. DOC-06d
paid the `selectedcontent` part, and WEB-05 enabled the scripting-on fixtures
and inserted processing instructions as inert DOM nodes. The runner records
four exact changes to legacy expectations for the current HTML tokenizer.
[HTML_TREE.md](HTML_TREE.md)

### TD-11 — `blob:` URLs return unsupported

The URL parser returns explicit unsupported errors rather than guessing
([URL.md](URL.md)). Honest failure, tracked in the M1 queue. The `file:` part
was paid by DOC-02b, which implements the standard's file states; converting a
file URL to a native path is loader/platform work, not URL debt.

The Unicode IDNA half is paid: `url.idna` implements UTS #46 over generated,
pinned Unicode data and `ParseError::UnsupportedIdna` is gone. What remains
there is deliberate and documented in [URL.md](URL.md): strict (`beStrict`)
domain validation is only needed by conformance checkers, and Punycode labels
past 2,000 bytes / 1,000 code points fail to bound quadratic work. The pinned
WPT `urltestdata.json` subset now includes all non-ASCII records (887 of 899;
only blob cases are excluded).

### TD-12 — Mobile is check-only; hosted CI is intermittent

The [platform matrix](PLATFORMS.md#verification-status) records hosted
runs for native Linux, Windows and macOS on x64/ARM64, the emulated jobs,
and compile checks. Billing interruptions previously limited verification
to the Linux sandbox. The first complete run on 2026-11-22 (`9961c1c`, run
45003031562) exposed Windows AppContainer profile races, macOS Seatbelt
loader issues and missing `ring` cross compilers. Those findings were fixed.

**Open:** physical Android and iOS runtime acceptance in M14, plus two
partial cross-checks. `aarch64-pc-windows-gnullvm` and
`loongarch64-unknown-linux-gnu` omit crates that require `ring` because the
workflow has no packaged C cross compiler for those targets. The Android
isolated-renderer test already runs on an emulator. See
[ADR 0067](adr/0067-mobile-devices-last.md) for mobile scheduling.

### TD-13 — Long spec state-machine functions

Five functions exceed 100 lines (`parser/tree/body.rs`, `parser/tree/formatting.rs`,
`tokenizer/doctype.rs`, `tokenizer/text.rs`, `tokenizer.rs`). They mirror
WHATWG state and insertion-mode tables, which keeps them traceable to the
specification. **Pay:** split by spec subsection only when it improves that
traceability. **Trigger:** a change that has to touch most of one function.

### TD-14 — Duplicate CI runs for pull requests

`ci.yml` triggers on every `push` and every `pull_request`, so a branch with an
open pull request runs the full matrix twice. Acceptable while Actions minutes
are not constrained. **Pay:** restrict `push` to the default and `feature`
branches. **Trigger:** queue times or minutes become a problem.

### TD-15 — HTML diagnostics not yet routed through `rustic-base` diagnostics

**Rescheduled 2026-11-20 (M6 completion):** Diagnostics only feed a developer console; DEV-01 is their first consumer and defines the byte-offset mapping they need. Nothing an M6 page does depends on them.

`rustic-base` now has the shared `Diagnostic`/`DiagnosticLog` contract with
source revisions and redaction (DOC-01c in [IMPLEMENTATION.md](IMPLEMENTATION.md)).
The HTML tokenizer and tree builder still keep their own compact `Copy`
diagnostics, positioned by normalized code-point offset and token index, so
they cannot yet be expressed as revision-tied byte spans. This is honest (each
type documents its units) but means two diagnostic shapes coexist. **Pay:**
map tokenizer positions back to original-byte spans, then convert at the
parser boundary into `Diagnostic`s with `Private` fields for any content.
**Trigger:** original-byte offset mapping, or the first consumer (DevTools
console, CLI diagnostic output) that needs both kinds together.

URL validation diagnostics (DOC-02b) follow the same pattern: a compact `Copy`
kind plus an original-input byte offset, with no input text. They already use
byte offsets, but a URL string has no `SourceId`/revision of its own, so the
conversion belongs to the caller that knows which document attribute or
script string the URL came from. Pay at the same trigger.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M10 (beta) row: the DevTools console (ADR 0072) reports script errors through the automation protocol, not HTML parser diagnostics; byte-span conversion is due when DevTools shows parse diagnostics in the M10 beta.

### TD-16 — Adoption reissues node IDs instead of keeping node identity (Paid)

`Document::adopt_node` moves a subtree by allocating in the target arena
and invalidating each source ID. Each `NodeId` includes a document and slot
generation, so the move preserves document-local budgets and stale handle
detection. The binding adoption path reserves its cache storage up front
and applies the complete `Adopted::ids` map to each realm's wrapper and the
node-held edges. JavaScript wrapper identity survives the move.
[ADR 0026](adr/0026-realm-dom-wrappers.md)

### TD-17 — Prototype collection was bound to one document (Paid)

JS-03 replaced the DOM prototype with a host tracer and real JS heap.
WEB-02 extends the explicit tracer to all documents registered with one
agent and keeps wrapper identity across adoption.
[ADR 0026](adr/0026-realm-dom-wrappers.md)

### TD-18 — Tree dumps grew quadratically with nested template contents (Paid)

`Document::dump_tree` indented every nested template-contents level by two more
spaces. Template nesting is content-controlled and not bounded by `max_depth`,
so output was quadratic in nesting: the 20,000-level regression test built a
1.6 GB string and took about 43 s in debug CI (already true at `b9595a5`). The
caller's byte cap bounded memory, but `dump-dom` hit it on small adversarial
inputs. Reported by the milestone M1 work. Paid by indenting at most 64 levels;
deeper lines start with an `@<depth> ` marker, so output is linear in node count.
The test now runs in well under a second and checks the threshold.

### TD-19 — Runners without rustup download its installer unpinned (Paid)

The native test job installs rustup with `curl | sh` from `sh.rustup.rs` (or
`rustup-init.exe` on Windows) when a runner image lacks it, such as
`windows-11-arm`. The download is over HTTPS but not checked against a hash,
unlike the SHA-pinned actions (rule 5). Acceptable while only
images without rustup use it. **Pay:** check the installer's published SHA-256
before running it. **Trigger:** when another job starts using the step, or
the hosted images ship rustup.

**Paid:** the step now downloads rustup-init 1.29.3 from the versioned
`rustup/archive` path and refuses to run it unless its SHA-256 matches the
hash pinned in the workflow for that OS/CPU (published hashes, checked when
pinned). Unknown runner platforms fail instead of falling back to `curl | sh`.

### TD-20 — `:nth-*` matching recounts siblings on every test (Paid)

`rustic-style` (CSS-02) computes an element's position for `:nth-child()`,
`:nth-of-type()` and their `last` forms by walking its siblings on every test,
and `of S` re-matches `S` against each sibling. Matching every element of a
parent with *n* children against such a selector is therefore O(*n*²), which
is fine for the reference matcher and tests but not for full restyles of large
lists. **Pay:** cache sibling indices per parent for the duration of a restyle
(as browsers do) and reuse them across selectors. **Trigger:** CSS-05 full
restyle, or a measured slowdown on a page with large sibling lists.

**Paid** (CSS-05): a match cache now lives for one `style_tree` restyle or
`query_selector_all` walk and memoizes `:nth-*` positions per (kind, `of`
list, element). A position is the previous counted sibling's plus one, so a
lookup walks back only to the nearest known sibling and records every counted
sibling it passes; each parent's list costs O(*n*) per selector. The cache is
keyed by selector-list addresses, so it is dropped at the end of the walk.
`nth_matching_over_long_sibling_lists_stays_linear` covers a 20,000-element
list and `shared_nth_positions_agree_with_fresh_matching` checks the cached
results against fresh matches.

### TD-21 — Nested pseudo-class matching is exponential in nesting (Paid)

`matches_complex` (`crates/rustic-style/src/matching.rs`) re-runs a nested
selector from scratch for every candidate element that an outer combinator
tries. With a descendant combinator at each level, a selector nested *k*
levels deep costs about O(*d*^*k*) on a document *d* elements deep. Measured
in a release build on a 100-deep `<div>` chain, `q` wrapped in *k* layers of
`:is(… *)` took 0.04 ms for *k* = 1, 1.4 ms for 2, 38 ms for 3 and 0.79 s for
4, roughly ×25 per level. `MAX_SELECTOR_NESTING` (32) bounds stack depth, not
time, so a 60-byte selector can hang matching for hours on a 250-deep tree.

**Why bad:** stylesheets are content, so this is an algorithmic-complexity hang
reachable from any page. **Pay:** memoize nested selector-list results per
(selector, element) for the duration of a match or restyle, which makes the
cost linear in nesting. Add the probe above as a timing regression case and a
style fuzz target. **Trigger:** before CSS-05 attaches stylesheets to documents.

**Paid** (CSS-04): each top-level match now memoizes nested selector-list
results (`:is()`, `:where()`, `:not()` and `:nth-*(of S)`) per (list, element),
so each pair is evaluated at most once and nesting costs O(*k*·*d*²) instead of
O(*d*^*k*). `ElementView` requires `Eq + Hash` for the memo key. The regression
test `nested_logical_pseudo_classes_stay_polynomial` matches eight nested
`:is(… *)` levels, the `:not(:not(…))` and `:nth-child(of …)` forms on a
100-deep chain in milliseconds, and the new `css-cascade` fuzz target styles a
document with arbitrary stylesheets.

### TD-22 — Emulated tests run serially to dodge a rare QEMU crash

In the sandbox, `qemu-s390x` crashed with an internal `SIGSEGV` (in QEMU, not
the guest) in about 1 of 12 runs of `rustic-fuzz`'s harness tests when tests ran
in parallel; reruns passed and native runs never fail. The emulated CI job sets
`RUST_TEST_THREADS=1` to lower the odds. Serial runs take about the same time
because the slow tests are sleep-bound timeouts. **Pay:** drop the variable
once a newer QEMU on the runner image passes repeated parallel runs, or move
the target to native hardware. **Trigger:** any emulated-job flake, or a runner
image QEMU upgrade.

### TD-23 — Light children of a shadow host inherit from the host

CSS-07b styles shadow trees, but `rustic-dom` does not assign slots, so there
is no flat tree. `style_tree` styles a host's light children as ordinary
children of the host: they inherit from it and match document-tree rules. In a
browser, a slotted child inherits from its `<slot>`, and an unslotted one is not
rendered. `::slotted()` is rejected rather than matching nothing silently.
**Pay:** with slot assignment (`dom.shadow` step 2), inherit along the flat
tree and add `::slotted()` with the slot's tree as its context. **Trigger:**
slot assignment, or layout consuming styles of shadow hosts' children.

### TD-24 — Custom element names approximate `PCENChar`

`Document::attach_shadow` accepts valid custom element names as hosts. The
check requires a leading ASCII lowercase letter, a hyphen, no ASCII uppercase
or whitespace and no ASCII punctuation other than `-`, `.` and `_`, and rejects
the reserved names; it accepts every non-ASCII character, while HTML's
`PCENChar` excludes a few ranges (such as U+037E and U+2000–U+200B). **Pay:**
implement the exact production when the custom element registry needs it.
**Trigger:** `dom.shadow` step 4, or a public `attachShadow` binding.

### TD-25 — Layers in non-matching `@media` still join the layer order

CSS Cascade 5 orders layers by their first declaration *that applies*: a layer
first declared inside an `@media` block whose query is false takes no place
until the query matches. Rustic builds each sheet's layer list at parse time,
before media results exist, so such a layer takes its place regardless; its
rules still apply only when the query matches. Layer order is only wrong for
pages that declare the same layer in competing conditional blocks. **Pay:**
record each layer declaration with its media conditions and rebuild the layer
order when `Cascade::set_environment` changes a result. **Trigger:** a WPT
layer-in-media case, or a site report.
### TD-26 — `url()` values stay unresolved until stylesheets carry a base URL (Paid)

**Paid 2026-11-18:** `StyleSheet::base_url` records each sheet's base: its
own URL for linked and imported sheets, the document base URL for `<style>`
elements. `StyleSheet::set_base_url` resolves `url()` values in
`background-image`, `filter` and `clip-path` with `rustic-url` (UTF-8, as
CSS requires), and `Cascade::set_document_base_url` does the same for
`style` attributes. Empty and fragment-only URLs stay as written, as do URLs
that fail to parse. A value with `var()` remembers its declaring sheet's
base and resolves after substitution, so a custom property's `url()`
resolves against the sheet that uses it, not the one that declared the
custom property (Chromium's behavior; CSSWG issue 757 is still open).
`url_values_resolve_against_the_declaring_sheets_base` checks each case.
The original entry is kept below.

`background-image: url(a.png)` keeps the URL text as written (CSS-03c). CSS
says the computed value is the absolute URL, resolved against the
stylesheet's base URL (the document for `<style>` and `style` attributes, the
sheet's own URL for external sheets), but `StyleSheet` and `ComputeContext`
do not know a base URL yet. Nothing loads images, so no behavior depends on
it today. **Pay:** record a base URL on each stylesheet and resolve `url()`
values with `rustic-url` when computing. **Trigger:** external stylesheets
(they have their own base URL) or the first image fetch, whichever comes
first. External stylesheets have now landed (`css.queries` step 2):
`StyleLoader` knows each fetched sheet's URL, so paying this means recording
it on the `StyleSheet` it produces.

### TD-27 — `white-space` is a longhand instead of a shorthand

CSS Text 4 makes `white-space` a shorthand for `white-space-collapse`,
`text-wrap-mode` and `white-space-trim`; Rustic parses it as one longhand
with the six Level 3 keywords (CSS-03c), which is enough for the HTML
user-agent sheet and line breaking's needs. The cost appears when the
longhands are exposed: values set through one must show in the other, and
CSSOM serializes the shorthand from them. **Pay:** add the longhands, make
`white-space` expand to them (with `pre`/`nowrap`/... as the documented
combinations) and give text layout the longhands. **Trigger:** inline layout
(REN) consuming white-space, or any page using the new longhands.

### TD-28 — External stylesheets decode as UTF-8 only (Paid)

**Paid 2026-11-18:** `rustic_css::decode_stylesheet` implements CSS Syntax
§3.2 (BOM, transport `charset`, a leading `@charset "…";`, the referring
document's encoding, UTF-8) over the `rustic-encoding` decoders
([ADR 0043](adr/0043-encoding-standard.md)), and the CLI's local fetcher
uses it. `stylesheets_decode_with_their_charset_rule` checks an ISO-8859-1
sheet end to end. Documents are still UTF-8, so the environment encoding is
UTF-8 until HTML encoding sniffing lands (`url.restart`, a planned
capability rather than debt). The original entry is kept below.

`dump-style`'s local fetcher decodes every linked or imported stylesheet as
UTF-8: it strips a UTF-8 byte order mark and turns invalid sequences into
U+FFFD. CSS Syntax §3.2 also honors a UTF-16 BOM, an `@charset` rule, the
referring `<link charset>`/document encoding and legacy single-byte encodings.
A Latin-1 sheet therefore shows replacement characters instead of its text;
nothing is truncated, and the replaced text is still bounded by the byte
budget. **Pay:** route fetched bytes through the encoding layer that DOC-06's
encoding sniffing will provide, with the CSS fallback chain. **Trigger:** the
encoding layer landing, or a real page fixture that needs a non-UTF-8 sheet.
### TD-29 — `:dir()` reads Bidi_Class from the IDNA tables (Paid)

**Paid 2026-11-11:** `rustic-style` now reads
`rustic_text::unicode::strong_direction`, generated with the rest of
`rustic-text`'s Unicode 17 tables, and no longer uses `rustic-url` for
directionality. IDNA keeps its own generated copy for the Bidi Rule because
`rustic-url` must not depend on the text stack;
`crates/rustic-style/tests/unicode_data.rs` checks that both tables give the
same strong direction for every scalar value. The original entry is kept below.

HTML directionality (`dir=auto`, `bdi`, `:dir()`) needs each character's strong
bidirectional type. The only pinned Bidi_Class data is the table
`tools/generate_idna_data.py` generates for the IDNA Bidi Rule, so
`rustic-url` exposes `idna::strong_direction` and `rustic-style` depends on
`rustic-url` for it. The data is exact (Unicode 17.0.0), but it lives in the
wrong crate. **Pay:** move the Bidi_Class table to a shared Unicode data crate
(or `rustic-base`) and point both users at it. **Trigger:** the bidi algorithm
or text shaping in M3, or any second user of Unicode properties.

### TD-30 — Multi-compound `:has()` arguments walk the subtree per anchor (Paid)

`:has()` arguments with a single compound (`:has(.x)`, `:has(> img)`,
`:has(+ p)`) are memoized per (compound, element), so a restyle costs time
linear in the tree. An argument with combinators inside it, such as
`:has(> a b)` or `:has(~ section em)`, depends on the anchor and is checked by
matching every candidate in the anchor's subtree (or following siblings and
their subtrees), so styling every element of a *d*-deep chain costs O(*d*²)
matches. Results are memoized per (`:has()` list, anchor), so no anchor is
evaluated twice. **Why watch:** a page can make this quadratic but not
exponential; 3,000-deep documents style in well under a second. **Pay:** use
per-element ancestor/sibling bloom filters or the invalidation-time `:has()`
indexes that engines use. **Trigger:** CSS-06 invalidation work on `:has()`, or
a measured slow page.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): a `:has()` argument whose inner combinators are child or descendant combinators (`:has(> a b)`, `:has(a > b c)`, `:has(~ section em)`) is matched suffix by suffix: whether an element starts the rest of the chain, and whether some descendant does, are memoized per (compound, element) in `MatchCache`, so styling a whole tree costs time linear in the tree per compound instead of O(*d*²). Tests: `has_chains_agree_with_brute_force` and `multi_compound_has_stays_linear_on_deep_trees` (3,000-deep chain; the old walk took 7.6 s in a debug build) in `crates/rustic-style/tests/selectors_level4.rs`. Arguments with a sibling combinator *inside* them (`:has(+ a ~ b)`) still try each candidate; they are bounded by the per-restyle memo, rare in practice, and covered by M12 `perf.style`.

### TD-31 — Form owners ignore the parser's form element pointer (Paid)

`:default`, `:checked` and `:indeterminate` need form owners (default buttons,
radio groups). Rustic resolves an owner from the `form` attribute or the
nearest form ancestor. The HTML parser also associates controls with the
form element pointer when they are not descendants (`<table><form><tr><td>
<input>`), but the DOM does not record that association yet, so such
controls get no owner. **Pay:** record parser-set form owners on the element
when form association (`HTMLFormElement.elements`) is implemented.
**Trigger:** WEB-series form APIs, or the first test that needs parser
association.

**Paid 2026-11-19 (WEB-07).** The tree builder records the form element
pointer for listed controls outside the form with
`Document::associate_parser_form`; style (`:default`, radio groups) and the
form bindings use it after the `form` attribute and the ancestor form. A
control that leaves its parent loses the association, as HTML resets its
owner; an ancestor's move does not yet clear it
([ADR 0049](adr/0049-focus-and-forms.md)).

### TD-32 — `:has()` and element states invalidate the whole document (Paid)

`StyledDocument` (CSS-06) restyles only what a mutation can affect, except
when the cascade uses `:has()` or an HTML element state that reads
whole-tree facts (`:checked`, `:default`, `:indeterminate`, `:disabled`,
`:read-write`, `:placeholder-shown`, `:lang()`, `:dir()`, ...). Then any
child-list or text change, and any change to an attribute those read, restyles
every element. That is correct but costs a full restyle per mutation on pages
that use them. **Pay:** invalidation sets for `:has()` (walk up from the
mutated element to anchors whose arguments can match, as browsers do) and
targeted state invalidation (a radio group's members, a form's submit
buttons, the subtree under a changed `lang`/`dir`/`disabled` fieldset, text
changes only under `dir=auto` ancestors). **Trigger:** script-driven DOM
mutation (JS-series) or a measured restyle cost on a page using these
selectors.

**Paid:** `InvalidationMap` records where `:has()` sits, how far its relative
selectors reach upward, whether they use sibling combinators, which attributes
and states their arguments test, and which states are used where. A mutation
now restyles the possible anchors (ancestors within reach and their preceding
siblings) and the elements a state change reaches (subtrees under `lang`,
`dir`, `disabled`, `contenteditable`; the outermost `dir=auto` ancestor of
changed text; all `input`, `button` and `option` elements for group-level form
changes). Remaining full restyles: a pragma language change, and states
inside a nested list's ancestor position of a `:has()` argument. Descendant
`:has()` outside the subject compound still restyles from the root down,
because the root is always a possible anchor; evaluating anchors before and
after the change would narrow it.

### TD-33 — `var()` substitution budget is per element (Paid)

`MAX_SUBSTITUTED_VALUES` (65,536 component values, `rustic-css/src/custom.rs`)
stops exponential `var()` expansion for **one element**, and each element that
matches the declarations expands and stores its own copy. The chain
`--v0: aaaaaaaaaa; --v1: var(--v0) var(--v0); …` to `--v39` in a 4.6 KB
stylesheet makes every matching element hold about 360 KB of substituted
values. In a release build, `dump-style` took 3.4 s for 500 `<p>` elements and
10.7 s for 2,000, and both exceeded the 64 MiB output cap. Memory and time grow
as budget × elements, so a few kilobytes of CSS on a page with 10,000 elements
amount to gigabytes.

**Why bad:** a content-controlled amplification that bypasses the per-element
budget's purpose. **Pay:** share computed custom properties between elements
with the same parent values and matched declarations (as browsers' matched
properties caches do), and/or charge substitutions to a document-wide budget
per restyle, failing the substitution (invalid at computed-value time) when it
runs out. Add the chain above as a timing regression case. **Trigger:** before
styling real pages or network content.

**Paid:** each restyle shares a `SubstitutionCache`
(`rustic-style/src/variables.rs`). Elements with the same parent custom
properties and the same winning custom declarations share one computed map,
and a `var()` longhand is substituted once per declaration, longhand and
custom-property map. Every cached computation gets a fresh budget, so a
result depends only on its key and caching never changes a style
(incremental and full restyles still agree). `CustomValue` equality
short-circuits on shared storage. Style dumps cut custom values at 256 bytes.
The 2,000-paragraph chain now styles in about 0.06 s and dumps in 0.13 s
(6.4 MB) in a release build, against 10.7 s and the 64 MiB cap before. The
chain is a regression test (`chained_variables_are_computed_once_and_shared`)
that checks the paragraphs share storage. Elements with different parents but
equal values still compute separately (no interning by content); that costs
time per distinct parent, not per element.

### TD-34 — Container query cost per element and rule

The cascade tested every `@container` condition before the rule's selector,
so each element paid an ancestor walk (and, for `style()` queries, a `var()`
substitution and two serializations) for every container rule, even rules
whose selector could never match it. With 2,000 `@container` rules, a
250-deep `<div>` chain and 3,000 `<p>` siblings, release `dump-style` took 6.0 s.
**Partly paid:** selectors are now tested first, so that input takes 0.39 s with
identical output. Still open: each matching rule still finds its nearest
eligible container by walking the ancestors, so cost is elements × matching
container rules × depth. **Pay:** cache the nearest container per (element,
name, axes) and the `style()` results per container for one restyle.
**Trigger:** a profile showing container lookup, or container-heavy pages.

### TD-35 — The CSS conformance denominator follows property support

The CSS-08 WPT gate counts a value case as in scope only when `rustic-css`
supports its property. That keeps the gate meaningful while the property
catalog grows, but it means removing a property, or failing to add one, would
quietly move cases out of scope. At the gate's introduction 1,461 of 2,916
cases were out of scope against 1,455 in scope. **Guarded:** the gate pins both
counts, so any change in scope fails until the constants and `docs/CSS.md` are
updated together. Shorthand value cases still check acceptance only (no
longhand-to-shorthand serialization), which the gate documents.

## Reviewing new work

When reviewing a merge, check each new shortcut against the rules above. Add a
ledger entry for deliberate debt in the same change that takes it, mark entries
**Paid** with the paying change rather than deleting them, and keep the summary
table in sync with the sections.

### TD-36 — Filtered sibling-count invalidation

M2 review found that changing a class in `:nth-child(2 of .hot)` restyled
only the changed element, leaving other counted siblings stale. Filters can
also inspect ancestors, sibling combinators, states and `:has()`, so cascades
with any `of S` filter now use a full restyle after a reported mutation.
This preserves the full-restyle reference contract without changing ordinary
structural or targeted `:has()` invalidation. **Pay:** record each filter's
counting scope and dependency reach, retaining the new regression tests.
**Trigger:** profiling a real filtered-sibling workload.

### TD-37 — Principal snapshots before normalized and flattened trees

REN-01a snapshots computed display boxes and raw UTF-16 text, suppressing
`none` and promoting `contents`. Anonymous boxes, whitespace processing,
pseudo boxes and formatting-context fixups come with REN-01b, before vertical
flow. Visible shadow hosts fail explicitly until slot assignment and a
flattened tree are available. `dump-boxes` labels the result a principal
snapshot and never claims positions or pixels. **Pay:** implement normalization
with mixed inline/block fixtures and a flattened-tree input contract.
**Trigger:** vertical flow/inline layout; shadow rendering before M5.

#### TD-37 update — REN-01c normal flow normalization paid

A separate borrowed formatting forest now wraps mixed inline runs, splits
inline ancestors around block groups, retains source/continuation identity and
preserves original UTF-16. Anonymous style and whitespace suppression have
focused fixtures, an independent grouping model and a mutation-fuzz target.
Flattened shadow input, pseudo boxes and other formatting contexts remain open;
the principal snapshot deliberately stays distinct from the formatting tree.

### TD-38 — Margin-free block-only vertical reference

The first vertical layout pass emits used content/padding/border rectangles for
static non-replaced block/flow-root boxes with visible overflow and zero vertical
margins. Normal/nowrap runs consisting entirely of CSS collapsible whitespace
produce no fragment. Every other text run, inline/list/flex/grid/table context,
non-static position, float, nonzero vertical margin, non-visible overflow and
special/replaced/control/foreign element returns a typed unsupported error.
This avoids inventing line metrics, dropping text or adding margins that should
collapse. `dump-layout` documents the restricted reference contract.
**Pay:** normalized/anonymous boxes and inline layout in REN-01b/REN-03; adjoining
and parent-child margin collapse, positioning and overflow in REN-02.
**Trigger:** expanding the renderer beyond controlled block fixtures. No full
block-flow/fragment progress step is credited for this subset.

#### TD-38 update — REN-02a margin restriction paid

The former nonzero-vertical-margin refusal is removed. The block reference now
implements sibling, parent-child and empty-through collapse, negative struts,
root/flow-root/border/padding boundaries and the min-height exception. Static
vertical flow and margin-collapse catalog steps have source/test evidence.
Inline/anonymous normalization, text shaping, positioning/floats, overflow
semantics and special/replaced content remain explicit unsupported cases; those
portions of TD-38 stay open. The earlier paragraph records the initial subset.

### TD-39 — Normal flow normalization before line layout and special contexts

REN-01c handles static non-floating block/flow-root/inline/inline-block content.
Other displays, replaced/control/foreign content and positioning/floats return
explicit errors. It leaves text unshaped, with only whitespace-only boundary
runs omitted and recorded. Used inline geometry, bidi, decoration propagation,
line boxes and general fragment construction remain pending. The block geometry
reference is still a separate consumer of the principal snapshot. **Reason:**
establish testable formatting relationships before font metrics and lines exist;
keep unsupported contexts visible rather than silently inventing layout.

Bottom-up sequence bubbling through nested split inlines revisits entries.
Explicit work and generated-box budgets cap this cost and keep failure atomic;
no claim of a linear-time algorithm is made. **Pay:** integrate normalization
with REN-03 line/shaping layout and general fragments; add each specialized
context with its own fixups. Replace sequence bubbling if measured workloads
exhaust budgets disproportionately. **Trigger:** first normalized-inline used
geometry or profiling of deep split-inline content.

### TD-40 — Text boxes copied whole computed styles (Paid)

The first principal snapshot cloned the DOM parent's `ComputedStyle` into every
text box, and normalization derived a fresh anonymous inline style per text run.
A style holds every longhand (69 values of 72 bytes, plus heap data), so the
default 100,000-box budget allowed roughly 1 GB of copies from a page whose
text budget is only 8 MiB. The box limit did not bound memory as intended.
**Paid** (M3 review, 2026-11-09): snapshot styles are shared `Arc`s, text runs
reuse their parent's style and normalization derives one anonymous inline style
per shared parent style. A regression test checks the sharing by identity.

### TD-41 — First paint/raster slice shortcuts

REN-04a/REN-05a (see [PAINT.md](PAINT.md)) take three deliberate shortcuts.
Out-of-gamut colors clip per channel instead of CSS Color 4 OKLCh gamut
mapping. PNG output uses stored deflate blocks, so files are about as large as
the raw pixels (a 1280×720 render is about 3.7 MB). Border sides of different
colors are composited separately, so antialiased pixels on a corner diagonal
let the background show through (coverage conflation). **Pay:** gamut mapping
with the CSS Color 4 reference tests; a deterministic deflate compressor
(fixed Huffman plus LZ77 is enough) checked by a PNG decoder; corner joins
rendered as one shape per pixel with per-side colors. **Trigger:** reftests
that compare with other engines, or output size in CI artifacts.

### TD-42 — TrueType-only font adapter

**2026-11-10 update:** CFF/CFF2, WOFF1/2 and deterministic bounded host-granted discovery are now implemented. The remaining advanced formats and native services are tracked by TD-85. The original entry below is retained for history.

REN-03a's `rustic-text` parses `glyf`-flavored sfnt fonts only. CFF/CFF2
(`OTTO`), collections (`ttcf`), WOFF/WOFF2, variable fonts, hinting, shaping
tables (GSUB/GPOS/kern), color/bitmap glyphs, vertical metrics and system font
discovery all fail explicitly or are absent. Table checksums are read but not
enforced (as browser sanitizers do), OS/2 tables shorter than their version
requires are rejected, family names are decoded only from Windows/Unicode
records and the ASCII subset of Mac Roman, and only cmap formats 4 and 12 are
accepted. Embedders add faces explicitly; the CLI dumps still compute styles
without fonts, so their output keeps the CSS fallback ratios. **Reason:** give
font-relative units and future shaping a validated, deterministic face model
with bundled original fonts before taking on more formats. **Pay:** add CFF and
WOFF2 decoding behind the same limits, platform discovery in
`rustic-platform`, and shaping/line layout in REN-03b+. **Trigger:** loading
real pages' web fonts, shaping text, or desktop/mobile hosts needing system
fonts.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 text and codecs row: the native font services part is in (ADR 0078); variable and color fonts are M9 text work.

### TD-43 — Query font units ignore loaded fonts (Paid)

**Paid 2026-11-11:** `rustic_css::FontUnitRatios` carries `ex`/`ch`/`cap`/`ic`
ratios. `MediaEnvironment::initial_font` holds the initial font's ratios
(embedders fill it from `ComputeContext::initial_font_ratios`, as the CLI
does), and `ContainerFacts` carries the container's and root's first available
font ratios, filled by `rustic-style`. Tests: `font_relative_units_*` in
`rustic-css`'s media and container tests and `queries_use_loaded_font_ratios`
in `rustic-style`'s font tests. The original entry is kept below.

Computed styles use the first available font's `ex`, `ch`, `cap` and `ic`, but
media queries still evaluate those units with fixed ratios of a 16px initial
font (`rustic-css` media evaluation), and container queries use the
container's font size with the 0.5em/0.7em/1em fallbacks. `rustic-css` cannot
see fonts, so the ratios must be passed in. **Reason:** keep REN-03a inside
computed values; query evaluation needs a font-aware environment/facts
contract. **Pay:** carry em ratios in `MediaEnvironment` (initial font) and
`ContainerFacts` (the container's computed font metrics) and add fixtures.
**Trigger:** a page whose queries use font-relative units with real fonts, or
the next change to either evaluation contract.

#### TD-38 update — REN-02b positioning, floats and overflow paid

Floats, clearance, relative/absolute/fixed positioning and non-visible overflow
no longer fail. Each has source and test evidence in `layout.position`. Sticky
positioning, text/inline content, other formatting contexts and
special/replaced elements remain explicit unsupported cases.

### TD-44 — Superlinear float placement metered by a fixed budget (Paid)

Placing a float scans every earlier float in its formatting context, so `n`
floats cost O(n²). An auto-height formatting root beside floats can be laid out
again once per float bottom below it, and nested retries multiply. Rather than
trust these bounds, every box visit, float scan and containing-chain step is
charged to `MAX_FLOW_WORK` (64,000,000 units). That also polls cancellation, and
exceeding it fails atomically with `FlowError::WorkLimit`. The budget is a
constant because `LayoutLimits` is built with struct literals in `rustic-cli`,
and adding a field would break them. **Pay:** sort exclusions into bands (or an
interval tree) and cache retry results per available width. Move the budget
into a non-exhaustive limits type. **Trigger:** profiling of float-heavy pages,
a fuzz finding that hits `WorkLimit` on small input, or the next `LayoutLimits`
change.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): float tops never decrease, so `Exclusions` keeps its areas sorted by top and a *live* list of the floats whose bottom is below the latest float top. Placement and every query that starts at or below that top scan only the live floats (the floats side by side on one row) instead of every earlier float; other queries scan only the prefix of floats that start above the query's bottom. The budget moved into `LayoutLimits::max_flow_work` (default `MAX_FLOW_WORK`), and `LayoutLimits` is now `#[non_exhaustive]` with `with_*` builders, so adding a limit no longer breaks callers. Test: `many_floats_cost_work_per_row_not_per_earlier_float` in `crates/rustic-layout/tests/flow.rs` (4,000 floats within 10⁶ work units; the old scan needed about 8 × 10⁶). Formatting-root retries are still re-laid out per float bottom and remain metered by the budget; caching retries per available width is M12 `perf.layout` work.

### TD-45 — Approximations in floats, static positions and scroll geometry

REN-02b makes these choices where the spec allows a choice or leaves the case
open:
- A float or absolute static position met while margins are still collapsing
  waits for the strut to resolve, as LayoutNG does.
- A block that gets clearance is treated as resolved, so its own margins no
  longer collapse through it.
- Before clearance is decided, queued floats are placed at `cursor + strut`.
- Block-only max-content sums consecutive floats and resets after in-flow
  boxes.
- Scrollable overflow has no end-padding inflation, there is no
  `overflow-clip-margin`, and no scrollbar gutter is reserved, matching overlay
  scrollbars.
- Percentage `top`/`bottom` against an indefinite height are treated as `auto`
  for relative boxes.

**Reason:** the fixture-backed geometry is deterministic and every choice is
documented, without inventing UI or text metrics. **Pay:** compare against
layout WPT once text and reftests exist (REN-07), add scrollbar metrics with
the platform UI, and use real intrinsic sizes after REN-03. **Trigger:**
layout-WPT import, scrolling UI, or inline content inside floats.

### TD-46 — Full-surface opacity groups

REN-04b composites each `opacity < 1` stacking context through a transparent
buffer the size of the whole surface, and nested groups each add one. Memory
is checked against `RasterLimits::max_bytes` before allocation and the
composite costs one pass over the surface, both charged to the work budget,
so the cost is bounded but proportional to the surface, not the group.
**Pay:** size groups to the union of their items' clipped device bounds, and
skip groups whose content is a single opaque-free fill (fold opacity into its
color). **Trigger:** pages with many small translucent boxes, or tiled raster.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 CSS, layout and paint row: bounded opacity surfaces are paint work for the M9 paint row.

### TD-47 — Untailored segmentation and bidi

**2026-11-10 update:** CSS Text 3 policies, language-aware English hyphenation and CSS bidi scopes are now implemented. Remaining locale and conformance work is tracked by TD-87. The original entry below is retained for history.

REN-03b's `segment` and `bidi` modules implement the default Unicode 17
algorithms only. CSS `line-break`, `word-break`, `overflow-wrap` and
`hyphens` tailorings, dictionary-based breaking for `SA` scripts (Thai, Lao,
Khmer, Myanmar), language-specific rules and UAX #9 rule L3 are absent; `SA`
characters break as `AL`/`CM` per LB1. **Reason:** the default algorithms
are fully conformance-tested and are what line layout needs first. **Pay:**
add the CSS tailorings with the properties in `rustic-css`, then dictionary
breaking behind a reviewed data source. **Trigger:** supporting
`word-break`/`overflow-wrap`, or Southeast Asian text.

### TD-48 — Blockification stops at `display:contents`

`rustic-style` blockifies the children of flex and grid containers (CSS
Display 3 §2.7) by looking at the parent's computed `display`. A child of a
`display:contents` element whose own parent is a flex container is therefore
not blockified, although its box becomes a flex item. **Reason:** blockifying
through `contents` needs the box-tree parent, which the per-element cascade
does not see. **Pay:** pass the nearest box-generating ancestor's display into
`compute`, or blockify in `BoxTree::build`. **Trigger:** a page or WPT case
that puts inline content under a `contents` wrapper inside a flex container.

### TD-49 — Flex and replaced sizing approximations

REN-02c and REN-08 make these choices before text and layout WPT exist:
- Flex and replaced item baselines are synthesized from the border-box bottom
  edge, since there are no line boxes to supply a first or last baseline.
- An absolutely positioned child of a flex container takes the content box's
  start corner as its static position instead of the full §4.1 alignment.
- A flex container's intrinsic sizes are the sum (row) or maximum (column) of
  its items' outer contributions; §9.9.1's flex-factor scaling is not applied.
- An item is laid out again after stretching or main-size resolution only when
  its size changed or became definite. Nested flex containers can still cost
  one relayout per level; `MAX_FLOW_WORK` bounds the total.
- A replaced box with only an aspect ratio and `width:auto` fills the
  containing block width (CSS Sizing's stretch fit; CSS 2.2 leaves it open).
- `img` without an embedder-supplied size is 0×0; `video`, `iframe`, `embed`
  and `object` use the 300×150 default.

**Reason:** each choice is documented and deterministic, and the missing
pieces (line baselines, layout reftests) belong to REN-03 and REN-07.
**Pay:** take baselines from line layout, implement §4.1 static positions and
§9.9.1 intrinsic sizes, and compare against the css-flexbox and
css-sizing WPT directories. **Trigger:** REN-03 line boxes, layout-WPT
import, or a fuzz finding that hits the work budget through nested flex.

### TD-50 — First inline layout slice

**2026-11-10 update:** GSUB/GPOS/kern, Arabic/Indic shaping and positioned marks are now implemented. Cross-source and selected-break refinements are tracked by TD-86. The original entry below is retained for history.

REN-03b's shaper and line layout cover the common Latin/RTL cases and refuse
or approximate the rest:

- **No OpenType layout.** One glyph per character from `cmap`/`hmtx`, so no
  ligatures, kerning, mark positioning, Arabic joining or Indic reordering.
- **Refused content (paid 2026-11-09).** Atomic inlines, blocks inside
  inline boxes and out-of-flow boxes nested in inline boxes are now laid
  out; their remaining approximations are TD-70.
- **Floats among inline content.** A float that is a direct child of an inline
  run ends the anonymous block before it, so it is placed below the previous
  line instead of on it.
- **Approximations.** `capitalize` is applied as `uppercase`. `full-size-kana`
  is ignored. Percentage margins and padding on inline boxes resolve against
  0. `unicode-bidi` on inline boxes is ignored, and each IFC is one bidi
  paragraph. East Asian segment-break removal is not done. `tab-size` is fixed
  at 8. Float-band re-measurement of one line stops after four attempts.
- **Overflow and paint.** Text does not extend scrollable overflow, and no
  glyphs are painted yet.

**Reason:** this slice gives paint and intrinsic sizing real text geometry
with exact tests, while each missing piece is its own unit. **Pay:** GSUB/GPOS
shaping on `rustic-text`, atomic inlines and block-in-inline splitting with
the formatting-context work, `unicode-bidi` embeddings, then glyph raster.
**Trigger:** any real-page text rendering, or the first WPT text reftests.

### TD-51 — First PNG decoder shortcuts

`decode_png` treats every image as sRGB: `gAMA`, `cHRM`, `iCCP`, `sRGB` and
`cICP` are read past without effect, so images with other color spaces or
gamma display with the wrong tones. Animated PNG shows only its default image.
`inflate` decodes each Huffman symbol one bit at a time (at most 15 steps),
which is linear but several times slower than table lookup. **Pay:** color
management with the paint pipeline's working space; table-driven Huffman
decoding with the same tests and fuzz target. **Trigger:** real pages with
tagged images, or decode time showing in render profiles.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 text and codecs row: PNG color management and table-driven inflate are M9 codec work.

### TD-52 — First text painting slice

**2026-11-11 update:** `--font FILE` grants local fonts to every CLI command
(M3 text completion), so the "no way to name a local font file" part is paid;
automatic platform discovery stays a host decision (TD-85). The rest stands.

REN-04c paints glyph outlines but no emphasis marks, `text-shadow` or
selection. REN-04d text decorations use fixed em fractions for thickness and
position rather than the font's `post`/`OS/2` underline and strikeout metrics,
use each fragment's own face rather than the decorating box's first available
font, draw only `solid` and `double`, ignore `text-underline-offset`,
`text-decoration-thickness` and `text-decoration-skip-ink`, and continue
through atomic inlines once those exist. Glyphs are unhinted and positioned at fractional
pixels, which keeps output identical across platforms but blurs small text.
Coverage accumulation clamps `|winding|` to 1, which equals the nonzero rule
when contours do not overlap with opposite winding inside one pixel; glyphs
from fonts with overlapping contours of opposite direction can lose coverage
there. The command line only knows the bundled test fonts: no system font
discovery and no way to name a local font file. Hit testing finds inline boxes
but never text nodes. **Pay:** decoration metrics from `post` and `OS/2`,
the remaining decoration styles and properties; an exact nonzero winding accumulator (per-pixel signed
winding with a separate coverage buffer); a `--font` option and system font
discovery per platform behind the same `FontDatabase`; text-node hit regions.
**Trigger:** the first real-page renders or text WPT reftests.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 CSS, layout and paint row: hinting and exact decoration metrics are paint work for the M9 paint row.

### TD-53 — First image painting slice

**2026-11-11 update:** inline-level images paint as atomic inlines (TD-70) and
JPEG, GIF and WebP decode through the image store (REN-05d), so those parts
are paid. Fit, sampling, density, alt text and cross-document caching stand.

REN-05c paints decoded PNGs for block-level replaced `img` boxes only:
inline-level images wait for atomic inlines (TD-50). The content is always
stretched over the content box (`object-fit: fill`, no `object-position`),
always sampled bilinearly (no `image-rendering: pixelated`, no mipmaps, so
strong downscaling aliases), and image density (`srcset`, `x` descriptors) is ignored (JPEG EXIF
orientation is applied by the decoder since the formats step; see TD-81). Broken images are zero-sized with no alt
text or broken-image icon. Decoded images live for one command; within a document the
`ImageStore` shares one entry per file, but nothing is shared across
documents. **Pay:** atomic inline images; parse and
apply `object-fit`/`object-position`/`image-rendering`; area-averaged
downscaling; alt text rendering; an image cache in the resource lifecycle
work (`paint.images` lifecycle). **Trigger:** the first real-page renders or
image WPT reftests.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 CSS, layout and paint row: image fit modes, filtering and alt text are paint work for the M9 paint row.

### TD-54 — First outline painting slice

REN-04e paints `solid` and `double` outlines; `dotted`, `dashed`, `groove`,
`ridge`, `inset` and `outset` fail atomically like non-solid borders, and
`auto` paints as `solid` in `outline-color` rather than a platform focus
ring. An inline box broken across lines gets one outline per fragment
instead of one enclosing shape,
and the outlines of all boxes in a stacking context paint together after its
content (CSS 2.2 Appendix E step 10) rather than per box as browsers do,
which only matters where outlines of one context overlap other content of
the same context. `double` splits the width in exact thirds without the
device-pixel snapping browsers use. **Pay:** share one styled-stroke
implementation with borders for the dashed, dotted and 3D styles; union
outlines for fragmented inlines. **Trigger:** border styles landing, or
outline WPT reftests.

### TD-55 — First rounded-corner slice

REN-04f rounds backgrounds, borders, outlines, overflow clips and replaced
content. A box that clips overflow on only one axis keeps a rectangular clip
(the curve applies only when both axes clip). An inline box broken across
lines rounds every fragment's corners, as if `box-decoration-break: clone`.
Multi-colored rounded borders split at the rays from outer to inner
corners, without the device-pixel snapping or gradient transition browsers
use, and abut with antialiased seams. Rounded clips are re-rasterized for
every item they clip, which is quadratic in deep nesting of rounded
scrollers (bounded by the work budget). Corner curves are four quadratics
(0.02% radial error). **Pay:** a cached clip mask per node in the tiled
raster work (paint.raster performance); `box-decoration-break`; one-axis
rounded clips. **Trigger:** the tiled raster step, or border-radius WPT
reftests that need them.

### TD-56 — First clip-path slice

REN-04g clips with `inset()`, `circle()`, `ellipse()`, `polygon()` and
reference boxes. `url()` references to SVG `<clipPath>` and `polygon()`
with `evenodd` fail atomically (the coverage accumulator only implements
the nonzero rule), and `xywh()`, `rect()`, `path()` and `shape()` do not
parse. Each clip-path context rasterizes into a full-surface layer and a
full-surface coverage mask, so many small clipped elements cost a whole
surface each (bounded by the memory and work budgets). Computed `clip-path`
values serialize every inset side and radius instead of the shortest form.
`margin-box` ignores `border-radius`. **Pay:** layers bounded by the
context's ink overflow in the tiled raster; an even-odd accumulator; the
remaining shape functions with SVG paths in M4 (`paint.formats`).
**Trigger:** the tiled raster step, SVG integration, or clip-path WPT
reftests.

### TD-57 — First transforms

REN-04h paints 2D and 3D transforms, but every context is flattened:
`transform-style: preserve-3d` parses and computes and is ignored, so there
is no 3D rendering context, depth sorting or plane intersection, and
`backface-visibility` looks only at the box's own matrix. Layout does not
know about transforms: a transformed box should be the containing block of
its absolutely positioned and fixed descendants (CSS Transforms 1 §2), but
layout still places them against the nearest positioned ancestor or the
viewport, and then paint clips them only by clips inside their coordinate
space plus the transformed box's own. Scrollable overflow ignores
transformed extents. Under a projective map, quadratic curves are flattened
in local space for an estimated stretch, so tolerance is not guaranteed
near the w = 0 plane; affine maps are exact up to flattening. A transform
with clips around it opens a full-surface layer. `calc()` in angles,
`transform-box` and the `zoom` property are unsupported, and transforms do
not create a containing block for `clip-path` hit areas outside the box.
**Pay:** preserve-3d with BSP depth sorting (M4 compositing); layout
containing blocks for transformed boxes (layout thread, with REN-02
positioning); bounded layers in the tiled raster. **Trigger:** 3D WPT
reftests, sites relying on transformed containing blocks, or the tiled
raster step.

### TD-58 — First shadows, blending and filters

REN-04i paints `box-shadow`, `mix-blend-mode`, `isolation` and the
`filter` functions. Filter color functions run in sRGB as browsers do,
not the linearRGB that `color-interpolation-filters` would select, and
round to 8 bits between functions. Every group, including a filter's and
one opened only to isolate blending children, is a full-surface buffer,
and blurs run over the whole surface (bounded by the memory and work
budgets). Layout's scrollable overflow ignores shadows and filter
outsets (ink overflow is not modeled). Inside a transform, shadow and
filter blurs use the map's stretch at one point, so they are isotropic
approximations under skew or perspective. Inline box shadows are per
fragment (`box-decoration-break: slice` is not honored). Named shadow
colors serialize as `rgb()` (28 WPT cases, listed). `text-shadow`,
`background-blend-mode`, `backdrop-filter` and SVG `url()` filters are not
implemented. **Pay:** filter regions and groups bounded by ink overflow
in the tiled raster; linearRGB as an option; ink overflow in layout.
**Trigger:** the tiled raster step, filter WPT reftests, or SVG filters in
M4.

### TD-59 — First tiled raster, damage and resize

REN-04j paints the surface in 256-pixel tiles and repaints only damaged
tiles. Each tile scans the whole display list into a frame grown by the
list's largest filter reach, skipping fills and layers whose bounds miss
the frame, but layers that do meet it still get full-frame buffers; there
is no spatial index, no tile cache between frames and no threading. Frame
times are measured by the `frame_budget` example, not gated in CI (wall
time on emulated targets means nothing). Damage
compares the two lists' fills by their common prefix and suffix, so one
change in the middle damages everything between the first and last
difference, and an item that only moved in paint order damages both
places. A change in the list-wide filter margin, the canvas color or the
surface size repaints every tile, because a tile's frame (and so its float
coverage rounding) depends on the margin. Fill bounds ignore rounded
clips, clip paths and transform clips (they only shrink paint). A resize
or zoom recomputes every style, rebuilds the box tree and lays out from
scratch, and its new surface size repaints every tile; the command-line
`hit-test` and dump commands keep the default view. **Pay:** bin commands
per tile once per list; a per-item diff keyed by box and generation;
margins per tile instead of per list; a retained tile cache with GPU
upload in M5; restyle only what viewport-dependent values touch and
incremental relayout. **Trigger:** frame-budget measurements in the paint
performance gate, scrolling, animation, or the shell's window resizing.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 CSS, layout and paint row: per-tile display-list culling and incremental relayout are raster work for the M9 paint row; the GPU frame-time gate now runs in CI.

### TD-70 — Atomic inline and block-in-inline approximations

`inline-block`, `inline-flex` and inline replaced elements are laid out as
formatting roots before line breaking, and inline boxes split around
block-level descendants. Approximations:

- **Baselines.** An atomic inline's baseline is the last (inline-flex: first)
  line box found in preorder through in-flow, non-root descendants; lines in
  nested formatting roots (`overflow`, `flow-root`, nested flex containers)
  are skipped, `order` is ignored for an inline-flex's first baseline, and
  relative offsets are not applied. `inline-grid`/`inline-table` are refused.
- **Split inline boxes.** `position: relative` on a split inline box shifts
  its inline fragments but not the block-level box inside it. Floats and
  absolutely positioned boxes inside an inline box split it like blocks do,
  instead of staying on the line (the same as TD-50's top-level floats).
  Sticky inline boxes stay in place (`inline.rs`).
- **Paint order (paid).** Atomic inlines are block fragments with
  `BlockFragment::line`; paint now paints each non-positioned one atomically
  after the rest of its line (inline boxes and text), in the inline phase of
  CSS 2.2 Appendix E. Within a line it follows all text instead of tree
  order among the line's boxes.
- **Cost.** Each atomic inline's baseline search scans its subtree, which is
  O(n·depth) for nested inline-blocks; it is metered by `MAX_FLOW_WORK`.

**Reason:** these cover the common inline-block/image cases with exact tests
while keeping each spec corner its own unit. **Pay:** baseline sets on
fragments (first/last per box, computed bottom up), relative offsets for
split boxes, floats placed during line layout. **Trigger:** inline WPT reftests or real-page renders with
`inline-block` navigation bars.

### TD-71 — Sticky positioning approximations

Sticky boxes get a `StickyConstraint` and the offset for the initial scroll
position. The offset does not adjust scrollable overflow; a sticky box inside
another sticky box (or inside a fixed box) ignores the ancestor's own shift
when computing its view rectangle; a root-element sticky box never moves;
sticky inline boxes (non-atomic) are not shifted at all; and there are no
scrollbars, so the scrollport is the full padding box. **Reason:** the
embedder has no scrolling yet, so the constraint API is what scrolling will
need. **Pay:** compose ancestor sticky offsets in `StickyConstraint::offset`,
inline sticky offsets on inline fragments, scrollbar gutters with the
scrolling work. **Trigger:** scrolling in an embedder, or css-position WPT.

### TD-72 — css-flexbox WPT comparison gaps

`tests/wpt_flexbox.rs` checks the 91 static `checkLayout` pages of the pinned
WPT revision; 40 pass (two table pages since `layout.tables`). `expected_failures.txt` gives each failure's reason:
23 need vertical writing modes, 12 need decoded images (the harness loads
none), and SVG, form controls, `fieldset`, `list-item`, quirks-mode
percentages and `aspect-ratio` on non-replaced boxes cover most others; six
are mismatches not yet diagnosed, and one percentage-height case. The harness
approximates `offsetParent` (nearest positioned ancestor, else body with
document coordinates), accepts values within 0.5px of the rounded result,
treats any other expectation attribute as a failure, and has no Ahem font.
60 pages with other scripts are not imported. **Pay:** diagnose the six,
then writing modes and an image fixture loader. **Trigger:** any flex layout
change (the list is exact, so passes and failures must be updated).

### TD-73 — Two geometry type families

`rustic-base` now has unit-typed `Length`/`Rect`/`Scale` and
`SurfaceAllocation`, which raster surfaces use, but layout and paint still
use layout's own `CssPx`/`CssRect`/`DevicePx` (same coordinate bound,
`to_typed()` converts). **Reason:** moving every layout and paint API at once
would collide with the threads changing them now. **Pay:** switch layout and
paint signatures to the base types and remove the layout copies.
**Trigger:** the next change that has to convert between them in more than
one place.

### TD-80 — JPEG decoder scope

`decode_jpeg` handles Huffman-coded baseline, extended 8-bit and
progressive frames, which is what the web serves. Arithmetic coding,
lossless (SOF3), hierarchical frames, 12-bit samples and a height defined
by `DNL` fail as `ImageError::Unsupported`. ICC profiles (`APP2`) are
ignored, so CMYK and wide-gamut JPEGs display with naive conversions. The
IDCT is an exact integer matrix product and upsampling uses the IJG
triangle filter for 2× factors, so output differs from libjpeg-turbo by at
most 3 per channel on the conformance images (allowed by T.81, which
specifies accuracy rather than bits). Non-integral sampling ratios use
nearest samples. **Pay:** arithmetic decoding (T.81 annex D) if real pages
need it; color management with TD-51. **Trigger:** a real page serving one
of these, or color-managed paint.

### TD-81 — Image metadata

ICC profiles, EXIF and XMP are skipped in WebP (`ICCP`, `EXIF`, `XMP `
chunks) and PNG (`iCCP`, `eXIf`); only JPEG applies EXIF orientation, so
a rotated PNG or WebP displays unrotated although CSS `image-orientation:
from-image` is the initial value. `image-orientation: none` is not
supported for JPEG either: the decoder always orients. **Pay:** a shared
EXIF reader for the three containers and an orientation flag on decoding
that paint chooses from the computed `image-orientation`. **Trigger:** the
`image-orientation` property in style, or real photos from phones.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 text and codecs row: ICC and EXIF handling for WebP and PNG are M9 codec work.

### TD-82 — Animated images paint their first frame

`decode_animation` composites every GIF and WebP frame (disposal,
blending, delays, loop count) and the store keeps them, but the page
loader paints only frame 0 because there is no animation timeline or
damage tracking yet. GIF delays are reported raw: browsers raise delays
of 10 ms or less to 100 ms when they play them. **Pay:** a frame clock in
the engine that picks the current frame and invalidates paint, with the
browsers' delay clamp. **Trigger:** M3 damage tracking or M4 event loop.

### TD-83 — Image store budget scope (Paid)

`ImageStore` charges decoded pixels it retains and evicts least recently
used entries to stay within `max_decoded_bytes`. Callers holding an
`Arc<Animation>` keep evicted pixels alive outside the budget, and while a
new image decodes the old entries are still resident, so the peak is up to
twice the budget. Eviction scans all entries (linear per eviction). The
command line loader takes the pixels out of the store and charges its own
per-document budget instead. **Pay:** hand out weak handles or lease
counts that the budget can see; decode into a reserved allocation; an
ordered LRU index. **Trigger:** a long-lived engine with many documents,
or memory pressure reports.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): evicting pixels that a caller still holds now keeps a weak lease on them, charged to `max_decoded_bytes` (`StoreStats::leased_bytes`) until the last reference drops, and eviction skips held entries because evicting them frees nothing. When held pixels leave too little room the decode returns `StoreError::Full` without breaking the image. Before decoding, the store makes room for the header's frame size, so a still image never takes the total over the budget while it decodes (an animation's later frames are charged when the decode finishes). Eviction takes the least recently used entry from an ordered `BTreeMap` index instead of scanning every entry. Test: `pixels_callers_hold_stay_charged_until_released` in `crates/rustic-codecs/src/store.rs`.

### TD-84 — Image decoder speed and untested paths

The decoders favour clarity: VP8L and the JPEG slow path read Huffman codes
bit by bit, VP8L applies transforms per pixel with bounds checks, and the
JPEG IDCT is a 1,024-multiply matrix product per block. Large images
decode several times slower than libwebp or libjpeg-turbo. The VP8 simple
loop filter and multi-partition token data are implemented from RFC 6386
but Pillow cannot produce them, so no reference image covers them (the
fuzz target still exercises both). **Pay:** table-driven Huffman lookup,
a separable fast IDCT with the same rounding, and reference images made
with `cwebp -f`/`-partitions` outside the repository. **Trigger:** decode
time showing in render profiles, or a mismatch report.

### TD-85 — Font adapter memory and platform services

The three reviewed font libraries are bounded at Rustic's input, decoded table,
outline output and shaping run boundaries. Their temporary allocations and lazy
caches are not charged to one global memory quota. Local discovery checks
symlinks and sorts host-granted roots, but native font services, per-user/configured
font directories, collection enumeration, variations, hinting and color glyph
painting remain outside this slice. Native paths are suggestions, not a universal
OS font database. A host-mutated directory can race portable metadata/open checks;
production brokered discovery needs platform handle-based traversal.

**Partly paid 2026-11-23 (ADR 0074):** `view` and `--system-fonts` grant the
platform's fonts: fontconfig's choices on Linux and the BSDs, per-OS file
tables (`rustic_text::native`) on Windows and macOS, generic families
mapped to the platform's families, fallback faces, OpenType collection
members (`collection_face`), within 64 MiB and 64 faces read by the broker.
Remaining: DirectWrite/Core Text adapters, per-user font configuration
beyond the listed directories, handle-based traversal and global cache
accounting.

**Partly paid 2026-11-23 (ADR 0078):** `rustic-native` asks DirectWrite on
Windows and Core Text on macOS for each generic family's files, the system
UI font (the message font, the system font) and Core Text's fallback
cascade; the broker reads and parses the files within the same budgets.
Remaining: handle-based traversal and global cache accounting. Native
codec and other OS service adapters belong to the capabilities that need
them (ADR 0078), not to this entry.

Pay before loading arbitrary remote fonts or native platform font integration:
add global temporary/cache accounting and handle-based traversal. Native
device acceptance follows the platform roadmap. The current host grants local files explicitly; neither shaping nor
layout performs I/O or downloads fonts.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 text and codecs row: backend caches and handle-based traversal are M9 text work.

### TD-86 — Shaping across source boundaries and selected breaks

Joining context crosses text/style-node boundaries (up to the backend's eight
neighboring scalars), but optional ligatures are not formed across separate
source nodes. Very long unbroken shaping runs over 4,096 scalars fail explicitly;
longer text splits only at existing soft-break opportunities. Output clusters
may span original graphemes for required or optional ligatures. A discretionary
point inside such a ligature is not exposed, and kerning/optional ligatures are
not reshaped around the finally selected line break. General caret maps and
advanced cursive justification are still pending. These are observable limits,
not proof of full text conformance.

Pay with a source-aware paragraph shaping cache, safe-to-break glyph metadata,
selected-break reshaping and carets; validate with broader shaping/layout corpora
and independent native comparisons under the existing M9 text conformance gate.

### TD-87 — Language resources and Level 4 text policies

Automatic hyphenation ships pinned English/US patterns, with two characters
left and three right, only for declared `en`/`en-US` and ASCII words up to 256
characters. Unknown/unsupported languages are not guessed. Explicit soft hyphens
suppress other automatic points in that word; the optional second pass allowing
automatic points when the remainder still overflows is not implemented.
Dictionary/phrase breaking for SA scripts and broader language spelling changes
remain pending. CSS Text 3 CJK required distinctions are implemented; `auto`
chooses the normal policy. Level 4 `word-break:auto-phrase` parses but fails
layout explicitly until phrase resources exist; `manual` uses available untailored
opportunities without language dictionaries. No full CSS Text WPT pass is claimed.

Pay with reviewed locale resources, SA/phrase segmenters, language-specific
hyphen replacement models and the M9 text conformance suites. Keep licenses,
source revisions and bounded work/output accounting for each added resource.

### TD-88 — RegExp patterns are not validated at parse time (Paid)

The lexer finds the end of a regular expression literal and checks its flags
(unknown or repeated flags, `u` with `v`), but the pattern itself is kept as
source code units and not parsed. ECMA-262 makes an invalid pattern an early
error, so `/(?<a>.)(?<a>.)/` or `/a{2,1}/` parse today instead of failing.
All 167 expected failures of the pinned Test262 parse gate are these files
(`test/language/literals/regexp`), listed in
`crates/rustic-js/tests/fixtures/test262_expected_failures.txt`. Nothing
executes yet, so no invalid pattern can run.

Pay with the RegExp pattern parser of JS-08, used both for early errors and
for compiling patterns; the gate then fails until the list is emptied.

Paid on 2026-11-12 by JS-08 part 3: the parser validates every literal
with the pattern grammar (`regexp::Regex::validate`, Annex B included), and
the expected-failures list is empty.

### TD-89 — Proposal syntax is unsupported

Decorators, explicit resource management (`using`), `import defer` and
source-phase imports are rejected as syntax errors. The Test262 gate skips the
709 files that declare these features (`test262::UNSUPPORTED_FEATURES`) and
reports the count; it never counts them as passes.

Pay when each proposal is part of the ECMAScript edition Rustic targets, by
removing it from the list so its files join the gate.

### TD-90 — Nesting limits depend on stack frame sizes

`ParseLimits::max_stack_bytes` (1 MiB by default) stops recursion by measured
stack use, in addition to the fixed `max_depth` of 256. Frames differ between
optimized and test builds and between targets, so the deepest source that
parses can differ slightly by build (about 225 nested parentheses in release
builds on x86-64). This is a resource limit that fails visibly with
`ParseError::TooDeep`, never a silent truncation. `rustic-js` builds at
`opt-level = 1` in the dev profile so test builds stay close to release.

Watch for: frame-size growth that lowers the limit below realistic nesting
(the `nesting_beyond_the_limit_fails_without_overflowing` test parses 128
mixed levels with default limits). Pay by moving deep expression chains to
an explicit stack if real content needs more.

### TD-91 — The JS heap never frees (Paid)

JS-02 allocated objects, strings, symbols, BigInts and environments into
`u32`-indexed arenas charged to the heap's byte budget, but collected nothing:
garbage stayed until the agent was dropped.

Paid by JS-03 (ADR 0016): nonmoving mark/sweep over the same arenas with free
lists that keep indices stable, collection of unused shapes and atoms, arena
tails returned to the allocator, and the budget reset to the exact live
footprint. `tests/gc.rs` and the `js-heap` fuzz target check it. Weak
references and DOM host-object tracing are TD-96.

### TD-92 — BigInt and number conversions are partial (Paid)

`BigInt` stores sign and magnitude, prints in decimal, and compares with
BigInts and Numbers exactly, which is what equality, `<` and `ToString` need.
`ToBigInt`, `NumberToBigInt` as a conversion, BigInt arithmetic, `ToIndex`,
`ToBigInt64`/`ToBigUint64` and `Number.prototype.toString(radix)` for radixes
other than 10 are not implemented. Callers get no silent wrong answers:
the operations simply do not exist yet.

The interpreter (JS-04) throws a `TypeError` for arithmetic on BigInt values
and rejects BigInt literals at compile time (TD-97).

Pay with the Number built-ins (JS-05) and the BigInt work of JS-08.

**Paid** by JS-08 part 4: `runtime/bigint.rs` implements the arithmetic of
§6.1.6.2 and `builtins/bigint.rs` the `BigInt` constructor, `ToBigInt`,
`ToBigInt64`/`ToBigUint64` and `ToIndex`. BigInt literals and literal property
keys compile. Number radix printing was already paid by JS-05.

### TD-93 — Bindings are found by name search (Paid)

A declarative environment record found a binding by a linear search over its
interned names.

Paid by JS-04: the compiler resolves every binding to a register or an
environment slot index. Names are searched only for globals (behind a
per-site cache) and inside `with`. The name lists that remain in each
environment are TD-104.

### TD-94 — Collection only at caller safe points (Paid)

**Rescheduled 2026-11-20 (M6 completion):** M6 pages are loopback fixtures; a native function growing memory without a safe point is an attack-surface concern for hostile remote content, reviewed with the other M7 entry budget items.

A handle in a Rust local is not a root, so allocation never collects on its
own (ADR 0016). The interpreter will poll `Agent::should_collect` between
instructions, but one native function that allocates in a long loop (a
built-in over a huge array) runs to the heap limit without a collection and
then fails with `OutOfMemory` even though garbage exists.

Pay with JS-05: built-ins that loop root their temporaries and poll at loop
boundaries, or runtime functions adopt handle scopes so allocation itself can
collect. Trigger: the first built-in that allocates per iteration.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): built-ins that loop over callbacks or conversions collect between iterations when script called them directly. `Agent::loop_collect` (first thing in the built-in) says whether it was called from a safe site at the collectable level, where its receiver and arguments are rooted by the caller's registers; `Agent::loop_safepoint(gc, held)` then collects at each iteration boundary if the heap asks, with the values the loop keeps in Rust locals (result arrays, `ToObject` wrappers, iterators, callbacks) pushed on the value stack meanwhile. Used by `forEach`, `map`, `filter`, `some`, `every`, `find*`, `reduce*`, `join`, `toLocaleString`, `Array.from` (both paths, through `iterate_with`) and Map and Set `forEach`. Called from inside another built-in, a loop collects nothing, as before. `looping_built_ins_collect_between_callbacks` runs each loop over 20,000 callbacks that make garbage several times a 4 MiB heap (before: OutOfMemory) and, under GC stress, checks the loops' own values survive. Other built-ins that hold many handles in Rust while they loop (sort's merge buffer, JSON) still collect only after they return; the stack-and-parse-tree charges of TD-102 and the per-loop limits bound them.

### TD-95 — Stop-the-world, full-heap collection

Every collection marks the whole live heap and sweeps every arena in one
pause. Pauses grow with the live heap, and each sweep also walks arena cells
that are free.

Pay with JS-09's limits and benchmark gate: measure pauses on the benchmark
corpus, then add incremental marking (worklist budget per step with a write
barrier) or a nursery if the PERFORMANCE.md budget is missed. Trigger: a
measured pause over budget.

**Measured by JS-09.** On the corpus the longest pause is about 0.17 ms per
MiB of live heap (15 ms at 94 MiB in `json_tree`, 0.2 to 1.3 ms for the
workloads that keep under 10 MiB live). That meets the 5 ms target up to
roughly 30 MiB of live data and misses it above, so the trigger has fired
for large heaps. Incremental marking with a write barrier is the catalog's
`perf.gc` (M12); it needs its own ADR, as the roadmap's collection design
says.

### TD-96 — No weak references or DOM host tracing yet (Paid)

The first part of JS-03 had strong edges only. Paid by its second part: weak
references, weak maps and sets with ephemeron marking and finalization
registries in `runtime/weak.rs`; host objects and `HostTracer`; and
`rustic-bindings::DomHeap`, which replaced the DOC-09 `HostHeap` and now
traces multiple documents (TD-17 paid).

### TD-97 — Syntax the bytecode compiler rejects (Paid)

JS-04 compiles functions, closures, control flow, destructuring, iteration,
exceptions and classes (with `extends`, `super`, fields, static blocks and
private names), object-literal `super`, tagged templates and optional
calls. The asynchronous module evaluator runs top-level await and dynamic
imports. The synchronous entry point rejects graphs containing top-level
await before any module body runs. A regular
expression literal that uses a Unicode property escape also fails explicitly
(TD-110).

Private names, object-literal `super`, tagged templates and optional calls
were paid on 2026-11-12, which ends the JS-04 share; `eval`, generators
(JS-08 part 1), async functions with promises (JS-06 part 1) and async
generators with `for await` (JS-08 part 2) were paid the same day, and
RegExp literals (JS-08 part 3) after them. JS-06 now loads and validates static dependency graphs through host callbacks.
Module environments, live bindings, namespace objects and synchronous bytecode
evaluation are implemented in JS-06. Each module now lazily creates its own
`import.meta` object when the host supplies no properties. A host can provide
string metadata when it instantiates the graph. The dynamic import portion
is paid. Remaining host conformance is tracked under TD-105.

### TD-98 — Eager function prototypes (Paid)

`new_closure` created the `prototype` object (80 bytes with its `constructor`
slot) for every non-arrow, non-method function, although most functions are
never used as constructors, so a closure cost 200 bytes.

Paid in JS-04: the `prototype` property exists from the start with its final
attributes and position, but holds a marker until something reads it; the
object is created then (`realize_function_prototype`), and assigning
`F.prototype = ...` first never creates it. A closure costs 120 bytes.
`function_prototypes_are_created_on_first_use` and the `js-run` differential
cover it.

### TD-99 — Arguments objects and Annex B functions in blocks

`arguments` is created unmapped in sloppy functions too (writing a parameter
does not change `arguments[i]`), and it is an ordinary object without the
arguments class. Function declarations in blocks follow the strict rules
only; the Annex B hoisting to the function scope is missing.

**Arguments part paid** by JS-08 part 5: a sloppy function with a simple
parameter list that names `arguments` gets a mapped arguments object
(§10.4.4), every arguments object has the `Arguments` class and a `callee`,
and unmapped ones throw from it. The Annex B hoisting of function declarations
in blocks is still missing.

The WEB-05 local inline fixtures use the existing function semantics.
Schedule Annex B block hoisting with the expanded M9 Test262 gate, or
earlier if a selected page or WPT fixture depends on it.

### TD-100 — Destructuring does not close iterators on error (Paid)

`for-of` closes its iterator on `break`, `return` and throw. Array
destructuring (`[a, b] = iterable`) closed it after a normal finish, but not
when evaluating a target or default threw midway.

Paid on 2026-11-12, once generators made the missing `return()` call
observable: the binding of an array pattern runs under a handler that closes
the iterator and rethrows (keeping the original error over one from
`return`), and under an iterator control so a generator's `return()` from a
`yield` in a default closes it too. An error from the iterator's own `next`
or `value` marks it done first, so nothing closes it, as for `for-of`.
`destructuring_closes_iterators_on_abrupt_completion` covers it.

### TD-101 — Strings concatenate by copying (Paid)

`a + b` allocates a new flat string. A loop that appends to one string copies
it every time, which is quadratic in the final length.

Watch for: JS-09's benchmark corpus. Pay with ropes or an append buffer for
the single-owner case if a workload shows it.

**Paid by JS-09.** The `strings` workload showed it (and showed that
`charCodeAt` and its siblings copied the string on every call). A
concatenation of 256 units or more is now a rope, flattened on first read
and by the collector; `appending_in_a_loop_allocates_linearly` and
`no_workload_is_quadratic_in_its_input` hold it. The late charge of the flat
copy is TD-116.

### TD-102 — Interpreter stacks are outside the heap budget (Paid)

The value stack (at most 2²⁰ values, 8 MiB), the frame stack (at most 10,000
frames) and the AST of the script being compiled are bounded by their own
limits but not charged to the agent's byte budget, so an agent's real
footprint can exceed its limit by those bounds.

Watch for: JS-09's heap report. Pay by charging stack growth to the budget
when the report needs one number per agent.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): the value and frame stacks are charged to the heap budget before they grow (`Agent::reserve_stacks` in `vm/interp.rs`, called for every frame push and generator resume, with 128 values of slack for the few values native helpers root there; `charge_stacks` at every safe point catches any other growth). The capacity stays charged while the agent lives (`Heap::stack_bytes`, part of the live footprint a collection restarts from), so deep recursion in a small heap stops with OutOfMemory at its limit. A script's parse tree is charged while it is compiled (`Heap::charge_transient`, 32 bytes per source unit: measured peaks are about 10 for comment-heavy code and 50 to 65 for dense minified code) by `evaluate`, `eval` and the `Function` constructor, and released when the tree is dropped. Tests: `interpreter_stacks_are_charged_to_the_heap_budget` and `parse_trees_are_charged_while_they_are_compiled` in tests/gc.rs. Module records parsed by the host (`ModuleRecord::parse`) are held by the embedder's module map, like `Source`, and are not agent memory.

### TD-103 — One cache entry per site, one epoch for all

A property site remembers a single shape, so a site that sees two shapes in
turn misses every time. All entries share one epoch: any write to an object
used as a prototype, any collection and any new global lexical binding
invalidate every cache in the agent.

Watch for: JS-09 profiles showing miss or flush cost. Pay with a second entry
per site or per-shape validity cells.

### TD-104 — Shape lookup is linear

Finding a property in a shape walks its transition chain (at most 32 steps
before dictionary mode). Inline caches hide the cost on hot paths, but every
miss and every uncached access pays it.

Watch for: JS-09 miss-path profiles. Pay with a per-shape lookup table.

The second half of this entry is paid: scope environments of compiled code
share the name list their function's code owns and box the fields only
function and global records need, so the environment of a closure that
captures one variable costs 64 bytes instead of 112.

### TD-105 — Test262 runner coverage

The script runner preserves its exact passing lists for `test/language`
and the installed areas of `test/built-ins`. The separate module runner
(JS-07) pins pass, fail and skip verdicts for the 817 files flagged
`module` in `test/language`. All 696 executed files pass.
The remaining 121 use proposal features. See the
[Test262 run](JS.md#static-module-loading-js-06) for the pinned revision
and reproduction commands.

`$262.createRealm` is implemented (JS-06 realms). Pay with `$262` support
for collection and test agents. `$262.evalScript` now runs UTF-16
script source against the current global environment. Extend the built-in gate
when new library areas land. Trigger on each implementation.

### TD-106 — No function source text (eval part paid)

**eval: Paid** (JS-05 part 2f). Direct and indirect `eval` run: a sloppy
direct eval makes the calling function's scope dynamic, so names it does not
declare resolve by name at run time, and the declaration checks of
EvalDeclarationInstantiation run before any binding is made. About 140
eval-code Test262 files still fail, mostly the `arguments` in parameter
defaults rule and async or generator bodies (JS-08).

Still open: `Function.prototype.toString` prints every function in the
`NativeFunction` form because source text is not kept. The specification
allows that only for built-in and host functions. Pay with: a source span per
function so `toString` can return its text. The WEB-05 inline fixtures
exercise execution and ordering without function source introspection.
Schedule source text with the expanded M9 Test262 gate, or earlier if a
selected page or WPT fixture depends on it.

### TD-107 — Complete collation data and matching

`String.prototype.normalize` is implemented for all four forms. IDNA and
JavaScript share the canonical tables and normalization code in
`rustic-base`. JavaScript also uses compatibility mappings and preserves
lone surrogates.

`Intl.getCanonicalLocales` canonicalizes locale lists. `Intl.Locale` exposes
identifiers, likely subtags and pinned locale preferences. Locale casing
applies Azerbaijani, Lithuanian and Turkish rules, using default Unicode
casing for other languages. `Intl.Collator` and `localeCompare` negotiate
the locale and collation options and compare CLDR root weights, with German
phonebook and search handling. Other locale tailorings remain open, including
European ordering (`eor`), locale-specific search, and synchronization with
`Intl.Locale.prototype.getCollations`. The root reader handles contiguous and
discontiguous contractions and a limited set of implicit ranges. Pay in JS-08
with generated tailoring rules, complete implicit weights and Unicode collation
conformance data.

### TD-108 — Date local time is a fixed offset

**Rescheduled 2026-11-20 (M6 completion):** Needs reviewed time-zone data, decided with the Intl data work (TD-107 family); no M6 network behavior depends on local time.

`Date` computes local time as UTC plus the offset the embedder passes to
`Agent::set_time_zone_offset` (zero by default). The Date local-time default does not select an IANA zone, so its offset
cannot change with daylight saving time or with a
zone's history, so local fields, `getTimezoneOffset` and parsed local times
are off by the DST shift for part of the year in most zones, and the
`toString` forms carry no zone name except for UTC. The engine embeds no
OS-specific time zone API, so the platform layer has to supply the data.

Pay with: a `LocalTimeZone` hook on the agent that maps a UTC time value
to an offset (and a local time to its candidate offsets for
`UTC(t)`, §21.4.1.26), filled by the shell from the OS or from a compact
TZif reader in `rustic-base`; Intl.DateTimeFormat (JS-08) shares it.
The WEB-05 local fixtures use the default UTC offset for deterministic
headless runs. Review the local-time hook at M6 entry before loading broader
page workloads, or earlier if a selected test depends on transition rules.

### TD-109 — Unhandled promise rejections are not reported (Paid)

A promise that is rejected with no handler attached stays silent: the
engine has no HostPromiseRejectionTracker (§27.2.1.9), so neither the
`reject` nor the later `handle` operation reaches the embedder. A page's
`unhandledrejection` and `rejectionhandled` events, and the console warning
other engines print, therefore cannot fire. Promise semantics are
otherwise unaffected; the `PromiseRecord` already knows whether it has
reactions, so the missing piece is the notification only.

Pay with: an `is_handled` flag on `PromiseRecord`, set by
PerformPromiseThen, and a host hook on the agent called from
RejectPromise and PerformPromiseThen that the shell batches per
microtask checkpoint (HTML §8.1.7.3). Trigger: the HTML event loop
(JS-09 bindings) or the first console that reports page errors.

**Paid** by JS-06 part 2: `RejectPromise` and `PerformPromiseThen` record
`reject` and `handle` events on the agent, the host takes them with
`Agent::take_rejection_events` at its microtask checkpoint, and a rejection
handled before then is withdrawn. The events root their promises until taken.
The HTML events themselves are M5 work.

### TD-110 — RegExp Unicode property escapes cannot run

**Rescheduled 2026-11-20 (M6 completion):** Needs Unicode property data shared with Intl (`js.conformance`); unrelated to networking.

The pattern parser accepts `\p{…}` and `\P{…}` in `u` and `v` mode and the
v-mode properties of strings, so such patterns are valid at parse time, but
the engine carries no Unicode Character Database tables. Compiling one
fails: a literal is a `CompileError::Unsupported`, and `new RegExp` throws a
SyntaxError that says so. The Test262 files under
`built-ins/RegExp/property-escapes` fail for this reason.

Pay with: generated, range-compressed tables for General_Category, Script,
Script_Extensions and the binary properties of ECMA-262 table 67-69,
built by a tool from a pinned UCD (as the URL crate pins its IDNA
data), and the sequence
properties for `v`. Weigh the size against the binary budget; the
tables are shared with rustic-text's segmentation data where they overlap.
Trigger: the first page that fails on a property escape, or JS-08
closing.

### TD-111 — The RegExp matcher backtracks (Paid)

`regexp::vm` is a backtracking matcher, as in the other engines, so a
pattern such as `/(a+)+b/` on a long run of `a`s takes exponential time.
The choice and trail stacks are capped (`MAX_BACKTRACK`, a RangeError past
it) so memory stays bounded, and the matcher polls the agent's step budget
and cancellation token every 16,384 steps, so a page's runaway pattern is
stopped like a runaway loop. Time is not otherwise bounded.

Pay with: a linear-time fallback (a Pike VM or lazy DFA) for patterns with
no backreferences or lookarounds, chosen when the backtracker exceeds a
step count, as V8's experimental engine does. Trigger: a measured slow
page or a ReDoS report.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): patterns with no backreference or lookaround also compile for a linear-time Pike VM (`crates/rustic-js/src/regexp/pike.rs`, `vm::compile_pike`: repetitions unrolled, an optional iteration that matches empty rejected by a `Progress` instruction, captures cleared by `Clear`). A search moves there after 65,536 backtracks summed over its start positions, or when the backtracking stacks would overflow, and resumes at the start position it reached. Threads keep the backtracker's priority order, so the match and its captures are the same; `the_linear_time_matcher_agrees_with_the_backtracker` compares both matchers alone at every start position, and `exponential_patterns_finish_in_linear_time` runs `(a+)+b`, `(a*)*b`, `(?:a|a)*b` and `^(\w+\s?)*$` over 5,000 characters in linear time (previously stopped only by cancellation). Test262 RegExp and String results are unchanged. Patterns with backreferences or lookarounds, and unrolled programs over 32,768 instructions, stay on the backtracker with its step budget, as in V8.

### TD-112 — BigInt size cap and quadratic arithmetic

A BigInt result may have at most 2²⁰ bits (`MAX_BIGINT_BITS`, 128 KiB of
limbs); a larger one throws the RangeError other engines throw at their own,
larger limits (2³⁰ bits in V8). Multiplication and division are the schoolbook
algorithms and decimal conversion is quadratic, and none of them reaches a
safe point, so the cap is what bounds the time one operator can take: about a
second for a worst-case multiplication at the limit. Sizes are checked before
any limb is allocated, and every result is charged to the heap budget.

Pay with: Karatsuba (and Toom-3 if measured) multiplication, a
divide-and-conquer radix conversion, polling the step budget inside long
operations, and then raising the cap. Trigger: a page or benchmark that needs
integers above 2²⁰ bits, or a measured hot spot in BigInt arithmetic (JS-09).

### TD-113 — for-in over a proxy enumerates eagerly

`for-in` collects the enumerable string keys of the object and its prototype
chain when the loop starts, then skips keys that are gone by the time they
come up. For ordinary objects nothing can observe the difference from the
specification's lazy `EnumerateObjectProperties`. For a proxy the `ownKeys`,
`getOwnPropertyDescriptor` and `getPrototypeOf` traps of the whole chain run
before the first iteration instead of interleaved with the body, and a key is
not asked about again. The set and order of visited keys are the same.

Pay with: a for-in iterator that walks one object at a time and calls
`[[GetOwnProperty]]` as each key comes up. Trigger: a script or Test262 file
that depends on the interleaving.

### TD-114 — Sloppy functions answer `null` for `caller` and `arguments`

The specification gives `%Function.prototype%` accessor properties `caller`
and `arguments` whose getter and setter are `%ThrowTypeError%`, and lets
sloppy functions have their own properties of those names; browsers give
them such properties, and scripts read `f.caller` and `f.arguments` without
expecting a throw. Own properties would cost every closure two slots, so
instead `%ThrowTypeError%` returns `null` when its receiver is a sloppy,
non-generator, non-async function compiled from source, and throws for every
other receiver (strict functions, classes, built-ins, arguments objects).
The visible differences from other engines: the properties are inherited
rather than own, and they are always `null` rather than the actual caller or
arguments (which other engines only provide without optimization).

Pay with: nothing planned; revisit only if a site needs the real caller.

### TD-115 — Shared memory stays inside one agent, and typed arrays have no fast path

`SharedArrayBuffer` and `Atomics` are installed since JS-08 part 7 with their
full single-agent semantics (see [JS.md](JS.md)). What is still missing is the
sharing itself: Rustic has one agent per heap and no workers yet, so a shared
buffer's bytes are a `Vec<u8>` owned by one heap, `Atomics.wait` can never be
woken, and the 112 Test262 `Atomics` files that start other agents through
`$262.agent` fail. The earlier form of this entry held the constructors back
for that reason; they are installed now because their behavior inside one
agent is fully specified and testable, and the host decides exposure. The
immutable ArrayBuffer proposal (`transferToImmutable`, `sliceToImmutable`,
`immutable`) is not implemented either.

A typed array element access takes the generic path: `[[Get]]` sees the exotic
flag, asks `exotic.rs`, and the element is decoded from the buffer's bytes.
That is correct for every case (detached and resized buffers included) but
several times slower than an indexed load should be.

Pay with: a shared, reference-counted byte store with real atomic accesses
and a waiter list shared between agents when workers arrive
(`storage.workers`, M9), and interpreter fast paths for typed array loads and
stores keyed on the element kind (`perf.interpreter`, M12).

### TD-116 — A rope's flat copy is charged late (Paid)

`Heap::string` flattens a rope behind `&self` (a `OnceCell`), because 141
call sites read strings through a shared borrow. It therefore cannot charge
the budget or fail. Instead the bytes are counted in `Heap::bytes_used` at
once, `Heap::concat` refuses a rope whose flat copy would not fit when it is
made, and the next allocation or interpreter safe point fails with
`OutOfMemory` if the heap is over its limit. Between those points a script
can hold more than its limit: straight-line code that reads several large
ropes made while the heap was emptier. Each such copy is at most the limit,
and none survives the next safe point without the run ending.

Pay with: a fallible `Heap::flatten(&mut self)` at the places that read
units, leaving `Heap::string` for strings known to be flat. Trigger: a host
that needs the limit to hold between safe points (a hard per-renderer cap in
M7), or a fuzz finding that reaches the overshoot.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): `Heap::flatten(&mut self)` charges a rope's flat copy before making it and fails with OutOfMemory past the limit. Reads flatten through it where they can fail: every built-in's `this` and arguments (`call_native`, `construct_native`), `ToNumber`, `ToPropertyKey`, loose equality and `<`, string indexing (`GetV`), `ToString` of an object (a `toString` result), and 41 reads inside built-ins of strings taken from objects (JSON, `join`, RegExp, URI, Intl and the rest). Strict equality compares a rope through its leaves without flattening it (`Heap::strings_equal`). `ToString` of a string value still returns the rope as it is, so `+` and template literals append without copying. `Heap::string` keeps the late copy behind `&self` only as a backstop, counted in debug builds: `reading_ropes_flattens_them_where_it_can_fail` runs 20 kinds of read and checks it is never used, and `flattening_ropes_never_passes_the_heap_limit` reads three large ropes in one expression and stops at the limit (before, the heap passed it until the next safe point). Test262 RegExp and String results are unchanged.

### TD-117 — List formatting context and best-fit locale matching

CLDR list templates are installed for all locale data in the pinned CLDR 48
main archive. Their selection uses locale, type, style and list length.
Language rules that inspect an adjacent string remain open, including
Spanish conjunction and Hebrew separator changes. Best fit shares the
prefix matcher and tries likely-subtag expansion when a prefix is absent.
It has no language-distance table or configurable embedder default.

Pay with contextual template selection for the affected languages and a
bounded locale-distance matcher shared by Intl services. Trigger when
implementing the next service that needs these rules or adding a conformance
case for a contextual form.

### TD-118 — Realms live as long as their agent

**Rescheduled 2026-11-20 (M6 completion):** Only frames create additional realms; frames arrive with ISO-01/M9 HTML work, and one realm per CLI run cannot leak.

`Agent::create_realm` adds a realm record that the collector treats as a
root: the realm's global object, global environment and named intrinsics
stay alive until the agent is dropped, even when no script or host value
can reach them. Objects find their realm by a 16-bit index, so a record
cannot be removed while any object might still name it, and at most 65,536
realms fit in an agent. Each realm also builds its whole standard library
up front (about 119 KB of heap budget).

A page that creates and discards many frames would grow without bound.
Nothing creates realms yet except tests and the Test262 host object.

Pay by freeing a realm's record and reusing its index once a collection
finds no live object of that realm, and by installing library objects on
first use if the per-realm cost shows up in frame-heavy pages. Trigger when
WEB-02 gives each same-origin frame a realm.

### TD-119 — Scoped styles recompute after DOM mutation

Root and limit selectors can read ancestors, siblings, descendants and
element state. The incremental style engine conservatively restyles the
document after a mutation when scoped style rules are present. That
preserves the reference cascade result without assuming which subjects the
changed boundary can affect. Scope matching checks ancestor candidates
against each rule and may rewalk the path to its root to check limits.

Pay by sharing scope membership for unchanged roots and tracking which DOM
changes invalidate roots, limits and the corresponding affected subjects.
Trigger when implementing nested scopes or collecting scoped-style
performance evidence, whichever comes first.

### TD-120 — Number formatting data size and ICU approximations

`Intl.NumberFormat` embeds `data/intl/number.bin` (3.1 MB), the number,
currency and unit data of every CLDR 48 locale, in every build. The relative
time patterns (`relative.bin`, 336 KB) and display names (`names.bin`,
4.5 MB) of every locale are embedded the same way, 8 MB in all. Rows are
already limited to what a locale does not inherit; currency and unit names
are most of the rest. The pack also carries the CLDR plural rules as text,
which `intl/plural.rs` interprets to choose compact, currency-name and unit
patterns, while `Intl.PluralRules` uses its own generated rule functions
(`intl/plural_rules/data.rs`) and its own decimal rounding
(`plural_rules/digits.rs` next to `intl/decimal.rs`).

Three behaviors are simplified where ECMA-402 leaves the result to the
implementation:

- Range formatting follows ICU's default collapse rule from the layers of
  each end rather than ICU's modifier objects, so a locale whose pattern
  mixes the sign and the currency in unusual ways can differ from ICU.
- CLDR currency spacing tests "letter or punctuation" with a small
  approximation of `[[:^S:]&[:^Z:]]` instead of the general category table.
- Best fit and lookup matching are the same likely-subtags matcher
  (see TD-117).

**Why bad:** binary size on every platform, including mobile; two rule
evaluators and two rounding implementations that must agree.

Pay with: one plural rule source and one decimal type shared by
PluralRules and NumberFormat; a locale-subset build option or loading the
pack from the platform layer; the general category table for spacing.
Trigger: the first binary-size budget (M12), or a formatting difference
from ICU reported by a page or a Test262 update.

### TD-121 — Segmentation is untailored and duplicated

`Intl.Segmenter` applies the default rules of UAX #29 in every locale.
Three things a browser does beyond them are missing:

- Chinese, Japanese, Thai, Lao, Khmer and Burmese text has no spaces
  between words; engines find words there with dictionaries. The default
  rules make each ideograph its own word and leave a run of Thai letters
  whole.
- CLDR's sentence break suppressions ("Mr.", "p.ej.") and its locale
  tailorings (for example Swedish and Finnish colons inside words) are not
  applied, so "Mr. Smith" ends a sentence after "Mr. ".
- The locale only matters for `resolvedOptions`.

`rustic-text` has its own grapheme cluster implementation and tables
(`segment.rs`), because `rustic-js` depends only on `rustic-base` and must
not pull in the font stack.

**Why bad:** word selection and counting differ from other engines for
East Asian text; two copies of the grapheme rules must both follow each
Unicode version.

Pay with: the segmentation rules and tables moved to `rustic-base` and
used by both crates, CLDR suppressions and tailorings from the pinned
`common/segments`, and a dictionary segmenter. Trigger: text selection by
word in the editing milestone, or a Test262 update that checks tailored
results.

### TD-122 — DateTimeFormat calendar and localization completeness

`Intl.DateTimeFormat` accepts Gregorian, ISO 8601, Buddhist and civil Islamic
calendars. Other calendar preferences fall back to the locale's supported
default. Date pattern selection currently uses Gregorian CLDR skeletons and a
component-distance matcher. Buddhist and civil Islamic dates derive their fields
from their own calendar arithmetic and use localized calendar names where the
data supplies them. The formatter uses a complete two-endpoint fallback when
a compact interval pattern cannot preserve the displayed fields.

The zone data uses the IANA 2025b default distribution and its link targets; it
does not add the optional pre-1970 `backzone` history. Named-zone display uses
available UTC names, short English abbreviations and localized GMT offsets.
Localized metazone, generic and long names still need the CLDR zone-name data.
The Temporal-dependent DateTimeFormat cases require a Temporal runtime.

Pay with: additional calendar arithmetic and calendar-specific patterns, pinned
CLDR metazone display names and full basic/best-fit pattern matching. Keep the
Test262 ratchet visible while these are added. Trigger: Intl conformance work
and locale-sensitive date display in the browser.

### TD-123 — Page script safe points need the joint collector (Paid)

The scripted document host now runs `DomHeap` at interpreter safe points,
including those reached by promise jobs. One tree and wrapper cache are
shared with the document scheduler. Tests under collection stress verify
connected expandos, detached nodes retained by jobs, and unreachable
cycles reclaimed by the joint sweep.
[ADR 0028](adr/0028-scripted-document-collection.md)

### TD-124 — DOMException stack, derived interfaces and serialization

Document realms install the base interface. Host quota limits throw the
legacy base `QuotaExceededError` name and code. The derived interface
requires numeric option conversion and its own quota and requested slots.
Add it when an exposed API can report those values. Capture DOMException
stacks alongside native Error stacks when script debugging adds that
storage. Serialize the exception's private name and message when the
structured clone capability lands.
[ADR 0027](adr/0027-host-exception-bindings.md)

### TD-125 — ARM64 benchmark timings are emulated (Paid)

The JS-09 gate asked for recorded benchmark results on x64 and ARM64. The
ARM64 record in the
[JS-09 report](reviews/2026-11-15-m4-js09.md#linux-arm64-under-qemu-emulation)
comes from QEMU user-mode emulation on an x86-64 host, because GitHub Actions
could not start jobs and the sandbox has no ARM64 hardware. MoeJoe accepted
this record for M4 on 2026-11-16. Emulation proves the results that do not
depend on the processor: checksums, peak heap sizes and collection counts
match x86-64. Its times and pauses say nothing about real ARM64 speed. Record
the corpus on native ARM64 hardware (the `Linux ARM64` CI job already prints
it to the job summary) before any ARM64 performance claim, at the latest with
the first cross-engine report of PERF-01.

**Rescheduled 2026-11-19 (M5 completion review):** M5 closed without native ARM64 hardware; the first PERF-01 report is x86-64 only. Moved to the M8 row, where reference devices are chosen.

**Note 2026-11-22:** the first green hosted run on `moejoe873/Rustic` (run
45003031562, `feature` at `9961c1c`) printed a native Linux ARM64 record in
the `Test (Linux ARM64)` job summary. Committing that record to
[JS.md](JS.md) with its exact runner, CPU and toolchain is the first step of
paying this entry.

**Paid 2026-11-23** (M8 workstream 5): the Performance workflow ran the JS-09 corpus natively on Linux ARM64 (Neoverse-N2), Windows ARM64 (Cobalt 100) and macOS ARM64 (Apple M1) runners next to V8 and, on macOS, JavaScriptCore; the timings and ratios are in [the M8 reference-device report](reviews/2026-11-23-perf-01-m8-reference-devices.md), and every weekly run repeats them. The QEMU record in the JS-09 report stays as history for checksums and heap sizes only.

### TD-126 — Initial DOM prototype and construction coverage

The page host supports detached HTML and XML documents, explicit and
automatic cross-document node moves, and namespace-aware element and
attribute operations. Element creation provides Element, HTMLElement and
HTMLInputElement prototypes, as applicable. Geometry rectangle interfaces
are bound by hand.
Add Attr nodes and NamedNodeMap with the DOM attribute wrappers, and add
HTML element-specific prototypes and custom element reactions with the
HTML interface and registry work.
[ADR 0029](adr/0029-scripted-node-creation.md),
[ADR 0033](adr/0033-detached-documents-and-adoption.md)

**Rescheduled 2026-11-19 (M5 completion review):** M5 exposes the interfaces its gate needs (WEB-01 to WEB-07). The rest of the HTML and DOM surface is M9 compatibility work, so the review moves to the M9 row.

### TD-127 — Element and attribute names use scalar UTF-8 storage

Node and attribute payloads preserve every UTF-16 code unit. QualifiedName
stores names as UTF-8 because parsing, matching and styling consume scalar
names. The new script factories reject unpaired surrogates in names on
creation. Attribute lookups for such names find no matching record. Update
name storage and its parser, selector and serializer consumers together
when name conformance requires representing those code units.
[ADR 0029](adr/0029-scripted-node-creation.md)

### TD-128 — Live NodeList positional access walked child links (Paid)

Reading length or a later index originally walked the current sibling chain.
A page-local positional index now retains the last requested root's child
and element-child IDs. The document child-link revision invalidates the
index on structural edits. After a scan, subsequent reads on that root are
constant time while the cache remains valid.
[ADR 0031](adr/0031-live-html-collections.md)

### TD-129 — Descendant and named collection reads scan live nodes and attributes

Filtered descendant reads and named collection reads scan their current
source, so attribute changes and reordering become visible immediately.
The scans are metered against the script turn. Iterating a descendant view
can repeatedly walk the same subtree, and changing collection roots can
cause repeated direct-child index rebuilds. Use measured collection traces
to guide a bounded, mutation-versioned filter or name index. Preserve live
behavior through attribute changes, subtree moves, and potential reentrancy
during named property visibility checks.
[ADR 0031](adr/0031-live-html-collections.md)

**Rescheduled 2026-11-19 (M5 completion review):** No M5 profile shows collection scans as a cost (PERF-01 DOM microbenchmarks include `querySelectorAll`). Moved to the M9 row with the DOM performance work.

### TD-130 — Window named access and additional browsing contexts

**Rescheduled 2026-11-20 (M6 completion):** Named properties, navigation and origin checks on Window need the browsing-context and agent-cluster model ISO-01 designs; M6 has one top-level document per run.

The page host has one top-level browsing context per script agent and a
fixed WindowProxy target for the active document's lifetime. This gives event and
script integration one global identity. Extend the Window prototype chain with named access alongside the remaining
HTML APIs. Before exposing related browsing contexts or navigation, add
lifetime-aware proxy retargeting and implement origin-sensitive property
access. Test context identity across navigation and cross-origin property
descriptors with the corresponding Web Platform Tests.
[ADR 0032](adr/0032-top-level-window-realm.md)
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 language and web APIs row: named access and related browsing contexts need cross-renderer WindowProxy forwarding (TD-178); both are web-API work in M9.

### TD-131 — Extended input and event delivery

WEB-04 now dispatches trusted primary clicks and key events from the host
queue, with button keyboard activation and reversible checkbox activation.
Focus, Tab navigation and click focusing landed with WEB-07
([ADR 0049](adr/0049-focus-and-forms.md)); pressed-key reset on focus
changes remains.
Extend the input model for pointer sequences, mouse and pointer attributes,
text input, IME, and additional control defaults when the corresponding host
and form APIs land. HTML input cloning still needs to copy mutable
checkedness and indeterminateness. Add signal-driven listener removal with
AbortSignal. Listener, observer, timer and script exceptions now reach HTML
error reporting and the console sink ([ADR 0048](adr/0048-error-reporting-and-event-handlers.md));
debugging hooks remain.

Extend the path to handle shadow retargeting, connected shadow input
targets, and scoped composed paths when exposing shadow trees in M9.
Listener registration and matching walk the current target's list under the
script budget. Consider indexing when measured listener workloads warrant it.
[ADR 0034](adr/0034-dom-event-dispatch.md),
[ADR 0035](adr/0035-trusted-input-and-activation.md)

**Rescheduled 2026-11-19 (M5 completion review):** Error reporting, focus and Tab navigation were paid in WEB-07 (ADR 0048, ADR 0049). The remaining pointer, IME, `AbortSignal` and shadow retargeting work is M9.

### TD-132 — Timer and frame integration with page policy

Window timers start after the registering task's or frame's completed
checkpoint. Use registration-time monotonic deadlines when the browser
event loop owns an injectable clock for both registration and pumping.
The headless visibility getters treat the active document as visible.
Connect background throttling and visibility changes when the host adds
activity and suspension. Route string timer source through the page's CSP and
Trusted Types policies when those policies are available. Frame timestamps
currently use the host's clock origin. Unify them with the page Performance
time origin and its precision policy when that API lands.

Frame callbacks run before the renderer. WEB-06 adds layout and style
flush ordering, and WEB-07 adds HTML error events and observer delivery.
Until then the engine reports uncaught animation errors through a host
counter. Test the integrated event loop against the matching Web Platform
Tests when these facilities are present.
[ADR 0036](adr/0036-window-timers-and-animation-callbacks.md)

**Rescheduled 2026-11-19 (M5 completion review):** Timer and animation callback exceptions now reach HTML error reporting (ADR 0048). Host-time frame timestamps and timer binding remain and move to M9 with the event loop work.

### TD-133 — Outward adoption during parsing and dynamic document markup

The WEB-09 review narrowed the adoption restriction to subtrees containing
identities retained by the parser or running or pending scripts. Both pinned
detached-document event propagation cases now pass. An adoption that would
move retained identities throws `NotSupportedError` before touching either
document. HTML `document.write` and `document.writeln` raise
`NotSupportedError` because the parser has no bounded insertion-point stack
for reentrant writes and no document-open replacement lifecycle.

Pay with the M9 HTML lifecycle work. Cover
identity-preserving adoption of parser-held nodes, nested insertion and
reentry limits, and the lifetime of a document replaced by `document.open`.
[ADR 0038](adr/0038-parser-inserted-inline-classics.md)

### TD-134 — Classic completion and the page event loop (Paid)

**Rescheduled 2026-11-20 (M6 completion):** The CLI now loads subresources over HTTP one at a time (TD-165); stylesheet-blocking scripts and task interleaving matter once loads run concurrently on the network process, which M7 introduces.

The opt-in local host delivers request completions explicitly. Async classics
execute when delivered, and document publication waits for EOF and all
results. Script element events and readiness transitions landed in
[ADR 0041](adr/0041-scripted-document-readiness.md).
Integrate stylesheet readiness as a blocker and order timers with pending
fetches when WEB-06 connects the rendering lifecycle. Add tests where a
stylesheet, script or timer becomes ready while the parser is suspended.
[ADR 0039](adr/0039-local-external-classics.md)

**Rescheduled 2026-11-19 (M5 completion review):** Stylesheet-blocking scripts and task interleaving depend on asynchronous resource loading, which arrives with NET-01; moved to the M6 entry row.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): The CLI's scripted load now services script requests through `ScriptRequests` (`crates/rustic-cli/src/loader.rs`). A network document starts every requested script, and the stylesheets parsed before them, side by side (TD-165's fetch controller, in process and through the broker). Before a parser-blocking or deferred script runs, the stylesheets in the tree parsed so far are loaded into the applied snapshot (HTML's script-blocking style sheets; the engine exposes the partial tree as `ClassicScriptLoad::live_document`). Timers that fall due while a request is outstanding run before its completion: a network document's clock moves by the time spent waiting, a local file's only runs timers already due. Test: `scripts_wait_for_earlier_stylesheets_and_timers_run_while_they_load` (rustic-cli `tests/network.rs`, both process modes).

### TD-135 — Dynamic page imports, import maps and data modules

**Rescheduled 2026-11-20 (M6 completion):** Dynamic `import()` over HTTP needs the event-driven fetch controller (TD-161, TD-169); import maps are an HTML-integration item of M9.

The local page host accepts static JavaScript imports using URL specifiers.
Connect dynamic import to the shared page module map when the M6 fetch layer
can deliver results independently of the parser. Add import-map resolution,
data module types and MIME checks with the corresponding conformance
selection. The mixed-host tests cover local JavaScript only.
[ADR 0040](adr/0040-local-module-scripts.md)

### TD-136 — URI functions and global Test262 coverage (Paid)

The M4 audit found that `encodeURI`, `decodeURI`, `encodeURIComponent` and
`decodeURIComponent` were missing. All four now work over UTF-16 strings
and throw intrinsic `URIError` values for malformed sequences. The pinned
built-ins gate adds all four URI directories, `Infinity`, `NaN`,
`undefined`, `eval` and `ThrowTypeError`, with 222 additional passes and
no previous passing files lost. The standard budget covers 171 of the 173
URI files. The two exhaustive supplementary-plane cases pass in a separate
test with a 400-million-step limit.
[Test262 conformance](JS.md#conformance)

### TD-137 — Web-legacy library surface

**Rescheduled 2026-11-20 (M6 completion):** Web-legacy library surface is measured by `test/annexB`, which the M9 Test262 full-rate work runs; M6 fixtures do not use it.

Browsers keep a layer of library features that pages still use and that
the specification puts in Annex B or that are de facto standards. Rustic
has the Annex B pieces the parser and `Date` need (`substr`, `getYear`,
`RegExp.prototype.compile`, HTML-like comments) but not:

- `escape` and `unescape` (§B.2.1);
- the String HTML methods `anchor`, `big`, `blink`, `bold`, `fixed`,
  `fontcolor`, `fontsize`, `italics`, `link`, `small`, `strike`, `sub` and
  `sup` (§B.2.2);
- `Object.prototype.__defineGetter__`, `__defineSetter__`,
  `__lookupGetter__` and `__lookupSetter__` (§B.2.2.2–5, 54 Test262 files in
  `Object/prototype`);
- the legacy RegExp statics (`RegExp.$1`–`$9`, `lastMatch`, `input` and
  the rest), a stage 3 proposal that documents what every engine ships;
- `Error.prototype.stack` (34 Test262 files, `error-stack-accessor`). TD-124
  covers DOMException stacks only.

The gates run `test/language` and selected `test/built-ins`, never
`test/annexB`, so nothing measures this. Pay before M6 entry: add the
missing pieces, run `test/annexB` as a pinned passing list, and give
`stack` a format shared with error reporting (TD-131).

### TD-138 — Completion values and binding TDZ

**Rescheduled 2026-11-20 (M6 completion):** Spec-edge completion values and TDZ checks are tracked by failing Test262 files in the committed lists; fixing them belongs to the `js.conformance` full-rate gate, not the network milestone. Still bad debt: prioritise first in that gate.

The audit's run of `test/language` at the pinned revision found two groups
of wrong results with no error:

- **Completion values** (57 files, `cptn-*` and
  `statements/try/completion-values.js`). Loops, `if`, `switch` and `try`
  do not reset the completion value to `undefined` where §14 requires, so
  `eval("1; for (var i = 0; i < 2; i++) {}")` returns 1 instead of
  `undefined`, and `break`/`continue` out of an empty `catch` or `finally`
  leak an older value. It only shows through `eval` and the console.
- **Missing TDZ errors** (56 files). A parameter default that reads a later
  parameter or itself (`function f(a = b, b) {}`) reads `undefined`, and a
  `for (let x of x)` head reads the outer binding instead of throwing a
  `ReferenceError`.

Pay before M6 entry, with compiler tests for each statement kind.

### TD-139 — Reference evaluation order

**Rescheduled 2026-11-20 (M6 completion):** Reference evaluation-order deviations are tracked by failing Test262 files; scheduled first in the `js.conformance` gate. Still bad debt.

The compiler evaluates some operand pieces in a different order from the
specification (65 `test/language` files):

- compound, update and logical assignment (`o[k] += v`, `o[k]++`,
  `o[k] ??= v`) convert the key or read the binding at the wrong step, so a
  throwing `toString`, a `null` base or a binding removed from a `with`
  object show the wrong error or value (`S11.13.2_A6/A7`, `S11.3.x_A6`,
  `S11.4.x_A6`, `lhs-before-rhs`);
- `with` over a proxy calls `has` and `@@unscopables` more often than
  required;
- keyed destructuring targets and `super[expr]` bases are evaluated after
  the source value instead of before;
- `delete super.x` must throw a `ReferenceError` and does not, and assigning
  through `super` to a non-writable property in sloppy code throws.

Pay before M6 entry: real pages mostly cannot observe this, but proxies,
getters and `with` libraries can.

### TD-140 — Destructuring assignment does not close iterators on target errors

**Rescheduled 2026-11-20 (M6 completion):** Tracked by failing Test262 files; scheduled first in the `js.conformance` gate. Still bad debt.

TD-100 paid the binding form. The assignment form (`[o[f()]] = iterable`,
and `for ([a.b] of …)` heads) calls `next` before evaluating the target
reference and does not call `return` when evaluating that reference
throws, so iterators and generators are left open (32 Test262 files under
`expressions/assignment/dstr` and `statements/for-of/dstr`). Pay before M6
entry by giving assignment targets the same close handler as bindings and
evaluating the reference first.

### TD-141 — Class, function and direct-eval edge semantics

Remaining `test/language` failures outside `eval-code`, modules and TD-138
to TD-140:

- a derived constructor whose `return` override runs inside `catch`,
  `finally` or `for-of` reports the wrong error (11 files);
- method and function names from symbol keys and cover grammars
  (`fn-name-*`, `name-prop-symbol`, 16 files);
- private fields added to non-extensible objects (the 2025 rule) and
  private accessors that are not own properties;
- direct `eval` inside class field initializers and methods cannot see
  `super`, private names or the `arguments` restriction (66 files), and
  `eval` var-scope early errors in parameter scopes are not raised. TD-106
  covers the `eval-code` directory itself.

Pay with the `js.conformance` (M9) Test262 full-rate work, or earlier if a
selected page needs one.

### TD-142 — Built-in edge bugs (Paid)

Wrong behavior in shipped built-ins, found in the `test/built-ins` run:

- ~~`WeakMap`, `WeakSet`, `WeakRef` and `FinalizationRegistry` accept
  registered symbols (`Symbol.for`), which `CanBeHeldWeakly` forbids.~~
  Paid 2026-11-19: the 8 files pass.
- ~~`%StringIteratorPrototype%[@@toStringTag]` is `"Array Iterator"`.~~
  Paid 2026-11-19: string iterators have their own prototype, tag and
  `next` (2 files).
- ~~`Object.fromEntries` does not close its iterator when an entry is not an
  object or its `next`/`done` throws (11 files).~~ Paid 2026-11-19: it read
  its argument as an array-like rather than an iterable; it now uses
  `AddEntriesFromIterable` (11 files).
- ~~`Object.setPrototypeOf(Object.prototype, x)` succeeds for a non-circular
  `x`.~~ Paid 2026-11-19: `Object.prototype` is an immutable prototype
  exotic object (2 files).
- ~~`length` definitions on arrays convert the value in the wrong order; a
  `DataView` over a resizable buffer reports bounds errors from
  `byteLength`/`byteOffset`; `Object.freeze` accepts a length-tracking typed
  array.~~ Paid 2026-11-19 (9 files): `ArraySetLength` converts twice and
  reads `length` afterwards, out-of-bounds views report 0, and typed arrays
  whose length can change refuse `[[PreventExtensions]]`.

**Paid 2026-11-19** in full: 32 more `test/built-ins` files pass. The
`DataView` `immutable-buffer` failures belong to the immutable ArrayBuffer
proposal (TD-143).

### TD-143 — Recent-edition and stage 3 library features

TD-89 covers proposal syntax. The library has no iterator chunking, joint iteration,
`includes` and `join` (about 170), immutable `ArrayBuffer` (43),
`ShadowRealm` (60) or Temporal (4,614 built-in and 2,111 `intl402` files).
None of these files are skipped: they fail and stay out of the passing
lists. Pay per ECMAScript edition as each reaches stage 4, with the M9
`js.conformance` work; Temporal also settles the DurationFormat and
DateTimeFormat gaps in TD-122.

**Paid for `Math.sumPrecise` (2026-11-23).** All 10 files now pass in the
pinned built-ins selection. See [JS.md](JS.md#mathsumprecise-m9).

**Paid for binary and JSON data interchange (2026-11-23).** All six
`Uint8Array` encoding methods, JSON reviver source text, `JSON.rawJSON` and
`JSON.isRawJSON` are implemented. Two pinned `Uint8Array` cases remain with
the immutable-buffer proposal because their fixtures call `transferToImmutable`.
See [JS.md](JS.md#uint8array-and-json-data-interchange-m9).

**Paid for `Array.fromAsync` (2026-11-23).** All 95 files pass in the
pinned built-ins selection. See [JS.md](JS.md#arrayfromasync-m9) and
[ADR 0070](adr/0070-native-promise-continuations.md).

### TD-144 — No proper tail calls

The specification requires calls in tail position of strict code not to
grow the stack. Rustic, like V8 and SpiderMonkey, keeps a frame per call,
so the 34 `tco` Test262 files throw a `RangeError` at the frame limit.
Kept by design: proper tail calls would hide frames from error stacks and
devtools. Revisit only if the committee drops the requirement or other
engines ship it.

### TD-145 — Test262 gates are manual and x86-64 only

The parse, script, module and Intl gates need a Test262 checkout, so no
workflow runs them. They run by hand in the sandbox (x86-64, release), and
`docs/JS.md` says the lists are not checked on other targets. Nothing shows
the passing lists hold on 32-bit, ARM64 or big-endian (s390x) builds, where
NaN-boxing, `libm` results for `Math` (allowed to differ in the last bit)
and endian-sensitive buffer code differ. Pay by adding a
job that fetches the pinned revision, runs the gates on x86-64 and ARM64,
and runs at least the built-in gate under the emulated tiers.

### TD-146 — Legacy multi-byte encodings are decoded byte at a time

`rustic-encoding` runs each decoder as the Encoding Standard's handler, one
byte per call, and each encoder one scalar value per call
([ADR 0043](adr/0043-encoding-standard.md)). ASCII runs and valid UTF-8
prefixes are copied in bulk, so UTF-8 and mostly-ASCII pages are fast, but
a page in Shift_JIS, GBK, Big5 or EUC-KR pays a branchy state machine and
an index read per character, and the first use of each multi-byte encoder
builds a sorted reverse index (up to about 100 KB). Nothing has measured
this against other engines. **Pay:** add legacy-encoding pages to the
PERF-01 corpus, and if decoding shows on a profile, add table-driven or
vectorized paths with the fuzz target's chunked-versus-whole differential
as the guard. **Trigger:** M12 performance work, or a profile showing
decoding on a critical path earlier.

### TD-147 — The default depth limit flattens earlier than Chromium

[ADR 0046](adr/0046-parse-budget-degradation.md) inserts nodes past
`max_depth` the way Chromium does, and with a 514-edge limit the trees are
identical. The default is still 256 edges, because the layout tree has its
own 256-edge limit (`LayoutLimits::max_depth`) and the paint, bindings and
emulated tiers have no evidence at 514 levels. A page between 256 and 514
levels deep therefore loads, but flattened earlier than in Chromium.
**Pay:** raise `DocumentLimits::max_depth` and `LayoutLimits::max_depth` to
514 together, with deep-tree layout, paint and binding tests on every tier.
**Trigger:** M9 layout work, or a real page that renders differently
because of the cap.

### TD-148 — Computed-style snapshots are eager and copied per read (Paid)

The CLI's `LayoutSource` builds `LayoutGeometry::computed_styles` on every
layout flush, including flushes caused only by `offsetWidth` or
`getBoundingClientRect`. It serializes each of the ~110 supported longhands
to a `String` for every styled element, whether or not a script ever calls
`getComputedStyle`. Each declaration read (`getPropertyValue`, a named
getter, `item`, `length` or an index) then clones that element's whole
`ComputedStyleSnapshot` out of the cache. Cost is bounded by the element
and per-turn flush budgets, so it is acceptable for local fixtures, but it is
O(elements × properties) allocation per flush and O(properties) per read.
**Pay:** keep the `ComputedStyle` values in the snapshot and serialize one
property on demand (or build an element's record lazily on first read), and
return borrowed values from `PageTree::computed_style`. Keep the CSSOM WPT
cases and add a flush-cost test on a large document.
**Trigger:** M7 entry, or profiling a page with many elements and layout reads.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): `ComputedStyleSnapshot` is now a shared, lazily built record: the CLI's `LayoutSource` stores, per styled element, a closure over its `ComputedStyle` and CSSOM box, so a flush caused by `offsetWidth` or `getBoundingClientRect` serializes nothing. The first declaration read builds that element's `ComputedStyleRecord` once, and every later read (`getPropertyValue`, a named getter, `item`, `length`, an index) clones only a reference to it. Test: `lazy_snapshots_serialize_once_on_first_read_and_share_clones` in `crates/rustic-bindings/src/page.rs`; the CSSOM WPT selection and the CLI computed-style tests pass unchanged. Serializing one property at a time instead of an element's whole record is left to M12 `perf.style`.

### TD-149 — Pseudo-element `getComputedStyle` throws

**Rescheduled 2026-11-20 (M6 completion):** Pseudo-element computed styles need generated-content layout (M9); returning empty styles would be wrong for `::before`/`::after` with content.

`getComputedStyle(element, pseudo)` uses the element when `pseudo` has no
leading colon, as CSSOM requires, but throws a `TypeError` for every
colon-prefixed value. CSSOM instead returns the pseudo-element's styles for
supported pseudo-elements (`::before`, `::after`, `::marker`, ...) and an
empty read-only declaration for invalid ones, so pages that probe
`::before` fail with an exception instead of reading values.
**Pay:** parse the argument as a `<pseudo-element-selector>`, return an
empty declaration for failures, `::slotted()` and `::part()`, and read
generated-box styles from the same snapshot for supported pseudo-elements,
with WPT `getComputedStyle-pseudo*` cases.
**Trigger:** WEB-06 completion, or before M6 loads server pages.

**Rescheduled 2026-11-19 (M5 completion review):** Pseudo-element styles need generated-box styles in the layout snapshot; moved to the M6 entry row, matching its own trigger (before M6 loads server pages).

### TD-150 — Dead mutation-observer registrations are dropped lazily (Paid)

A registration is an edge held by its node ([ADR 0047](adr/0047-mutation-observers.md)).
`disconnect()` and the end of a transient registration mark the record dead
instead of finding its node, because the observer deliberately keeps no
strong reference to observed nodes and node IDs change on adoption. The
node drops the dead edge when a later mutation walks it, or when the node
is collected. Separately, a live observer that observes many short-lived
nodes and never disconnects keeps one small record per node it ever
observed in its own list. Both are bounded by script's own allocations and
charged to the JS heap, but they are memory nothing will use again.
**Pay:** give each registration a weak back-reference the collector clears
(or a per-observer compaction at collection time), and drop dead edges
during `disconnect` when the node is still reachable.
**Trigger:** M7 entry, or a heap report that shows observer records
dominating a page.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): an observer now holds *weak* references to its registrations, so a registration dies with the node that holds it, and the observer's list is compacted when it holds more than twice the registrations live at the last compaction. `disconnect`, re-observing a node and the end of a transient registration remove the registration's edge from the node it was attached to (a side map from registration to node, pruned as it doubles), instead of leaving it for the next mutation. A node that was adopted after it was observed has a new ID, so its dead edge is still dropped lazily, when the next mutation walks it or the node is collected. Test: `observing_many_short_lived_nodes_does_not_retain_their_registrations` in `crates/rustic-bindings/tests/mutation_observer.rs` (3,000 dead nodes: the old list grew by 408 KB, now under 64 KiB); the Window WPT selection is unchanged at 387/462.

### TD-151 — Mutation records for parser and host edits resolve observers when drained

Binding operations drain the document journal when the outermost call
releases the page, so their records see the tree as it was changed. The
HTML parser and host code (checkbox activation, resource completion) edit
the tree outside a binding call; their entries are interpreted at the next
binding call or microtask checkpoint, and interested observers are found
from the target's ancestors *then*. A node the parser inserts and later
moves within the same chunk (adoption agency, foster parenting) can be
attributed to its later ancestors, and parser text appends copy the old
character data for every append while an observer exists.
**Pay:** drain the journal at each parser insertion step that changes an
existing node's ancestry, or record the ancestor chain's registration
snapshot in the entry; coalesce parser text appends into one record per
token run.
**Trigger:** M9 HTML lifecycle work, or a WPT parser-mutation case that
depends on it.

### TD-152 — Error reports lack source locations and console formatting

**Rescheduled 2026-11-20 (M6 completion):** M6 now tracks document URLs, but script URLs, lines and `location` belong to the History/Location bindings and DEV-01 error display, not to the network gate.

`ErrorEvent.filename` is empty and `lineno` and `colno` are zero, because
script sources carry no URL and thrown values no position the host can
read; `window.location` does not exist yet. The console joins arguments
with spaces and does not interpret `%s`, `%d`, `%o` or `%c`, and objects
appear as `[object Object]` or `Name: message` rather than an inspectable
preview ([ADR 0048](adr/0048-error-reporting-and-event-handlers.md)).
**Pay:** record the script URL and the throw position in the VM, pass them
through `report_exception`, add `location` with navigation, and implement
the Console Standard's formatter with side-effect-free previews.
**Trigger:** M6 navigation, or the DevTools work in M10.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 language and web APIs row: script URLs and throw positions belong to the VM work of the M9 language row (`js.conformance`), and console formatting to the same bindings.

### TD-153 — Event handler content attributes and remaining handler names

Event handler IDL attributes exist for a subset of `GlobalEventHandlers`
plus Window's rejection handlers. Content attributes such as
`onclick="…"` are not compiled into handlers, `<body>` and `<frameset>` do
not forward Window handlers such as `onload`, and handlers like
`onmessage` and the animation, pointer and drag families are absent.
Separately, `replaceChild` with a node from another document records an
insertion and a removal instead of the DOM's one combined record, and
`normalize()` validates each merge on its own, so a document at its data
budget can be left part-normalized when it throws.
**Pay:** add a scoped function compilation entry point in `rustic-js`
(element, form owner and document scopes), compile on first use with the
raw-handler rules, forward the body handlers, generate the handler list
from the pinned IDL, adopt then replace in one DOM operation, and
preflight the total normalized size.
**Trigger:** M9 HTML lifecycle work, or the first WPT case that needs one.

### TD-154 — Form and focus gaps outside the M5 scope

WEB-07 implements focus, text editing at the end of the value, form owners,
constraint validation and submission without navigation
([ADR 0049](adr/0049-focus-and-forms.md)). Not done: `:focus`,
`:focus-within`, `:focus-visible`, `:valid`/`:invalid` and dirty
checkedness are not passed to style, and focus changes do not invalidate
frames; control values and carets are not painted; `select`/`option`,
`label`, `output`, `autofocus`, selection APIs and caret movement are
missing; `pattern` (needs a `v`-flag RegExp from the bindings), `step` and
`badInput` validity are always false; removing the focused element moves
focus without firing `blur`; radio groups do not move with arrow keys.
**Pay:** feed a per-element state set from the bindings into the style
view and the frame revision, paint text control values, add the remaining
controls and the selection model, and compile `pattern` through the JS
RegExp engine.
**Trigger:** M9 HTML lifecycle and forms work, or the first WPT form case
that needs one.

### TD-155 — ResizeObserver lifetime and geometry approximations

The page holds an observer while it observes anything, and the observer
holds its targets, so an observer script dropped without `disconnect` and
its targets live until the page closes. `devicePixelContentBoxSize`
equals the content box (one device pixel per CSS pixel), inline and block
sizes are width and height (no vertical writing modes), inline boxes and
SVG elements report zero, and `IntersectionObserver` is not implemented
([ADR 0050](adr/0050-resize-observer.md)).
**Pay:** make observations weak with collector support for active
observers, pass the device scale and writing mode in `BoxSizes`, measure
inline and SVG boxes, and add `IntersectionObserver` with scrolling.
**Trigger:** M9 layout and scrolling work, or a heap report showing
retained observers.

### TD-156 — One thread and one connection per exchange

`rustic-net` runs every exchange on its own OS thread with a fresh TCP
connection and sends `Connection: close` ([ADR 0052](adr/0052-network-io-model.md)).
There is no connection pool, no keep-alive reuse, no HTTP/2 or HTTP/3, and
`TcpStream::connect_timeout` cannot be interrupted, so cancelling during a
connect waits up to `connect_timeout`. This is correct and bounded, but
costs a handshake and a thread per request.
**Pay:** a bounded per-origin pool with keep-alive (after the parser has
proved message-boundary handling under reuse), a readiness-based I/O loop
or reviewed runtime if thread count shows in measurements, and HTTP/2 via a
reviewed library or own code with its own ADR.
**Trigger:** M12 page-load measurements, a page corpus with more than ~64
parallel subresources, or HTTP/2 scheduling.

### TD-157 — No DNS: only allowlisted loopback origins are reachable

`NetworkPolicy` maps allowlisted origins to loopback addresses; there is
no name resolution and no route to any other host
([ADR 0051](adr/0051-transport-and-tls.md)). This is the intended
pre-M7 authority model.
**Update 2026-11-20:** the TLS half is paid: `https` now uses rustls with
platform certificate verification (`rustic_net::tls`, `tests/tls.rs`), so
the entry moves from the M6 networking row to M7 entry, where the broker
owns name resolution.
**Pay:** add DNS (system resolver or a reviewed resolver) behind the M7
network broker capability, with the allowlist replaced by broker policy.
**Trigger:** M7 entry (remote browsing) or the first non-loopback test
host.
**Paid 2026-11-22 (ADR 0066):** `--allow-origin ORIGIN` lets one origin,
and `--allow-remote` every origin, resolve through the system resolver in
the broker (`NetworkPolicy::address`). A resolved name or IP literal
reaches only public unicast addresses (`rustic_net::is_public`: no
loopback, private, CGNAT, link-local, unique-local, documentation,
multicast or reserved ranges, including mapped forms), so DNS cannot be
turned against the user's machine or network; the network process
re-checks every address and never resolves. Evidence:
`resolved_origins_reach_only_public_addresses`,
`special_purpose_addresses_are_not_public`,
`resolved_origins_never_reach_local_addresses`.

### TD-158 — Content codings are not decoded (Paid)

**Paid 2026-11-20:** `rustic-net` advertises `Accept-Encoding: gzip, deflate, br` (not on range requests) and undoes `gzip`/`x-gzip`, `deflate` (zlib, or raw DEFLATE) and `br` in `Content-Encoding` order under `max_body_bytes`; unknown codings pass through. gzip is new RFC 1952 code in `rustic-codecs`; brotli streams through the already-reviewed `brotli-decompressor` without its `std` feature. gzip and deflate still buffer (TD-162). Evidence: `coding.rs` tests, `content_codings_are_advertised_and_decoded`.

Requests carry no `Accept-Encoding`, and a `Content-Encoding: gzip`,
`deflate` or `br` response body reaches the consumer still encoded. Real
servers mostly honour the missing header, but some send gzip regardless.
**Pay:** decode gzip and deflate with `rustic-codecs`' bounded inflate in
streaming form (and brotli through the reviewed decompressor already in
the font graph, or a new review), with output limits and ratio checks
counted against `max_body_bytes`, then advertise them.
**Trigger:** NET-03 (Fetch), which must expose decoded bodies.

### TD-159 — No bundled Public Suffix List (Paid)

**Paid 2026-11-20:** the Public Suffix List is vendored at publicsuffix/list `d7d7f1fd` (`crates/rustic-net/data`, `tools/import_public_suffix_list.py`) and `PublicSuffixList::bundled()` passes all 78 upstream test vectors; the cookie jar defaults to it.

`rustic_net::site::PublicSuffixList` implements the list's algorithm, but
Rustic ships no list ([ADR 0054](adr/0054-sites-and-cookie-policy.md)).
`PublicSuffixList::fallback()` applies only the implicit `*` rule, so
`a.example.co.uk` has registrable domain `co.uk`: two unrelated
`co.uk` sites would be same-site, and `Domain=co.uk` would be accepted.
Only allowlisted loopback origins are reachable today (TD-157), where the
fallback is exact.
**Pay:** the maintainer decides ADR 0054's proposed option; then vendor a
pinned list with a digest-checked import script (or another reviewed
source) and an update path, and make the fallback unreachable for remote
origins.
**Trigger:** before DNS or any non-loopback origin (M7 entry), or the
first test that needs a multi-label suffix.

### TD-160 — Cookies and preflight cache are per fetcher, in memory (Paid)

**Paid 2026-11-20:** persistent profiles keep the cookie jar's persistent cookies (`CookieJar::export_persistent`/`import_persistent`, revalidated on import) in a checksummed profile record, and the CORS-preflight cache is keyed by the network partition key (`PreflightCache::insert_in`). Evidence: `persistent_cookies_round_trip_and_session_cookies_do_not`, `preflight_grants_do_not_cross_network_partitions`, `profiles_keep_local_storage_and_persistent_cookies_and_private_writes_nothing`.

The `CookieJar` and `PreflightCache` live inside one `Fetcher`, are lost
when it is dropped and are not keyed by Fetch's network partition key.
Profiles, private mode and storage keys are NET-06.
**Pay:** move both behind the profile store with partition keys (top-level
site plus frame site), a versioned on-disk schema for persistent profiles
and an ephemeral one for private mode.
**Trigger:** NET-06 (`storage.profiles`, `storage.partition`).

### TD-161 — Fetcher is a blocking, cache-less prototype (Paid)

**Rescheduled 2026-11-20 (M6 completion):** the HTTP cache, referrer computation and fetch metadata are done. What remains is the event-driven controller, which belongs to the M7 network process (ISO-02), and the Lax-allowing-unsafe exception, a Chromium compatibility allowance Rustic may never adopt (decide with M7 cookie policy review).

`Fetcher::fetch` waits for each hop's head on the calling thread, so it is
for tests, the CLI and a future network task, not the page's event loop.
It has no HTTP cache (NET-04), takes request bodies
as complete byte vectors, and does not implement Chromium's two-minute
"Lax-allowing-unsafe" exception for top-level cross-site `POST`s.
**Pay:** an event-driven fetch controller over `Exchange` events on the
networking task source, the partitioned HTTP cache, referrer computation
and fetch metadata (both done 2026-11-20: the `referrer` module and the
`Sec-Fetch-*` and `Accept-Language` headers), streaming request bodies with the WEB bindings for
`fetch()`.
**Trigger:** the engine loading subresources over HTTP (NET-02 wiring) or
the script-facing `fetch()` binding, whichever comes first.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): `Fetcher::begin` now runs a fetch as a non-blocking state machine (`PendingFetch`: redirects, CORS preflight, cache revalidation and the client policy before every hop), and `FetchController` runs many of them over one `Fetcher`, each `Exchange` waking a shared `Signal` instead of blocking per hop; `fetch_checked` is a thin loop over it. The CLI network session uses it to load a document's subresources side by side, in process and through the broker (TD-165). The cookie jar adopts Chromium's Lax-allowing-unsafe window (120 seconds for a cookie without `SameSite`, not for cookies restored from a profile; ADR 0054 updated). Streaming request bodies need `ReadableStream`, which does not exist yet, so they moved to TD-169 with the rest of Streams. Tests: `the_controller_runs_fetches_concurrently`, `the_controller_checks_every_hop_and_cancels` (rustic-net `tests/fetch.rs`), `same_site_rules` (cookie tests), `subresources_load_concurrently` (rustic-cli `tests/network.rs`).

### TD-162 — gzip and deflate bodies are not streamed

`rustic-codecs`' DEFLATE decoder is one-shot, so `ContentDecoder` keeps a
`gzip` or `deflate` body until it ends and then inflates it (brotli
decodes as it arrives). Memory is bounded by `max_body_bytes` for the
input and the output, but a large compressed page reaches the parser only
after its last byte, and a slow server delays first paint.
**Pay:** a resumable inflate state machine in `rustic-codecs` (bit
reader, block and Huffman state carried across `feed` calls, a 32 KiB
window), used for PNG too, with split-invariance tests like the HTTP
parser's.
**Trigger:** M12 page-load measurements, or progressive rendering of large
documents.

### TD-163 — CSP violation reports are not delivered

`csp::Policy` checks return violations, and report-only policies only
produce them, but nothing sends them: no `report-uri` POST, no Reporting
API `report-to` endpoint, and no `securitypolicyviolation` event on the
document. Pages that rely on reports to tune their policy see nothing.
**Pay:** a reporting queue on the networking task source that POSTs
`application/csp-report` and Reporting API bodies, and the
`SecurityPolicyViolationEvent` binding.
**Trigger:** the Reporting API or `SecurityPolicyViolationEvent` work in
M9's web APIs.

### TD-164 — CSP does not cover inline styles or event handlers (Paid)

**Paid 2026-11-20:** `rustic_style::InlineStylePolicy` gates inline `<style>` elements (`SheetStatus::BlockedByPolicy`) and `style` attributes through `StyleLoader::with_inline_policy` and the cascade; the CLI supplies the document's CSP (`style-src-elem`/`-attr`, nonces, hashes). Inline event-handler attributes are not compiled at all yet (TD-153); when they are, their compilation must ask `CspList::check_inline(InlineKind::ScriptAttribute, …)`, which already decides them. Evidence: `csp_blocks_unnonced_inline_styles_and_style_attributes`.

The engine's `ScriptPolicy` hook covers parser-inserted inline scripts,
and hosts check script, stylesheet and image URLs and `eval` against the
policy, but the engine applies inline `<style>` elements, `style`
attributes and inline event-handler attributes without asking the
policy, although `csp::Policy` can decide all three.
**Pay:** policy hooks in the engine's stylesheet collection, the `style`
attribute path and event-handler compilation, with tests that a
`style-src 'nonce-…'` page drops the unnonced sheet.
**Trigger:** the M6 network policy gate (`network.policy`).

### TD-165 — CLI network loads are sequential and buffered (Paid)

`rustic-cli`'s `NetworkSession` reads a document's body completely before
the parser sees it, and fetches each stylesheet, script and image one at
a time, blocking, in discovery order. A slow subresource delays
everything after it, and a document cannot render progressively. Its
stylesheet check also requires `text/css` everywhere, while HTML lets a
quirks-mode document use a same-origin sheet of another type.
**Pay:** the event-driven fetch controller of TD-161 on the networking
task source, streaming the body into the parser as `Exchange` events
arrive and running subresource fetches concurrently; pass the document
mode to the stylesheet check.
**Trigger:** the M7 browser shell's network process, or a page-load
measurement over HTTP.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.

**Paid 2026-11-23** (M8 workstream 5): A navigated document's body now streams into the parser as it arrives: `navigate` keeps the body's `Exchange` in the session, `Session::next_document_chunk` reads each piece (through the broker with the new `DocumentChunk` IPC messages in split mode), and the loader feeds it, keeping the bytes for a reload after a late `<meta>` and waiting for 1024 bytes only when the encoding is not already certain. The HTTP cache no longer reads a storable body whole before returning it: the exchange keeps a copy as the consumer reads it and the fetcher stores it once complete (redirects are still read whole). Stylesheets, images and scripts load side by side through TD-161's controller (TD-134 orders them), and stylesheet requests carry the document's quirks mode so a same-origin sheet of any type applies there unless `nosniff`. Frame and popup documents are still read whole: TD-198. Tests: `documents_stream_into_the_parser_as_they_arrive`, `subresources_load_concurrently`, `quirks_mode_takes_same_origin_stylesheets_of_any_type` (rustic-cli `tests/network.rs`, both process modes), `storable_bodies_stream_and_are_stored_once_read_to_the_end` (rustic-net `tests/http_cache.rs`).

### TD-166 — Profile locks are not checked for a dead owner

`ProfileLock` creates `LOCK` exclusively and removes it when dropped. A
process that crashes leaves it, and the next run reports the profile as in
use. Checking the recorded process id is not portable, and the declared
MSRV (1.85) predates `File::try_lock`.
**Pay:** an OS file lock (`File::try_lock` once the MSRV allows it, or a
reviewed platform call) held for the profile's life, with the `LOCK` file
kept only as a hint.
**Trigger:** the M7 browser shell, which opens the user's real profile.
**Rescheduled 2026-11-22:** M7 closed with this entry open ([ADR 0067](adr/0067-mobile-devices-last.md)); it is now due in M8, before remote browsing is on by default or the browser shell ships, as the payoff schedule records.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M10 (beta) row: an OS file lock needs `File::try_lock` (Rust 1.89) or a reviewed platform call; the MSRV is still 1.85 and a shell that ends normally releases the lock (a8b81be). Due before the M10 beta opens real user profiles by default.

### TD-167 — No `storage` events

HTML §12.2.4 fires `storage` at every other `Window` whose document has
the same storage key when an area changes. Rustic's areas report each
change (`StorageChange`), but nothing dispatches it: the CLI has one
document, and there is no `StorageEvent` interface.
**Pay:** a per-profile broadcast of `StorageChange`s to the other live
documents of the key, queued on their DOM manipulation task source, and
the `StorageEvent` binding.
**Trigger:** a host with more than one live document (tabs or frames).

### TD-168 — The HTTP cache is not persisted

`rustic_net::http_cache::HttpCache` lives in the `Fetcher`, so every run
starts cold even with `--profile`, and it implements neither
`immutable`, `stale-while-revalidate` nor partial (range) entries.
Correctness does not depend on any of them; repeat-visit speed does.
**Pay:** a profile-backed store (body files plus an index record, checked
on load like the other profile files), the extensions, and eviction under
the profile quota.
**Trigger:** M12 repeat-visit page-load measurements.

### TD-169 — fetch() is synchronous underneath and body types are minimal

`fetch()` returns a promise, but the host runs the request to completion
before returning, so a slow response stalls every other task of the page;
it is cancelled only with the document. Request and response bodies are
held whole. The CLI uses the response URL as the API base URL.

**Partly paid 2026-11-23:** buffered bodies now accept unshared fixed-length
array buffers and views and `URLSearchParams`. `arrayBuffer()` and `bytes()`
return fresh byte storage and consume a supplied body once. `Blob` input
and `blob()` output use the same buffered path. Blob reads resolve through
the promise queue. The file reading task source and Blob streams await
Streams integration. `Headers` iterators read the current list on each step.
The static `Response.json()` factory uses intrinsic JSON serialization.

**Pay:** the event-driven fetch controller of TD-161 queuing response and
body tasks on the networking task source, Streams and FormData body
support (including streaming request bodies, moved here from TD-161 on
2026-11-23), `AbortSignal`, upload progress and `<base>`.
**Trigger:** the M9 web platform APIs work (Streams, Blob, FormData).

### TD-170 — IPC copies bodies and serializes requests

`rustic-ipc` sends documents, images, scripts and PNG output through the
renderer's standard streams, so large bodies cross the process boundary
as copies, and a renderer has one request outstanding at a time.
**Pay:** validated shared-memory regions for bulk data (behind a reviewed
platform crate, since the workspace forbids unsafe code) and request
multiplexing with the event-driven fetch controller (TD-161).
**Trigger:** M12 page-load or frame-time profiles showing IPC copies, or
the GPU process (frames need shared memory).

### TD-171 — The renderer is split but not yet confined

Document commands run their content in a child process that receives no
authority from the broker, but the operating system still lets that
process open files and sockets itself: the separation holds against bugs
that misuse the protocol, not against code execution in the renderer. A
renderer that stops answering blocks the broker indefinitely (script and
parser budgets bound the work, but not a hang in native code), and
`reftest` renders its test and references in one process.
**Pay:** ISO-03/ISO-04 OS sandboxes (ADR per platform), and `reftest`
over renderers. **Partly paid 2026-11-20:** the broker's watchdog stops a
renderer after `--renderer-timeout`, and Linux has the opt-in bubblewrap
sandbox of ADR 0058. **Partly paid 2026-11-21:** the renderer is confined
by default on Linux (ADR 0058 with a seccomp allowlist) and Windows
(ADR 0060); macOS remains (TD-174), and `reftest`.
**Partly paid 2026-11-22:** macOS confines by default too (ADR 0063,
TD-174 paid); `reftest` remains.
**Trigger:** before any remote browsing (the M7 gate).
**Paid 2026-11-22:** `reftest` runs its test in a renderer like the other
document commands. The renderer resolves each reference lexically and
asks the broker for it, which serves only files inside the test's
directory after symbolic links resolve (the single-process rule); the
verdict lines match a single-process run, failures included
(`tests/isolation.rs`).

### TD-172 — The Linux sandbox filters no system calls

ADR 0058 confines the renderer with namespaces, an empty network, no
capabilities and a filesystem of read-only `/usr`, but every system call
stays available, so a kernel vulnerability reachable from them is not
mitigated, and system libraries and their versions are readable.
**Pay:** a seccomp-bpf allowlist for the renderer (passed to
`bwrap --seccomp`, which needs descriptor passing: a reviewed crate or a
reviewed unsafe module), and binding only the libraries the executable
links (or a static build).
**Trigger:** before the sandbox becomes the default or remote browsing is
enabled on Linux.
**Paid 2026-11-21:** `rustic_sandbox::seccomp` builds a measured
allowlist (unknown calls `ENOSYS`, no sockets or namespaces, other ABIs
killed) that `/bin/sh` hands to `bwrap --seccomp` as descriptor 3, so no
unsafe code or crate is needed; the sandbox is on by default (ADR 0058).
The readable `/usr` moved to TD-175.

### TD-173 — No trusted UI for permissions; downloads are not quarantined

ISO-05 needs prompts, the address display and clipboard access drawn by
the browser, outside anything a page can paint over. The development CLI
has no UI, so it grants no prompt-gated permission at all, which is the
safe side but leaves the prompt flow untested. Downloads are read whole
before they are written, and saved files get no `com.apple.quarantine`
attribute or Windows Mark of the Web (both need platform calls).
**Pay:** the M8 shell's permission prompts bound to transient activation,
streamed downloads through the broker, and quarantine metadata per
platform.
**Trigger:** the M8 desktop shell.
**Partly paid 2026-11-23:** the strip is the whole browser UI now (ADR
0074): tabs, toolbar, find bar, prompt and download notice from one layout
shared by drawing and hit testing; a prompt is dismissed when its tab is
left or navigated; downloads show a notice and keep the previous page.
**Partly paid 2026-11-22:** `rustic view` draws a trusted strip above the
page (`rustic-cli/src/chrome.rs`, `rustic_shell::present`): the address
the broker committed (after redirects; ASCII hosts, no credentials or
fragment, elided from the host's start so the registrable domain stays
visible, "Secure"/"Not secure"/"File"), and a permission prompt with
Block and Allow. The broker renders the strip itself, scripts off, from
escaped printable ASCII; page pixels never reach its rows and clicks
there never reach the page. `chrome::Permissions` holds the rules:
secure, non-opaque origins only, transient activation required, one
prompt at a time, buttons ignore clicks for 500 ms, answers kept per
origin for the session, three dismissals embargo an origin, a new
document closes the old prompt; clipboard writes need activation and
focus, reads need focus and the permission. Remaining: page APIs
(Notifications, Permissions, Geolocation, async Clipboard) reaching these
rules once renderers are long-lived (TD-178), streamed downloads, and
quarantine metadata.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 language and web APIs row: the page APIs that would ask the permission prompt (Notifications, Geolocation, async Clipboard) are M9 web APIs; download quarantine ships with M10 packaging.

### TD-174 — macOS has no renderer sandbox

**Paid 2026-11-22:** the first native macOS 15 runs (x64 and ARM64)
found that a renderer started under `sandbox-exec` aborts in dyld; the
renderer and network process now apply their profiles to themselves with
`sandbox_init` before reading broker input (`rustic_sandbox::enter`), the
escape probes pass on both runners, the sandbox is on by default
(`SANDBOX_AVAILABLE`), and ADR 0063 is Accepted. Narrower `sysctl` reads
and pre-`main` initializers are TD-183.

**Partly paid 2026-11-21:** ADR 0063 (Proposed) adds a deny-by-default
Seatbelt profile (`rustic_sandbox::seatbelt`) started through
`sandbox_init` (applied by the renderer itself) when `--sandbox` is given; it is unit-tested and
type-checked for both macOS targets on Linux, and the escape-probe test
has a macOS branch. What remains: run that test on native macOS x64 and
ARM64, adjust the profile if a system path is missing, then make it the
default (`SANDBOX_AVAILABLE`) and accept the ADR.

Linux and Windows confine the renderer by default (ADR 0058, ADR 0060);
on macOS the renderer is a separate process with brokered authority but
the operating system still lets it open files and sockets, and
`--sandbox` is an error. A seatbelt profile (`sandbox_init` or
`sandbox-exec`) needs an ADR and must be verified on native macOS x64
and ARM64, which no runner provides (TD-12).
**Pay:** a deny-by-default seatbelt profile with the same probe tests,
on by default.
**Trigger:** a native macOS runner, and before remote browsing on macOS.

### TD-184 — GPU process pixels travel over a pipe

ADR 0068 moved frame compositing into a `rustic __gpu` child. Each
composited frame sends the page and its frames' pixels over the child's
standard input and returns the composited page, two full copies per
frame, because Rustic has no shared memory between processes yet
(TD-170). Compositions larger than half the 256 MiB frame limit stay in
the broker. **Pay:** shared-memory surfaces (or the platform's GPU
texture sharing) under the backend record. **Trigger:** the hardware
backend (`compositor.backend`), or a profile showing the copies. The
backend exists since ADR 0071 (2026-11-23); these copies are now what
keeps it from being faster than the CPU path end to end.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M12 (performance leadership) row: shared-memory surfaces are the frame-time and energy work of M12; the GPU process is correct without them.

### TD-185 — The window is not presented by the GPU process

`rustic view` composites its frames through the GPU process like
`render`, but the window's surface (softbuffer, ADR 0065) is still
written by the broker. A lost window surface is recreated in place (at
most `MAX_SURFACE_RECREATIONS` times in a row, 2026-11-23,
`compositor.recovery`). Presenting from the GPU process needs the window
handle (HWND, X11 window, `CAMetalLayer` through a Mach port, a Wayland
subsurface) passed to another process and wgpu's `create_surface_unsafe`,
an `unsafe` boundary no reviewed crate covers yet. **Pay:** a reviewed
cross-process presentation record per platform, then hand the surface to
the GPU process. **Trigger:** a frame-time profile showing the broker's
copy, or GPU rasterization of page content.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M12 (performance leadership) row: presenting from the GPU process needs an `unsafe` window-handle boundary (ADR 0071) and pays off as latency, which M12 measures.

### TD-186 — Windows and macOS GPU processes have no driver access

ADR 0071 gives the GPU process a hardware compositor through wgpu. On
Linux its bubblewrap profile binds the DRM device nodes and the sysfs
paths drivers probe. On Windows it still runs in the `Rustic.Renderer`
AppContainer and on macOS under the Seatbelt renderer profile, neither of
which grants Direct3D or Metal driver services (IOKit user clients, the
graphics kernel), so the device may fail to open and confined runs
composite on the CPU; `--no-sandbox` reaches the hardware. **Pay:** a
GPU AppContainer (Chromium's GPU sandbox capabilities) and a Seatbelt GPU
profile allowing only the IOKit classes Metal needs, each with the escape
probes of `tests/gpu.rs`, verified on native x64 and ARM64 runners with a
GPU. **Trigger:** UI-02 windows on Windows and macOS, or a runner with a
GPU on either.

**Paid 2026-11-23.** Windows: the GPU process runs in its own `Rustic.Gpu`
LPAC AppContainer with only `registryRead` and `lpacPnpNotifications`
(Chromium's GPU sandbox grants the same pair for driver discovery). macOS:
`seatbelt::gpu_profile` adds the graphics IOKit user clients by class,
IOKit property reads, the Metal compiler and GPU memory services, driver
bundle reads and metadata reads of the binary's directory (IOSurface reads
the main bundle's GPU policies; without them it crashed). wgpu errors now
carry their description instead of only "Validation Error". CI requires
the confined GPU process to reach the same adapter as an unconfined one on
Linux (lavapipe), Windows x64 and ARM64 (WARP) and macOS x64 (Apple
Paravirtual Metal). Vendor drivers on physical GPUs ride with the hardware
runner that `compositor.parity` records as an external dependency.

### TD-187 — Some code has no debugger steps

The debugger (ADR 0072) instruments code compiled while a hook is attached.
`eval`, `new Function`, class field initializers, generated constructor
bodies and module scripts are compiled through paths that do not register
a debugger script, so they carry no `Step` ops; code compiled before
`debug.enable` has none either. **Pay:** register those sources and
compile their steps, and recompile (or lazily instrument) functions when a
hook attaches late. **Trigger:** a DevTools user stepping into such code,
or M9 language work on those paths.

### TD-188 — Inline script line mapping assumes UTF-8

Inline scripts are reported in document lines by searching the normalized
document bytes for the script text. For documents in legacy encodings the
search fails and lines stay script-relative. **Pay:** carry the source
position of each script element from the tokenizer. **Trigger:** a
non-UTF-8 page in a debugging report, or parser source positions (TD-15).

### TD-189 — The network log is partial

`network.requests` wraps the broker network session, so it sees requests of
network documents only; local-file loads and the requests of frames and
popups (other renderers) are not logged, and entries keep no timing or
bodies. **Pay:** a per-page log owned by the broker across frames, with
timing and opt-in bodies under the `network` grant. **Trigger:** frames in
automation tests, or performance tooling (M12).

### TD-190 — Automation protocol 1 is minimal

Version 1 has no matched-rule or cascade view (`css.computed` gives the
computed values only), only click input, no DOM editing, and no event
timeline (`profile.statements` and `profile.metrics` give statement counts
and task, timer and settle-turn totals). **Pay:** a timeline of tasks, style, layout and paint with durations;
matched rules from the cascade with source locations, key and pointer
input through the same trusted-input path as clicks, and DOM edits under a
new grant. **Trigger:** UI-04 parity work, or a WPT test needing input
beyond clicks.

### TD-195 — Find in page stays within one text fragment

`render --find` and live frames search each laid-out text fragment (one
text box on one line) case-insensitively, so a phrase broken across lines,
elements or bidi runs is not found, and a match's left and width are
estimated from its share of the fragment's characters rather than from
glyph clusters.
**Pay:** search the document's text in tree order with a mapping to glyph
clusters across fragments (the editing and selection model gives both).
**Trigger:** M9 editing/selection, or a find bug report.

### TD-196 — Background tabs are not kept alive

The window keeps one long-lived page, the active tab's. Switching tabs
loads the other tab's page again (scripts restart; its scroll position is
restored), because a persistent profile is opened per thread and two live
pages would contend for its lock.
**Pay:** a profile shared by the broker's page threads (or a storage
process) and one live renderer per tab with background throttling.
**Trigger:** M10 embedding and beta, or tab-switch state loss reports.
**Paid 2026-11-23 (ADR 0074):** `view.rs` keeps a worker and live page
per tab (`MAX_LIVE_PAGES`, six; the least recently shown is discarded and
loads again when shown) and asks background pages for at most one frame
a second (`BACKGROUND_FRAME`), so their timers and scripts keep running.
Storage opens the profile once per process and shares it between page
threads (`storage.rs`). Test: `background_tabs_keep_their_live_page`
(a clicked page keeps its state across a tab switch).

### TD-197 — IME composition stops at the browser's own fields

The host enables the input method for the address and find fields, places
its candidate window at the field and applies preedit and commit there.
Pages receive key presses but no text or composition events, and CI drives
composition with synthetic events through the host's handler rather than
the platforms' input methods (IBus, TSF, macOS text input).
**Pay:** route text and composition to the focused page element
(a11y.editing) and add an input-method run per platform.
**Trigger:** a11y.editing, or the M10 beta.
**Partly paid 2026-11-23 (ADR 0074):** each frame reports the focused
editable field's box (`Painted.editing`, from the accessibility tree's
editable test); while it has focus the window enables the input method
there, preedit and commit become the page's composition (start, update,
end) and keys go to the page. Test:
`input_methods_compose_into_the_focused_page_field`. Remaining: an
input-method run per platform (IBus or Fcitx, TSF, macOS text input) in
CI, with the M10 beta.

### TD-198 — Frame and popup documents are not streamed

TD-165 streams a top-level network document into its parser. A frame's or
popup's document is fetched by the broker (`NetworkSession::open_frame`,
the popup navigation) and read whole before its renderer starts, so a slow
frame body delays its first parse and the broker holds the whole body.
**Pay:** keep the body's exchange in the frame's session and grant the
document as streamed, as `navigate` does, then read it through
`DocumentChunk` in the frame's renderer.
**Trigger:** M12 page-load work, or frames measured in `perf.harness`.

### TD-199 — Ignored advisories in the AT-SPI dependency graph

`accesskit_unix` 0.23 (ADR 0073) depends on atspi-common 0.13, which
pulls zbus-lockstep 0.5, zbus_xml 5 and quick-xml 0.38. Two quick-xml
advisories (quadratic duplicate-attribute check, unbounded namespace
allocation) apply to XML parsing; here that code runs only in
atspi-common's `#[cfg(test)]` signature checks, so no XML reaches it in
Rustic. zvariant, zvariant_derive and zbus_names are flagged unmaintained
with no vulnerability. `deny.toml` ignores the five IDs with these
reasons. **Pay:** move to an AccessKit release whose Unix adapter drops
zbus-lockstep and the unmaintained crates (with the winit upgrade it needs),
then remove the ignores. **Trigger:** such a release, a vulnerability in
the zvariant family, or any run-time XML parsing in this graph.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M10 (beta) row: no AccessKit release drops the AT-SPI graph yet; due with the AccessKit upgrade before the M10 beta.

### TD-183 — macOS profile breadth: sysctl reads and pre-main code

The Seatbelt profiles (ADR 0063) allow `sysctl-read` of every name,
which reveals system details such as the host name, because the first
native runs could not log which names dyld's libignition needs. And the
binary links AppKit (for `rustic view`), whose initializers run before
`main`, so before the renderer's profile applies; they read no page
input but may open Mach connections that stay usable.
**Pay:** narrow the `sysctl` list from logged reads on native runners,
and give renderers a binary or launch path without the window stack.
**Trigger:** M8 macOS packaging, or a finding against either.
**Paid 2026-11-23 (ADR 0074):** `rustic-cli`'s window stack is the
`window` feature; `crates/rustic-renderer` builds the same commands
without it, and on macOS the broker starts renderers and the network
process from that executable when it sits beside `rustic` and is not
older (`broker::child_exe`). CI builds it on its own, checks with
`otool -L` that it links no AppKit, and runs the isolation and live
tests with it (`RUSTIC_TEST_WINDOWLESS_CHILDREN`). The profiles now
hide `kern.hostname`, `kern.uuid`, the other machine identifiers and the
`kern.proc.`, `kern.procargs` and `net.` families instead of allowing
every read; an allowlist stays impractical because the system
libraries' reads are undocumented. Escape probes `read-host-name` and
`list-processes` are denied confined on the native macOS runners.

### TD-175 — The Linux sandbox sees all of `/usr` and needs bubblewrap

The renderer's filesystem is read-only `/usr` plus its executable, so it
can read every installed library and data file (versions, not user
data), and the sandbox depends on a `bwrap` binary on `PATH` that a user
could replace.
**Pay:** bind only the libraries the executable links (or ship a static
renderer), and launch a packaged, path-pinned bubblewrap or Rustic's
own namespace setup.
**Trigger:** M8 packaging, or a library-disclosure finding.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M10 (beta) row: packaging (installers that ship bubblewrap or a bundled launcher and narrower binds) is M10 beta work.

### TD-176 — The network process is not OS-confined

Split runs move sockets, TLS, HTTP framing and content decoding into a
`rustic __network` child (ADR 0061), and the broker treats its replies as
hostile. The child itself is started like an unconfined renderer: it can
read the user's files and reach any loopback service, so code execution
in it (a TLS or decoder bug) is contained only by what the broker refuses
to accept from it.
**Pay:** confine it on each platform with what it needs and nothing
else: on Linux bubblewrap without `--unshare-net`, read-only trust-store
paths and a seccomp allowlist that adds `socket`/`connect`; on Windows an
AppContainer with the client network capability; macOS with TD-174.
Same probe tests as the renderer, minus networking.
**Trigger:** before remote browsing (the M7 gate), or when DNS moves into
the network process (TD-157).
**Update 2026-11-21 (Linux paid):** under the sandbox (the default on
Linux) the network process runs in bubblewrap with every namespace but
the network unshared, read-only `/usr`, its executable and the trust
store (`/etc/ssl`, `/etc/pki`, `/etc/ca-certificates`), and
`rustic_sandbox::seccomp::network_filter`: the renderer allowlist plus
client socket calls, `socket` only for `AF_INET`/`AF_INET6`
(`the_network_filter_adds_internet_client_sockets_only`,
`the_sandboxed_network_process_reaches_only_internet_sockets`, which
reads a file, lists a directory, starts a shell and connects to an
abstract Unix socket: allowed unconfined, denied confined). Windows
(an AppContainer with only the client network capability, in the
`unsafe` launcher of ADR 0060) and macOS (with TD-174) remain; both need
native runs.
**Update 2026-11-22 (macOS written):** `seatbelt::network_profile`
confines the macOS network process under `--sandbox` (outbound IP
connections and the certificate trust daemons only), checked by
`the_seatbelt_network_process_reaches_only_internet_sockets` on the
native macOS runners; it becomes the default with the renderer profile
(TD-174). **Windows finding:** an AppContainer (LPAC or not) cannot
connect to loopback without a machine-wide exemption that needs an
administrator (`CheckNetIsolation LoopbackExempt`), and the CLI grants
loopback addresses only (NET-02, no DNS), so a confined Windows network
process could not reach anything Rustic loads today. Windows is
therefore paid together with remote addresses (TD-157): then an LPAC
with `internetClient` (and `privateNetworkClientServer` for LAN
grants) serves remote exchanges, while loopback grants need a design
of their own.
**Update 2026-11-22 (macOS paid):** the Seatbelt network profile is on
by default with the renderer sandbox and passes
`the_seatbelt_network_process_reaches_only_internet_sockets` (explicit
and default runs) on native macOS x64 and ARM64. Windows remains.
**Paid 2026-11-22 (Windows, ADR 0066):** with remote addresses (TD-157)
the Windows network process starts in its own LPAC, `Rustic.Network`,
with `internetClient`, `lpacCryptoServices` and `registryRead` only, in
a kill-on-close job of one process, whenever no loopback fixture origin
is granted (no AppContainer can reach loopback; such runs serve only the
user's own servers and stay unconfined).
`the_windows_network_process_is_confined_for_remote_origins` checks the
escape probes (file, directory, `cmd.exe`, loopback) unconfined and
confined; it runs on the native Windows runners in the next CI run.

### TD-177 — Frame compositing is linear and untransformed

[ADR 0062](adr/0062-out-of-process-frames.md) composites a frame without the
embedder seeing it: the embedder paints the frame black, then white, and the
difference is the frame's coverage at each pixel. Source-over compositing,
clips and opacity are linear in premultiplied color, so the result is exact
there. Blend modes other than `normal` and color-matrix filters are not
linear, so frames under them composite approximately. A frame under a CSS
transform would need its pixels resampled along the transform, which the
coverage rectangle cannot express, so it paints white with a diagnostic.
Frame documents paint an opaque white canvas, where browsers leave a frame
without a background transparent.
**Pay:** with the M8 compositor (or GPU process), composite frames as
surfaces with their transforms, blend modes and filters, and give frame
canvases HTML's transparent default; keep the coverage path as the CPU
reference.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 CSS, layout and paint row: blend modes, filters and transforms across frame boundaries are paint work for the M9 CSS/paint row and its WPT reftests.

### TD-178 — Frames have no script relationships and need a split run

Every frame document renders in a renderer of its own (ADR 0062), the
strict end of ADR 0057's process keys, so same-origin frames cannot script
each other: there is no `contentWindow`, `contentDocument`, `window.parent`,
`window.frames` or `postMessage`, no frame `load` event, and frames render
once at the end of the embedder's script turns. Popups (ADR 0057 §6)
have the same limit: `window.open()` returns `null`, the popup has no
`window.opener` even when COOP keeps the group, the CLI does not show the
popup's document, and `_self`, `_parent` and `_top` targets open nothing.
Only `render` in a split
run loads frames; `--single-process`, `reftest`
and `render --frames` paint them white, and the dump commands and `hit-test` do not descend into
them.
**Pay:** with the M8 browser shell's long-lived renderers, put same-site
frames of one agent cluster in one renderer (ADR 0057 §1), add WindowProxy
retargeting and cross-origin property access (TD-130) and cross-renderer
`postMessage` through the broker, give popups and openers WindowProxies
across renderers and show popups in the shell, and load frames in
single-process runs for `reftest`.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M9 language and web APIs row: frame scripting across renderers (`contentWindow`, `postMessage`, `window.open`) is HTML browsing-context work in M9, together with TD-130.

### TD-179 — HTML presentational hints are partly mapped

HTML §15 maps attributes such as `width` and `height` on `img`, `iframe`,
`video`, `embed` and `object` (and `align`, `bgcolor`, `border`, …) to CSS
presentational hints. Found while adding frames (ADR 0062). Partly paid
2026-11-27 with `layout.tables`: the cascade has a presentational-hint
level below author rules ([CSS guide](CSS.md#presentational-hints)), and
dimensions and the table attributes are mapped. Not mapped yet: `body`
`text`/`link`/`bgcolor`/`background`, `font` `color`/`size`/`face`,
table `bordercolor`, `rules` and `frame`, `hspace`/`vspace`, `hr` and list
attributes, `dir`-independent `align` on other elements, and the legacy
color parser's lenient forms (only names and 3- or 6-digit hex are taken).
**Pay:** map the rest with the WPT `html/rendering` tests.

### TD-180 — Script scrolling is the position subset

ADR 0064 step 4 gives scripts the scroll positions: `scrollX`/`scrollY`,
`scroll()`/`scrollTo()`/`scrollBy()` on Window and Element,
`scrollTop`/`scrollLeft` and `scroll` events. Missing: the CSSOM View size
members (`scrollWidth`, `scrollHeight`, `clientWidth`, `clientHeight`,
`clientTop`, `clientLeft`), `scrollIntoView`, smooth `behavior` (every
scroll is instant), `scrollend`, scroll snapping and scroll anchoring, and
`getBoundingClientRect` moves sticky boxes with their scroll container
instead of re-solving their offset at the script's position (painting
does re-solve them).
**Pay:** the size members from the same geometry, `scrollIntoView` over
the scroll chain, then smooth scrolling and `scrollend` once the shell
animates frames (M8), with the WPT `css/cssom-view` scrolling tests.

### TD-181 — Scene layering limits

Scenes (ADR 0064 §3) cache the page in tiled layers: page segments, the
contents of each top-level scroll container, and an overlay of fixed and
viewport-sticky boxes. A scroll container nested in another stays in the
outer one's layer, and a container whose contents sit inside a group
(opacity, filter, blend, clip-path, transform) or carry rounded clips
paints into its page segment: scrolling those moves their fills, which
damages the tiles they cover. The overlay is rasterized in full every
frame. Pages with
iframes render in one pass (their frame layers are composited by the
broker, ADR 0062), `dump-paint` and `hit-test` use the unsnapped
position, and a page whose overlay boxes are not painted last (a later
positioned box above a fixed one) is painted in one pass.
**Pay:** nested scroller layers, layers inside groups (the group becomes
the layer) and rounded scrollports with the M8 GPU compositor
(`compositor.parity`), damage-based overlay repaints, frame layers inside
scenes, and the same snapping for dumps and hit testing.
**Rescheduled 2026-11-23 (M8 completion):** moved to the M12 (performance leadership) row: finer scene layering is compositor performance, measured in M12.

### TD-182 — The first window re-renders every frame

`rustic view` (ADR 0065) keeps the process split by running each frame as
a `render` in a fresh sandboxed renderer at the window's size and scroll
position, so every frame re-parses and re-lays out the page and scripts run
from scratch each time. Input other than scrolling (clicks, keys, text) is
not delivered to the page, there is no address bar, tab or prompt UI, and
the window is verified on Linux (Wayland and X11) only.
**Partly paid 2026-11-23:** [ADR 0069](adr/0069-long-lived-renderer.md)
keeps one renderer per page that receives clicks, keys and frame requests
over IPC and answers with scene frames; scripts and timers run between
frames, user scrolls go through the page, and failed renderers are
replaced (`broker::live`, `rustic drive`, tests in
`crates/rustic-cli/tests/live.rs`). **Pay the rest:** `rustic view`
renders through `broker::live` in the UI-01 shell, text/IME input reaches
the page, then native Windows/macOS runs.
**Paid 2026-11-23:** `rustic view` renders through `broker::live` on a page
thread (`crates/rustic-cli/src/view.rs`): clicks and keys the browser does
not use go to the page, frames come from the long-lived renderer, and the
UI-01 strip (tabs, history, address entry, reload/stop, find, zoom,
download notice) drives it ([ADR 0074](adr/0074-browser-shell-and-native-hosts.md)).
Scripted window tests (`crates/rustic-cli/tests/view.rs`) run on native
Windows and macOS (x64, ARM64) and on Linux under Xvfb and headless Weston.
IME composition into pages is TD-197.

### TD-200 — WebAssembly compilation runs on the agent

[ADR 0080](adr/0080-window-wasm-module-compilation.md) first compiled
admitted response bodies in a promise job. [ADR 0094](adr/0094-wasm-progressive-response-decoding.md)
adds cancellable, incremental response decoding in both renderer modes.
The promise reaction still performs pulls and compilation on the agent.

**Pay the rest:** move response reading and compilation off the agent and
settle the promise from the networking task source. Keep cancellation,
MIME/CSP policy and result ownership checked at that boundary.

### TD-201 — Transform interpolation edge cases

Transform lists that need matrix decomposition and contain percentage
translations interpolate discretely, because the percentages need the box
size that styles do not have. Decomposition uses the platform's `f64`
trigonometry, which can differ in the last bits between platforms, so tests
compare with tolerances.
**Pay:** carry percentage translations through decomposition as a deferred
`calc()` resolved by layout, and pin a portable trigonometry if a reftest
needs bit-exact frames (M9 CSS/layout row).

### TD-202 — Animations cannot target pseudo-elements

CSS transitions and animations run on elements only: `::before`, `::after`
and `::marker` styles do not start them, and `KeyframeEffect` rejects a
`pseudoElement`. Events carry an empty `pseudoElement`.
**Pay:** give generated-content boxes animation state keyed by (element,
pseudo) when the M9 generated-content work lands.

### TD-203 — Web Animations API limits

`commitStyles()` throws `NotSupportedError` (no inline style editing from
the binding yet). Every `DocumentTimeline` shares the document clock: a
non-zero `originTime` only shifts its own `currentTime`. Finished script
animations that no longer affect anything are dropped oldest first beyond
256 (`KEPT_FINISHED`), and a detached `KeyframeEffect` keeps its native
effect until the page goes away.
**Pay:** implement `commitStyles` with the inline-style CSSOM (TD-204),
timelines with their own origin, and release detached effects from a
collector hook (M9 language and web APIs row).

### TD-204 — CSSOM is read-only and not canonical

`document.styleSheets` and the rule interfaces are snapshots: no
`insertRule`, `deleteRule`, `replace`, constructable sheets or
`style` setters, and linked or imported sheets expose no rules. Text
fields are the source text with whitespace collapsed, not the canonical
CSSOM serialization.
**Pay:** a mutable rule tree shared with the cascade, with canonical
serialization from the typed values (M9 CSS row).

### TD-205 — Anchor positioning subset

`anchor()` and `anchor-size()` are accepted only as a whole inset or size
value (not inside `calc()` or margins). Anchors must be block-level boxes
laid out before the positioned box; scroll offsets between anchor and
containing block are ignored. Logical sides and `position-area` keywords
resolve as `horizontal-tb`; `position-try`, `position-visibility`,
`anchor-scope`, `anchor-center` and implicit (popover) anchors are not
implemented.
**Pay:** with writing modes and the M11 parity work on anchor positioning
WPT.

### TD-206 — Scroll-driven animation subset

Timelines are measured from unscrolled client rectangles with the scroll
container's border box as the scrollport; logical axes resolve as
`horizontal-tb`. `timeline-scope` is not supported, named timelines
resolve only from the element and its ancestors, and timelines update once
per rendering opportunity from the previous layout. Script
`ScrollTimeline` and `ViewTimeline` report `currentTime` as a percentage
number rather than a `CSSUnitValue`, `ViewTimeline.source`,
`startOffset` and `endOffset` are missing, and script animations on
progress timelines keep time-based timing.
**Pay:** with CSS Typed OM, writing modes and scroll-driven animation WPT in
the M11 parity work.

### TD-207 — View transitions are not painted

`startViewTransition` runs the full promise and capture sequence, but no
snapshots are taken and the `::view-transition` pseudo-element tree is not
built or painted, so the new state replaces the old without a visual
animation; `finished` waits for the default 0.25 s group duration.
Transition types, `:active-view-transition` and cross-document transitions
are not implemented.
**Pay:** render captured snapshots as compositor layers and build the
pseudo-element tree once the compositor has per-element layers (M11).

### TD-208 — Shared WebAssembly table and reference ownership (Paid)

[ADR 0089](adr/0089-live-wasm-function-tables.md) first exposed module-defined
function tables in the allocating instance's index space.
[ADR 0090](adr/0090-live-wasm-external-tables.md) added tracing for guest state
and suspended frames. [ADR 0091](adr/0091-wasm-external-table-imports.md) made
external-reference tables shareable. [ADR 0092](adr/0092-wasm-function-table-imports.md)
adds function-table imports and traces each function's defining instance
through shared state and suspended invocations.

Cross-instance indirect calls verify the callee's signature.
[ADR 0093](adr/0093-wasm-reference-interop.md) extends the boundary to nullable
reference-valued calls and globals.

### TD-209 — Grid layout subset

`layout.grid` implements placement, the track sizing algorithm, box
alignment including baseline self-alignment in the block axis, grid areas
as containing blocks for positioned descendants, and grid container
baselines, and subgrids (2026-11-27), for `horizontal-tb` grids. Not
implemented: masonry (`grid-lanes`, which are not parsed), vertical writing
modes (so `justify-self: baseline` falls back to `start`, and subgrids in
orthogonal writing modes are not handled), baseline sharing between a
subgrid's items and its parent's, and baseline groups across items spanning
several rows beyond their start or end row. Each row subgrid is laid out
twice per layout of its parent (measured, then at the parent's row sizes),
so deeply nested row subgrids multiply work within the flow budget. Auto repetitions
do not count against percentage `max-width` while the container's own
intrinsic size is computed, and implicit lines named on both sides of the
explicit grid are counted from the explicit grid only. Grid properties
interpolate discretely. Logical properties (`inline-size`, `margin-block`,
...), `aspect-ratio`, `width: stretch`, generated content and `overflow:
clip` next to `hidden` affect a few WPT pages through other subsystems.
**Pay:** the vertical pages with writing modes (`text.writing`), then
subgrid baselines.

### TD-210 — css-grid WPT comparison failures

`tests/wpt_grid.rs` lays out the 168 static `checkLayout` pages that
`tools/import_wpt_grid_tests.py` extracts from the pinned css-grid directory
and compares every element's expectations. 79 pass. Each of the other 89 is
listed in `fixtures/wpt-grid/expected_failures.txt` with its reason: 41
vertical writing modes, 20 masonry, 9 subgrid baselines (most with Ahem
text), 7 logical properties, and 12 single causes (Ahem `::first-letter`/`::first-line` text, form-control
items, generated content, `aspect-ratio`, unread expectation attributes and
the TD-209 edge cases). Rustic's bundled test fonts have no Ahem. The list
is a ratchet: a listed page that passes fails the test.
**Pay:** shrink the list with TD-209; bundle Ahem in the test fonts.

### TD-211 — Table layout subset
`layout.tables` lays out tables with anonymous table boxes (CSS Tables 3
§3.9.1) when fonts are available; the block-only reference without fonts
still refuses misparented table parts. Only the separated borders model
exists: `border-collapse: collapse` drops the spacing and keeps every cell
border. Column and column group boxes give widths but get no geometry (column
backgrounds are not painted), `min-width`/`max-width` on columns are
ignored, `visibility: collapse` and `empty-cells: hide` change nothing, and table parts are not relatively positioned. Anonymous
table boxes inherit from the box whose children they wrap rather than from
each other, and an inline table's baseline is its first line in preorder
(a top caption's, if any). Extra height of spanning cells and of a taller
table is shared by rows in proportion to their heights.
`tests/wpt_tables.rs` compares the pinned css-tables `checkLayout` pages:
18 of 35 pass; `fixtures/wpt-tables/expected_failures.txt` gives each
other page's reason (eight need list items, which Rustic does not lay out;
three tentative width-merging rules; others collapsing borders, vertical
writing modes, Ahem text, `height: stretch` and percentage heights inside
cells).
**Pay:** the remaining `layout.tables` step (collapsing borders, columns,
percentages, `empty-cells`, `visibility: collapse`), then shrink the list.
