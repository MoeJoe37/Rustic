# Technical debt ledger

This ledger records known shortcuts in committed code, why each exists, and
when it must be paid. It is reviewed against everything merged to `main`; the
first full review covered `main` at `b9595a5` (2026-11-08).

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

## Ledger

Status values: **Open** (accepted, has a payoff trigger), **Fixing** (in
progress), **Paid** (kept for history, with the change that paid it).

| ID | Kind | Area | Summary | Status |
| --- | --- | --- | --- | --- |
| TD-01 | Bad | HTML parser | Diagnostic count limits made error-heavy pages fail | Paid |
| TD-02 | Deliberate | DOM / parser | Depth and node budgets abort the whole parse | Open, pay before real-page loading |
| TD-03 | Watch | DOM | Every insert walks the moved subtree to measure height | Open, no measured blowup |
| TD-04 | Deliberate | All crates | Invariant `expect`/indexing in library code | Open, governed by rule 6 |
| TD-05 | Bad | HTML tokenizer | `char as u8` truncation in end-tag matching | Paid |
| TD-06 | Bad | CI | Fuzz workflow toolchain/action pins were unchecked | Paid |
| TD-07 | Deliberate | Process | Progress bot commits to `main`, so `feature` drifts | Open |
| TD-08 | Watch | Docs | Hard-coded corpus and test counts repeated across docs | Open, rule 8 |
| TD-09 | Deliberate | Workspace | `rustic-engine` holds only `Viewport` | Open, pay with the engine facade |
| TD-10 | Deliberate | HTML parser | Scripting-on and processing instructions pending (`selectedcontent` paid by DOC-06d) | Open, M1 queue |
| TD-11 | Deliberate | URL | `blob:` URLs return unsupported (`file:` paid by DOC-02b, IDNA paid by `url.idna`) | Open, M1 queue |
| TD-12 | Deliberate | CI / platforms | ARM and 32-bit now in CI, but mobile is check-only and runs are blocked | Open |
| TD-13 | Deliberate | Tokenizer / tree builder | Five spec state-machine functions exceed 100 lines | Open |
| TD-14 | Watch | CI | Branch pushes and pull requests both run the full CI | Open |
| TD-15 | Deliberate | HTML parser / URL / base | HTML and URL diagnostics not yet routed through `rustic-base` diagnostics | Open, pay with byte-offset mapping |
| TD-16 | Deliberate | DOM | Adoption reissues node IDs instead of keeping node identity | Open, pay with JS wrappers (DOC-09/WEB-02) |
| TD-17 | Deliberate | DOM / JS | Prototype `HostHeap` lives in `rustic-dom` and is bound to one document | Open, pay with JS-03 |
| TD-18 | Bad | DOM | Tree dumps grew quadratically with nested template contents | Paid |
| TD-19 | Watch | CI | Runners without rustup download its installer unpinned | Paid |
| TD-20 | Watch | Style | `:nth-*` selector matching recounts siblings on every test | Paid |
| TD-21 | Bad | Style | Nested `:is()`/`:where()`/`:not()` matching is exponential in nesting | Paid |
| TD-22 | Watch | CI | Emulated tests run serially to dodge a rare QEMU crash | Open |
| TD-23 | Deliberate | Style | Light children of a shadow host inherit from the host, not a slot | Open, pay with slot assignment |
| TD-24 | Watch | DOM | Custom element names approximate HTML's `PCENChar` ranges | Open, pay with the custom element registry |
| TD-25 | Deliberate | Style | Layers declared in a non-matching `@media` still take a place in the layer order | Open, pay before media-dependent layer tests |
| TD-26 | Deliberate | Style | `url()` values stay unresolved until stylesheets carry a base URL | Open |
| TD-27 | Deliberate | CSS | `white-space` is a longhand instead of a shorthand for the CSS Text 4 longhands | Open |
| TD-28 | Deliberate | Style / CLI | External stylesheets decode as UTF-8 only; no `@charset` or legacy encodings | Open, pay with the encoding layer |
| TD-29 | Deliberate | Style / URL | `:dir()` reads Bidi_Class from the IDNA tables in `rustic-url` | Open, pay with text shaping (M3) |
| TD-30 | Watch | Style | `:has()` with multi-compound arguments walks the subtree per anchor | Open |
| TD-31 | Deliberate | Style | Form owners ignore the parser's form element pointer | Open, pay with WEB form association |
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
| TD-42 | Deliberate | Text | TrueType-only font adapter without discovery, shaping or CFF/WOFF | Open, REN-03 |
| TD-43 | Deliberate | CSS | Media and container query `ex`/`ch`/`cap`/`ic` ignore loaded fonts | Open, REN-03 |
| TD-44 | Watch | Layout | Float placement and formatting-root retries are superlinear; metered by a fixed work budget | Guarded |
| TD-45 | Deliberate | Layout | Spec-permitted approximations in floats, static positions and scroll geometry | Open, REN-02b |
| TD-46 | Deliberate | Paint | Opacity groups allocate a full surface each instead of their bounds | Open |
| TD-47 | Deliberate | Text | Untailored UAX #14/#29 segmentation and bidi without L3 | Open, REN-03b |
| TD-48 | Deliberate | Style | Children of a `display:contents` child of a flex container are not blockified | Open, REN-08 |
| TD-49 | Deliberate | Layout | Flex and replaced sizing approximations pending text baselines and layout WPT | Open, REN-02c/REN-08 |
| TD-50 | Deliberate | Layout | First inline layout: no OpenType shaping, approximations (atomic inlines paid, TD-70) | Open, REN-03b |
| TD-51 | Deliberate | Codecs | PNG ignores color management; inflate decodes Huffman codes bit by bit | Open |
| TD-52 | Deliberate | Paint | First text painting: approximate decoration metrics, no hinting, system fonts or exact nonzero fill | Open, REN-04c |
| TD-53 | Deliberate | Paint | First image painting: block-level PNG only, fill-only fit, bilinear only, no alt text | Open, REN-05c |
| TD-54 | Deliberate | Paint | First outline painting: solid/double only, `auto` as solid, per-fragment inline outlines | Open, REN-04e |
| TD-55 | Deliberate | Paint | First `border-radius`: single-axis overflow ignores radii, per-fragment inline curves, unsnapped side joins | Open, REN-04f |
| TD-56 | Deliberate | Paint | First `clip-path`: no `url()`/SVG, no `evenodd`, no `xywh()`/`rect()`/`path()`/`shape()`, full-surface layers | Open, REN-04g |
| TD-57 | Deliberate | Paint | First transforms: `preserve-3d` flattened, layout unaware of transformed containing blocks, approximate projected curves | Open, REN-04h |
| TD-58 | Deliberate | Paint | First shadows, blending and filters: sRGB filters, full-surface groups, no ink overflow, approximate blur under transforms | Open, REN-04i |
| TD-59 | Deliberate | Paint | First tiled raster, damage and resize: each tile replays the whole list, a new filter margin repaints everything, list diff by common prefix and suffix, relayout from scratch | Open, REN-04j |
| TD-70 | Deliberate | Layout | Atomic inlines and block-in-inline splitting: baseline, relative-offset and paint-order approximations | Open, M3 layout |
| TD-71 | Deliberate | Layout | Sticky positioning: no overflow adjustment, no nested sticky composition, inline sticky boxes stay static | Open, M3 layout |
| TD-72 | Deliberate | Layout | css-flexbox WPT comparison: 53 of 91 static pages fail (writing modes, images, tables, 6 undiagnosed) | Open, REN-08 |
| TD-73 | Watch | Base | Layout keeps its own `CssPx`/`CssRect` beside the unit-typed `rustic-base` geometry | Guarded |

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

