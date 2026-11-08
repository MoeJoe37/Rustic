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
| TD-20 | Watch | Style | `:nth-*` selector matching recounts siblings on every test | Open, pay with CSS-05 restyle |
| TD-21 | Bad | Style | Nested `:is()`/`:where()`/`:not()` matching is exponential in nesting | Paid |

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

### TD-20 — `:nth-*` matching recounts siblings on every test

`rustic-style` (CSS-02) computes an element's position for `:nth-child()`,
`:nth-of-type()` and their `last` forms by walking its siblings on every test,
and `of S` re-matches `S` against each sibling. Matching every element of a
parent with *n* children against such a selector is therefore O(*n*²), which
is fine for the reference matcher and tests but not for full restyles of large
lists. **Pay:** cache sibling indices per parent for the duration of a restyle
(as browsers do) and reuse them across selectors. **Trigger:** CSS-05 full
restyle, or a measured slowdown on a page with large sibling lists.

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

## Reviewing new work

When reviewing a merge, check each new shortcut against the rules above. Add a
ledger entry for deliberate debt in the same change that takes it, mark entries
**Paid** with the paying change rather than deleting them, and keep the summary
table in sync with the sections.
