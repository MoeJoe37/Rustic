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
| TD-36 | Deliberate | Style | Filtered `:nth-child(... of S)` mutations use full restyles | Open |
| TD-35 | Watch | CSS / tests | The CSS conformance denominator follows property support | Guarded |

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