### TD-02 — Depth and node budgets abort the whole parse

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

### TD-03 — Every insert walks the moved subtree to measure height

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

### TD-09 — `rustic-engine` holds only `Viewport`

The crate is nearly a placeholder, which `AGENTS.md` discourages. It is kept as
the seam between hosts (CLI, desktop, mobile) and the engine. Do not grow it
with unrelated helpers. **Pay:** fill it with the engine facade (navigation,
document lifecycle) or fold `Viewport` into its first real consumer.
**Trigger:** the first host-facing engine API (M2).

### TD-10 — Scripting-on and processing instructions pending

Document and fragment parsing pass all 1,776 runnable html5lib tree records;
DOC-06d paid the `selectedcontent` part (the 4 former known failures). Sixteen
records are still skipped because they need scripting or processing-instruction
nodes, each with a reason in the corpus runner ([HTML_TREE.md](HTML_TREE.md)).

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

### TD-12 — Mobile is check-only; platform CI currently cannot run

`ci.yml` now tests natively on Linux, Windows and macOS for both x64 and
ARM64, and under emulation for armv7, i686 and riscv64 (32-bit pointers and a
third instruction set). Android and iOS still get only `cargo check`, with no
tests on a device or simulator. Since 2026-11-08 GitHub has refused to start
the Windows and macOS jobs because of an account billing limit, so those
platforms have no CI evidence at all. In the meantime merges are verified in a
local sandbox: the full Linux suite, MSRV 1.85, fuzz smoke, and `cargo check`
for aarch64 Linux, Android (aarch64 and armv7), Windows (x86_64-gnu), iOS and
i686. **Pay:** restore billing so the full matrix runs, then add Android/iOS
test runs once a host exists. **Trigger:** now for billing; mobile tests with
the mobile hosts.

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

### TD-16 — Adoption reissues node IDs instead of keeping node identity

`Document::adopt_node` copies a subtree into the target arena and destroys it in
the source, because a `NodeId` embeds its document and slot generation (ADR
0002, [ADR 0011](adr/0011-fragments-cloning-adoption.md)). The DOM's adopt keeps
the same node object. Today no script object holds an ID, and `Adopted::ids`
returns the complete old-to-new map, so nothing is lost or silently stale: old
IDs fail as `InvalidNode`. **Why deliberate:** moving slots between arenas would
break per-document budgets and generation checks. **Pay:** when JS wrappers
exist, the binding layer must retarget every wrapper through the map in the same
operation (or wrappers must hold a stable cross-document identity). **Trigger:**
the first script-visible node wrapper (WEB-02). The DOC-09 prototype heap prunes adopted IDs rather than retargeting them (TD-17).

### TD-17 — Prototype host heap lives in the DOM crate, one document only

`HostHeap` (ADR 0012) stands in for the JS heap so DOC-09 could test roots,
detached-tree retention and node/object cycles before a runtime exists. ADR 0002
says the runtime owns its heap without depending on the DOM crate, so this type
is not the final home. It is bound to one document: edges to another document's
nodes are rejected, and adoption prunes the source heap's entries (TD-16) rather
than retargeting them. Its edge registry is a flat vector sorted per collection,
and collection is stop-the-world. **Why deliberate:** it is the smallest thing
that tests the contract, it fails loudly instead of leaking, and the DOM side
(per-tree marking, sweep through `destroy_subtree`) is what carries forward.
**Pay:** JS-03 owns objects and roots; the DOM keeps tree-closure marking and
exposes it to a shared marking phase across documents; adoption retargets edges
through `Adopted::ids`. **Trigger:** JS-03, or a second document sharing a heap.

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
### TD-26 — `url()` values stay unresolved until stylesheets carry a base URL

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

### TD-28 — External stylesheets decode as UTF-8 only

`dump-style`'s local fetcher decodes every linked or imported stylesheet as
UTF-8: it strips a UTF-8 byte order mark and turns invalid sequences into
U+FFFD. CSS Syntax §3.2 also honors a UTF-16 BOM, an `@charset` rule, the
referring `<link charset>`/document encoding and legacy single-byte encodings.
A Latin-1 sheet therefore shows replacement characters instead of its text;
nothing is truncated, and the replaced text is still bounded by the byte
budget. **Pay:** route fetched bytes through the encoding layer that DOC-06's
encoding sniffing will provide, with the CSS fallback chain. **Trigger:** the
encoding layer landing, or a real page fixture that needs a non-UTF-8 sheet.
### TD-29 — `:dir()` reads Bidi_Class from the IDNA tables

HTML directionality (`dir=auto`, `bdi`, `:dir()`) needs each character's strong
bidirectional type. The only pinned Bidi_Class data is the table
`tools/generate_idna_data.py` generates for the IDNA Bidi Rule, so
`rustic-url` exposes `idna::strong_direction` and `rustic-style` depends on
`rustic-url` for it. The data is exact (Unicode 17.0.0), but it lives in the
wrong crate. **Pay:** move the Bidi_Class table to a shared Unicode data crate
(or `rustic-base`) and point both users at it. **Trigger:** the bidi algorithm
or text shaping in M3, or any second user of Unicode properties.

### TD-30 — Multi-compound `:has()` arguments walk the subtree per anchor

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

### TD-31 — Form owners ignore the parser's form element pointer

`:default`, `:checked` and `:indeterminate` need form owners (default buttons,
radio groups). Rustic resolves an owner from the `form` attribute or the
nearest form ancestor. The HTML parser also associates controls with the
form element pointer when they are not descendants (`<table><form><tr><td>
<input>`), but the DOM does not record that association yet, so such
controls get no owner. **Pay:** record parser-set form owners on the element
when form association (`HTMLFormElement.elements`) is implemented.
**Trigger:** WEB-series form APIs, or the first test that needs parser
association.

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

### TD-43 — Query font units ignore loaded fonts

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

### TD-44 — Superlinear float placement metered by a fixed budget

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

### TD-47 — Untailored segmentation and bidi

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

### TD-52 — First text painting slice

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

### TD-53 — First image painting slice

REN-05c paints decoded PNGs for block-level replaced `img` boxes only:
inline-level images wait for atomic inlines (TD-50). The content is always
stretched over the content box (`object-fit: fill`, no `object-position`),
always sampled bilinearly (no `image-rendering: pixelated`, no mipmaps, so
strong downscaling aliases), and image density (`srcset`, `x` descriptors,
EXIF orientation) is ignored. Broken images are zero-sized with no alt
text or broken-image icon. Decoded images live for one command; there is no
cache or sharing across documents. **Pay:** atomic inline images; parse and
apply `object-fit`/`object-position`/`image-rendering`; area-averaged
downscaling; alt text rendering; an image cache in the resource lifecycle
work (`paint.images` lifecycle). **Trigger:** the first real-page renders or
image WPT reftests.

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
tiles. Each tile replays the whole display list into a frame grown by the
list's largest filter reach and skips nothing by bounds first, so a tile
costs a scan of every command and full-frame group buffers; there is no
spatial index, no tile cache between frames and no threading. Damage
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
- **Paint order.** Atomic inlines are ordinary block fragments with
  `BlockFragment::line`; paint still paints them in the block phase instead
  of atomically in the inline phase of CSS 2.2 Appendix E.
- **Cost.** Each atomic inline's baseline search scans its subtree, which is
  O(n·depth) for nested inline-blocks; it is metered by `MAX_FLOW_WORK`.

**Reason:** these cover the common inline-block/image cases with exact tests
while keeping each spec corner its own unit. **Pay:** baseline sets on
fragments (first/last per box, computed bottom up), relative offsets for
split boxes, floats placed during line layout, Appendix E atomic painting in
`rustic-paint`. **Trigger:** inline WPT reftests or real-page renders with
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
WPT revision; 38 pass. `expected_failures.txt` gives each failure's reason:
23 need vertical writing modes, 12 need decoded images (the harness loads
none), and tables, SVG, form controls, `fieldset`, `list-item`, quirks-mode
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
