# Implementation progress

This ledger describes completed code, not a promise of web compatibility. The
[roadmap](ROADMAP.md) remains the full plan. Each finished unit is committed and
pushed with focused tests and updated contracts.

## DOC-01 — Shared document-core primitives

Implemented in `rustic-base`, with no external dependencies and no unsafe code:

| Contract | Behavior | Regression evidence |
| --- | --- | --- |
| `DocumentId` | Process-local monotonic allocation; exhaustion never wraps or reuses an ID | Concurrent allocation and exhaustion tests |
| `NodeId` | Typed document/slot/generation identity; ownership and generation are checked by storage | Foreign/stale/vacant/out-of-range checks in the DOM arena |
| `Budget` | Explicit charge/release; failures preserve usage; arithmetic never wraps | Exact-limit, overflow, zero-capacity and release-underflow cases |
| `CancellationToken` | Shared one-way cooperative cancellation, including across threads | Clone sharing, repeat cancellation and independent-token cases |
| `SourceSpan` | Half-open byte offsets; checked bounds and UTF-8 slicing | Reversed ranges, extreme offsets and split multibyte characters |
| `Clock` | Monotonic elapsed time; real and agent-local manual implementations | Deterministic advancement and overflow preserving time |

IDs are labels, not permissions or strong references. A `NodeId` constructor
cannot create or authorize a node. Lookup must reject the wrong document,
generation, vacant slot and out-of-range slot. Cross-process identity and IPC
authorization remain future work.

Budget units belong to the consumer: node count, input bytes or another explicit
resource. This primitive is not a total memory limit and does not automatically
free resources. Cancellation is checked cooperatively; it does not preempt blocked
code. Source spans refer to the caller's matching buffer; they are byte offsets,
not JS UTF-16 indices. `SourceLocation` (DOC-01c below) ties a span to a source
revision.

## DOC-01c — Structured diagnostics and redaction

`rustic-base` now provides the shared diagnostic contract (`base.diagnostics`):

| Contract | Behavior | Regression evidence |
| --- | --- | --- |
| `SourceId` / `SourceRevision` | Never-reused source identity plus a non-wrapping revision counter; every text change takes a new revision | Uniqueness, ordering and `u64::MAX` exhaustion |
| `SourceLocation` | A `SourceSpan` tied to the revision it was measured in; resolving against another source or revision fails instead of slicing different text | Stale, foreign, out-of-bounds and split-UTF-8 cases |
| `Diagnostic` | Severity, validated stable code (`html.eof-in-tag` style), fixed `&'static str` summary, optional location and ordered named fields | Name validation, field order, `Debug` redaction |
| `DiagnosticValue` / `RedactionPolicy` | Every field value is `Public` or `Private`; rendering redacts private values to `<redacted>` (no length) unless a caller explicitly chooses `Reveal` | Default policy, both policies, `Debug` never printing private text |
| `EscapedText` | Quoted single-line output that escapes quotes, backslashes, C0/C1 controls, DEL, line/paragraph separators, bidi controls and zero-width characters | Log-forging, ANSI and Trojan-Source style inputs; non-Latin text kept |
| `DiagnosticLog` | Ordered log with entry-count and byte budgets (code, summary, keys and raw values); a rejected push changes nothing | Exact entry/byte boundaries, zero capacity, atomic rejection |
| `render` | Deterministic one-line format within a caller byte cap; exceeding it returns no partial output | Exact-cap and cap−1 cases for single entries and whole logs |

Because summaries and names are fixed engine strings, content-derived data can
only enter a diagnostic through a field with declared sensitivity. Privacy is
the default: the policy must be changed deliberately to reveal anything.

`rustic dump-dom` uses `EscapedText` for the user-supplied path and unknown
options in its error messages, so a file name containing a newline, ANSI escape
or bidi override prints as one inert line (CLI test
`dump_dom_escapes_hostile_paths_and_options_in_errors`).

Known limits, recorded as deliberate follow-up work: the HTML tokenizer and tree
builder keep their own compact `Copy` diagnostics, whose positions are
normalized code-point offsets and token indexes rather than original-byte spans.
Converting them needs original-byte offset mapping, which is separate HTML work;
until then they are not routed through `DiagnosticLog` ([TD-15](TECH_DEBT.md)). Line/column display,
UTF-16 positions, structured (JSON) export and a process-wide logging sink are
not implemented. Redaction classifies values at the point they are recorded; it
does not detect secrets inside text that a caller wrongly marks `Public`.

## DOC-03 and DOC-04a — Document-owned tree core

Implemented in `rustic-dom`, depending only on `rustic-base`:

- Protected document root, elements, text, comments and document type nodes.
- Parsed namespace/prefix/local-name storage; stable ordered attributes keyed by namespace/local name.
- UTF-16 text and values, including unpaired surrogates, with no implicit lossy conversion.
- Read-only node inspection; only the owning document edits links and payload accounting.
- Same-document insert-before, append, reorder and detach; validation precedes unlinking.
- Document root/doctype ordering, cycle prevention, child-kind rules and subtree depth checks.
- Per-document live-node and retained-string budgets, including detached nodes.
- Explicit iterative subtree destruction; slots reuse new generations and retire at generation exhaustion.
- Stack-free preorder traversal, restricted to the selected attached or detached subtree.

Validation covers invalid/foreign/stale handles, generation exhaustion, root
protection, no-op/self moves, invalid references, failure atomicity, exact budgets,
namespace attribute replacement, and a 1,500-edge tree. A seeded 1,500-operation
mutation test compares every parent/child/sibling link with an independent model
using ordered vectors instead of the implementation's sibling links.

The internal model follows the supported node hierarchy and tree concepts from
the [DOM Standard](https://dom.spec.whatwg.org/#concept-node-tree) and
[mutation algorithms](https://dom.spec.whatwg.org/#concept-node-ensure-pre-insertion-validity),
reviewed 2026-11-07 against the living document last updated 2026-11-04. This is
not a conformance claim: public DOM APIs, shadow trees, mutation
records/observers and script bindings remain unimplemented. General fragments,
processing instructions, CDATA sections, cloning and adoption are DOC-04c below. Internal parsed names do not perform the
validation or case normalization required by future public `createElement*` APIs.

Detachment retains node identity and data. Explicit destruction is a separate
engine operation that invalidates all handles in that subtree; it is not the
semantics of JS-visible DOM removal and will not replace future tracing GC.
String budgets account for UTF-8 names and UTF-16 payload length, not allocator
capacity/overhead, caller buffers, or total process memory. `Box<Node>` keeps live
node allocations stable even when the slot table grows. Resource limits are
checked before logical mutation; process-wide allocation failure is not a
recoverable guarantee.

## DOC-04b — Checked character-data storage edits

Text/comment values now support UTF-16 range replacement, raw-unit append and
direct UTF-8 append, with checked offsets, clamped counts and overflow handling.
The document preflights its shared payload budget and reserves capacity before
changing content/accounting. Private growable storage reuses capacity for parser
text coalescing; it preserves lone surrogates, cloned snapshots, node identity
and links. Existing whole-value replacement keeps its return-value contract.

Eight focused tests cover exact shared budgets, rejected edits, detached-node
charges, deletion/destruction, invalid handles/kinds/offsets, surrogate edits and
4,096 chunked appends. A 2,500-operation independent UTF-16 model checks accepted
and rejected edits after every step; a separate arithmetic test covers impossible
length/byte overflow without large allocations. [DOM.md](DOM.md) defines the
contract, specification reference, capacity accounting and remaining web APIs.

These primitives support the initial tree builder described below; they are not
a full `CharacterData` implementation. Live ranges, mutation records/observers and
bindings remain pending. No additional whole-engine capability gate is completed.

## DOC-04c — Full node kinds, cloning, adoption and lifecycle

`rustic-dom` now has every DOM tree node kind (shadow roots came with CSS-07b): free-standing
`DocumentFragment`s, `ProcessingInstruction` (XML `Name` target, data without
`?>`) and `CdataSection` (data without `]]>`) join documents, doctypes, elements,
text and comments. Processing-instruction and CDATA data use the checked
character-data edits; a processing instruction's target stays fixed and charged.
Inserting any fragment, including template contents, moves its children in
order after validating all of them. Pre-insert checks follow the DOM step
order, and the ancestor check walks host-including ancestors, so a template can
no longer be inserted into its own contents.

`clone_node(node, deep)`, `adopt_node(source, node)` and `clone_document()` share
an iterative snapshot-then-materialize engine. Templates get their own contents
(filled when deep). Adoption copies the subtree into the target with new IDs,
destroys it in the source and returns an old-to-new ID map (TD-16). Any node,
data, depth or allocation failure rolls back what was created, so both documents
stay unchanged. `ReadyState` adds the document lifecycle: parser documents start
`Loading` and only move forward; script-style documents start `Complete`.
[ADR 0011](adr/0011-fragments-cloning-adoption.md) records the design and
[DOM.md](DOM.md) the contracts. The HTML parser does not yet set readiness, and
live ranges, mutation records and the inert template owner document are pending.

Sixteen tests in `tests/node_kinds.rs` and `tests/adoption.rs` cover fragment
moves, atomic rejection, step order, host-including cycles, kind validation and
accounting, deep/shallow clones, limit failures in both documents, ID maps,
stale source IDs, document cloning and readiness. `dom.nodekinds` and
`dom.adoption` are complete.

## DOC-04a — Deterministic inspection

`Document::dump_tree` now emits a preorder diagnostic dump of one valid subtree,
with a caller-provided output byte cap. It omits IDs and allocation history,
preserves attribute order, quotes names and values, and escapes unpaired UTF-16
surrogates, control characters and line separators. A failed dump returns no
partial string. The format is a development diagnostic, not HTML serialization
or a stable embedding protocol.

Golden output tests compare independently constructed documents with different
allocation histories. Other tests cover exact output limits, Unicode escaping,
invalid roots, sibling/detached-node exclusion and escaped names.

Run a real constructed-tree demonstration:

```sh
cargo run --locked -p rustic-dom --example tree
```

The example creates a doctype, HTML/body nodes, an attribute, mixed Arabic/Latin
text and an SVG-namespaced node. There is no parser involved; the separate
parser-backed `dump-dom` command arrived with DOC-07.

## DOC-02a — URL records, parsing and origins

`rustic-url` now implements immutable URL records, original special/generic URL
and reference parsing, ASCII and Unicode (UTS #46 IDNA) hosts, IPv4/IPv6,
encoding/serialization and structured origins. It depends only on `rustic-base`;
IDNA uses generated Unicode 17.0.0 tables (see [URL.md](URL.md)). File/blob URLs
return explicit unsupported errors. Limits and cooperative cancellation are integrated;
the supplied base is immutable even on failure. Debug/error output omits userinfo
and private URL payloads.

DOC-02b adds `file:` URLs and validation-error diagnostics; see the section below.

Tuple origins compare canonical scheme/host/port. Opaque origins have unique
process-local identity; `null` is never compared as authority. Each URL-origin
derivation for a non-tuple URL is fresh, while cloning an assigned `Origin`
retains its identity. Document inheritance and navigation authority remain future
work, separate from URL parsing.

Eleven focused regression tests cover canonicalization, references, addresses,
origin equality/hashing, resource boundaries, cancellation and sensitive debug
output. An offline WPT-data runner passes 685 statically selected cases from 899
upstream records: 443 positive, 242 negative, 214 excluded. The exact revision,
source/generated digests, indices, license, selection rules and untested fields
are recorded; this is not whole-WPT or complete URL conformance. See [URL.md](URL.md)
for contracts, evidence and the runnable `resolve` example.

## DOC-02b — File URLs and URL validation diagnostics

`rustic-url` now parses `file:` URLs with the standard's file, file slash and file
host states: empty/`localhost` hosts, UNC-style hosts, Windows drive letters
(including `C|` and drive letters in the host position) and resolution against
file bases. The rules are platform-independent; parsing does not touch the file
system, and file URLs get a fresh opaque origin. `ParseError::UnsupportedFile` is
gone. Native path conversion and access policy remain loader/platform work.

`UrlParser::parse_with_diagnostics` returns the unchanged parse result plus up to
64 standard-named validation errors with byte offsets into the original input,
covering the URL parser, host code points and all IPv4/IPv6 errors. IDNA-specific
errors arrive with the IDNA adapter.

Evidence: the pinned WPT subset grew from 685 to 815 cases (all 130 file cases
pass), then to 887 once IDNA allowed the 72 non-ASCII records (only 12 blob
records remain excluded), every expected failure there yields a diagnostic, two new
file tests and eight diagnostics tests in `rustic-url`, and the URL fuzz target
now also uses file bases and checks diagnostic offsets and result equality.

## DOC-05a — Incremental HTML input, tag and declaration states

`rustic-html` now implements explicitly UTF-8 streaming input and the initial
data/tag/attribute tokenizer with comments, doctypes and processing instructions.
BOM stripping, invalid-sequence replacement and
CR/CRLF preprocessing survive arbitrary byte boundaries. Token grouping is
deterministic across feeds. Names use ASCII case normalization; attributes retain
source order and the first duplicate. Supported syntax errors have bounded
diagnostics and recovery. EOF, budgets and cancellation have defined terminal behavior.

Twenty-nine focused integration tests include every two-byte input compared with a separate
whole-buffer decoding/preprocessing oracle, fixed expected tokens/errors, all
fixture split positions, one-byte feeds, exact budgets and poisoned/closed stream
checks. One compiled rustdoc example and a runnable `tokenize` example accompany
the crate. See [HTML.md](HTML.md) for specification references, resource units,
diagnostic offsets and API contracts.

Comment/declaration, doctype and processing-instruction states have separate
modules. Doctype fields distinguish absent and empty identifiers and preserve the
force-quirks recovery rules. Invalid declarations/targets recover as bogus
comments; HTML-context CDATA has defined recovery. Token payload limits now cover
all markup, including keyword fallback and instruction-to-comment conversion.

An offline corpus test passes 1,079 statically selected html5lib cases from 1,846
records in five files, with complete token fields and ordered error codes checked.
It also passes 16,539 two-chunk splits and 1,079 one-byte runs, comparing local
diagnostic offsets across feeds. Provenance, exclusions, notices and untested
fields are pinned; the importer has three Python tests. This begins DOC-08's
corpus work without claiming full tokenizer conformance or fuzzing.

DOC-05a's initial data/tag/comment scope is complete. Its capabilities describe
their declared tested scope; full tokenizer/tree construction remains unfinished.

## DOC-05b (partial) — Character references

Named/numeric references now expand in data and quoted/unquoted attributes, with
longest-name matching, legacy attribute ambiguity, Unicode/control recovery and
EOF handling. Expanded characters are appended as data rather than retokenized.
The original lookup/state machine uses a bounded name probe and constant-size
numeric accumulator, with decoded-payload budgets and cooperative cancellation.

Eleven new focused tests cover state restoration, offsets, decoded markup/quotes,
two-scalar values, malformed prefixes, large numbers, exact budgets and poisoned
streams. An offline reference corpus passes all 4,626 upstream cases with zero
exclusions, plus 43,069 two-chunk splits and 4,626 one-byte runs. The declaration
subset remains unchanged. Generated WHATWG standard data has 2,231 names, a
pinned source snapshot/digests, a reproducible generator and retained notice.
See [HTML.md](HTML.md) for algorithms, provenance and untested contexts.

`html.references` is complete for its algorithmic scope. Foreign-content CDATA,
broader tree construction and loading remain pending. No full-parser claim is made.

## DOC-05b — Text modes and token-at-a-time drive

RCDATA, RAWTEXT, script-data escape/double-escape and plaintext states are now
implemented in a focused text module. Appropriate end-tag matching preserves
original spelling on fallback and bounds probes by the known context; long
nonmatching names/keywords stream as literal text. EOF/null behavior is explicit.

The pull driver separates input submission/closure from token delivery, allowing
mode feedback at a delivered start-tag boundary within the same queued chunk.
Input ownership, amortized compaction, token queuing and lifecycle live apart from
syntax state modules. Invalid control operations preserve state; fatal processing
errors terminate it. Collecting feed/finish helpers remain available. [ADR 0008](adr/0008-tokenizer-drive.md)
records this contract and pending script reentry/encoding/fragment controls.

Thirteen focused tests cover exact token behavior, same-chunk feedback, queued
append/read order, chunk invariance, long probes, limits, cancellation and lifecycle.
A pinned text corpus passes 109 selected records in 282 contexts, with 1,902
two-chunk splits and 282 one-byte runs. Unicode fixture adaptation preserves an
intentional leading U+FEFF without changing the byte decoder's BOM contract.
Earlier declaration/reference selections still pass. The importer and manifest
checker account separately for records, selected contexts and omitted states.

`html.textmodes` completes its declared four-mode scope. DOC-05b's broader encoding
work, foreign-content CDATA, broader tree construction and the corpus/fuzz gate remain
pending. The new example demonstrates tokenizer feedback, not a document parser.

## DOC-06a — Initial document construction subset

`HtmlDocumentParser` now owns the tokenizer and a checked DOM until successful
EOF. Initial/before-html/before-head/head/noscript/after-head/text modes create
explicit/implied shell nodes, select text contexts and compute compatibility
mode. A small body subset adds p/div/span/br/hr/plaintext, paragraph/end-tag
recovery, repeated html/body attribute merging, text coalescing and post-body
recovery. Syntax families, driver lifecycle and DOM mutation remain separate.

Fatal errors discard private partial trees and poison the parser; unsupported
body families, templates, framesets and instruction nodes fail explicitly.
Scripting is disabled and input is fixed UTF-8. [ADR 0009](adr/0009-parser-owned-construction.md)
records ownership, and [HTML_TREE.md](HTML_TREE.md) lists exact accepted/rejected
constructs, diagnostics, resource contracts and pending web integration.

Sixteen focused tests cover fixed trees, text-mode feedback at all byte splits
and three token sizes, attribute/whitespace/paragraph recovery, exact DOM and
diagnostic budgets, Unicode, unsupported inputs, cancellation and lifecycle.
A pinned 45/112-record tree subset passes complete-tree comparisons, all 905
two-chunk splits and 45 one-byte runs. Offline regeneration retains the complete
upstream source, source/fixture digests, selection counts and MIT notice. Four
Python regressions and ordinary repository checks validate importer provenance.

`html.tree` is now visibly in progress in the README map; broad body rules,
tables, formatting, templates, foreign content, script reentry and the full
corpus/fuzz gate remain unfinished. No new completion credit is awarded yet.

## DOC-06b — Complete document tree construction

The tree builder now implements all 21 document insertion modes: the full body
rules, the list of active formatting elements and adoption agency, tables with
pending character tokens and foster parenting, the current customizable-select
rules, templates with a template-mode stack, framesets, and SVG/MathML foreign
content with name/attribute adjustment and integration points. The tokenizer
gains CDATA sections, enabled per token by the tree builder. `rustic-dom` gains
template contents fragments owned by their template ([ADR 0010](adr/0010-template-contents.md)).
Body, table, template and frameset constructs no longer return `Unsupported`;
only processing-instruction tokens do. Scripting remains disabled.

All 57 top-level html5lib tree-construction files at the pinned revision are
vendored with a digest manifest. **1,580 of the 1,584 document records** that do
not need fragments, scripting or instruction nodes pass complete-tree comparison
for unbroken input, all 70,993 two-chunk splits and one-byte feeds; the four
explicit known failures need `selectedcontent` mirroring. Focused tests cover
template ownership and 20,000-deep template nesting, CDATA permission, foreign
adjustments, foster parenting, adoption, framesets and newline skipping. The
earlier 45-record subset and its generator are replaced by the full runner.
See [HTML_TREE.md](HTML_TREE.md) for exact boundaries.

`html.tree` and `html.tables` are complete for document parsing. The fragment
parsing algorithm, scripting/reentry, encoding restart, `selectedcontent`,
processing-instruction nodes and the fuzz gate remain open.

## DOC-06c — HTML fragment parsing algorithm

`HtmlFragmentParser` runs the WHATWG fragment parsing algorithm for an HTML, SVG
or MathML `FragmentContext` (with optional attributes), sharing the document
parser's tree builder, budgets, lifecycle and failure atomicity through one
private driver. The context selects the tokenizer state (no last start tag) and,
standing in for the scratch `html` root, the reset insertion mode; a `template`
context pushes "in template" and a `form` context is the form pointer. The
adjusted current node now drives HTML/foreign dispatch, CDATA permission
(including before the first token, a case the corpus does not cover) and foreign
element namespaces. Fragment-case rules cover `</html>`, `</frameset>` and
`input`/`select` in a `select` context; foreign breakout now reprocesses with
the insertion mode's rules as the standard says. `ParsedFragment::root()` is the
scratch `html` element whose children are the fragment; the detached context is
destroyed at EOF.

**All 192 html5lib fragment records** now pass for unbroken input, every
two-chunk split and one-byte feeds, so the tree runner passes 1,772 of 1,776
runnable records (74,661 splits); the four known failures remain document
`selectedcontent` records and only 16 scripting/instruction records are
skipped. Seven focused tests cover tokenizer states, the absent context node,
table/template/select/form and foreign contexts, integration points, CDATA,
splits, budgets, cancellation and lifecycle. A fourth fuzz target,
`html-fragment`, picks one of 28 contexts per input; 300,000 mutated executions
found no failures. See [HTML_TREE.md](HTML_TREE.md). DOM-string/UTF-16 fragment
input and `innerHTML` bindings remain future work.

## DOC-06d — `selectedcontent` mirroring

The tree builder now tracks option selectedness for single-selection selects
(the last option inserted with `selected`, otherwise the first non-disabled
option) and runs "maybe clone an option into selectedcontent" when an option
leaves the stack of open elements, including the stop-parsing pops at EOF. The
selected option's children are deep-cloned with `Document::clone_node` (DOC-04c)
into the select's enabled `selectedcontent` (none for `multiple`, or when the
first one sits in a nested select), replacing earlier mirrored contents. Clones
consume the ordinary node and payload budgets; a limit failure is fatal like any
other DOM failure. Selectedness is evaluated when the parser notices the pop, at
the end of the token that caused it, which yields the same final tree.

The four `webkit02.dat` records that were known failures now pass, so **all
1,776 runnable html5lib tree records pass** at every split and `KNOWN_FAILURES`
is empty. Two focused tests cover default/explicit/disabled selection,
`multiple`, clone independence and exact node budgets with mirrored clones.
Mirroring after parsing (user selection changes, script-set `selected`) is DOM
element behavior for the bindings milestone.

## DOC-07 — Bounded local loader and `dump-dom`

`rustic dump-dom [OPTIONS] [--] <file>` loads exactly one local file granted on the command line and prints the
deterministic `Document::dump_tree` output. The loader lives in
`rustic-cli/src/loader.rs` and only calls the public `HtmlDocumentParser`
feed/finish API; it adds no tree-construction rules of its own.

Arguments with a URL scheme of two or more characters (`https:`, `file:`,
`data:`, `about:` and similar) are rejected before any filesystem access;
single-letter prefixes stay paths for Windows drives. Directories, devices and
FIFOs are rejected from metadata before opening and again through the handle.
The file is read in fixed chunks (default 64 KiB) and never buffered whole. Its
byte limit (default 8 MiB) is checked from metadata and enforced while reading,
and also becomes the tokenizer's raw input budget. Parser token/node/depth/
payload/diagnostic limits keep their defaults, and dump output is capped at
64 MiB. Because exceeding a parser resource limit fails the whole load, large or
deep pages can raise `--max-nodes` and `--max-depth` (default 256).
`--max-diagnostics` (default 1,024, applied to tokenizer and tree recoveries
separately) only bounds retained recoveries; later ones are counted, not fatal;
`--max-input-bytes`, `--max-output-bytes` and `--chunk-bytes` set the rest. Cancellation is checked between reads. Any failure prints nothing on
stdout: usage/URL errors exit 2, load/parse/budget failures exit 1. Unsupported
constructs (processing instructions) fail explicitly rather than dumping a partial
tree. No network, script, subresource, encoding sniffing or base-URL resolution occurs.

Nine loader unit tests cover URL/path classification, exact file and dump byte
limits, chunk-size invariance at every size up to the input length, parser
budgets, unsupported markup, directories, `/dev/zero`, missing files, zero limits
and cancellation. Eight CLI tests compare a malformed fixture against a checked
snapshot for 19 chunk sizes and check URL rejection, usage errors, exact budgets,
configurable depth/diagnostic/node limits and clean failures. `html.loader` is complete for this scope; tree coverage
still grows with DOC-06.

## DOC-08 — Parser fuzz targets and CI smoke job

`rustic-fuzz` is a dependency-free, deterministic mutation fuzzer in safe,
portable Rust (standard library only, so it builds on every supported host). It
is blind, not coverage-guided: coverage engines and sanitizers stay behind the
[dependency review](DEPENDENCIES.md). Five targets check crash-freedom at
default budgets plus a differential invariant:

| Target | Invariant beyond "no panic, no hang" | Seeds |
| --- | --- | --- |
| `html-tokenizer` | Tokens and diagnostics match the unbroken input for a two-chunk split, fixed-size chunks and (up to 512 bytes) one-byte feeds | Checked-in pages plus all 1,792 html5lib tree `#data` inputs |
| `html-document` | The same feed plans yield the same dump or the same `ParseError`; completed trees always dump; a fresh parser never reports a lifecycle error; a cancelled parser never completes | Same as above |
| `html-fragment` | The same feed-plan equality for fragment parsing, with one of 28 HTML/SVG/MathML contexts chosen from the input's fingerprint (added with DOC-06c) | Same as above |
| `css` | Token spans are ordered, nonempty, on character boundaries and cover all non-comment input; stylesheet and nested block parsing are total and deterministic (added with CSS-01); typed longhands serialize to a fixed point (CSS-03) | Checked-in sheet plus the css-parsing-tests inputs |
| `css-selector` | Every valid selector list serializes to a fixed point with equal specificities and selects the same elements of a fixed document in HTML, quirks and XML modes (added with CSS-02) | Checked-in selectors plus the pinned `An+B` inputs |
| `css-cascade` | Styling a fixed document with an arbitrary author sheet is total and unchanged by splitting the sheet, repeating it or adding an empty user sheet (added with CSS-04); computed values stay finite and the style dump always succeeds (CSS-05) | The `css` seeds |
| `css-invalidation` | A fixed document styled with an arbitrary sheet and mutated by a script of up to 96 attribute, insertion, removal, move, text and container size changes restyles incrementally to exactly a full restyle (added with CSS-06) | The `css` seeds |
| `css-media` | Media query list serialization is a fixed point after one round and the reparsed list matches the same desktop, phone, print and zero-size environments; the same input as an `@container` prelude also serializes to a fixed point with unchanged names, axes and results (added with CSS-06) | `corpus/css-media` |
| `url` | Parsing with no base and five pinned bases is total; every success reserializes to a fixed point and an equal record | Inputs and bases of the pinned WPT URL cases |

Feed plans come from the input's FNV-1a fingerprint, so a saved file replays the
same check alone. Each input runs on a fresh worker thread with a time limit;
panics are caught at the thread boundary and a timeout abandons the worker and
fails the run. A campaign replays every seed and saved regression, then applies
seeded SplitMix64 byte mutations (bit flips, interesting bytes, range
deletion/duplication/reversal, cross-seed splicing and target dictionaries). The
first failure is minimized by chunk deletion (timeouts are kept as found) and
saved as `<target>/<kind>-<fingerprint>.bin` with a note holding the seed,
iteration and replay command. `rustic-fuzz replay` re-runs any saved file.

The required CI job **Parser fuzz smoke** replays all seeds and runs 20,000
fixed-seed mutations per target, uploading reproducers on failure; a separate
nightly/dispatchable `Fuzz` workflow runs ten-minute campaigns per target with a
fresh seed and is not a merge gate. Five harness tests inject panicking,
violating and hanging targets to check classification, minimization, reproducer
contents, seed-first stopping and per-seed determinism, and replay every seed of
every target; unit tests pin the generator, mutation bounds and literal
decoding. Before landing, 1.2 million mutated executions over five seeds found
no failures, so `regressions/` is empty. That is evidence of robustness at the
tested depth, not proof of absence of defects.

## DOC-09 — DOM/host tracing prototype and DOM conformance gate

`HostHeap` (`crates/rustic-dom/src/document/tracing.rs`) is a stop-the-world,
nonmoving mark/sweep model of the future JS heap, bound to one document, so the
DOM side of the tracing contract is tested before a runtime exists
([ADR 0012](adr/0012-dom-tracing-prototype.md), [DOM.md](DOM.md)). Objects have
generation-checked `HostObjectId`s; node → object, object → node and
object → object edges and counted node/object roots are registered explicitly
and bounded by `HostHeapLimits`. Holding any node keeps its whole tree; a
template and its contents keep each other; the document's tree is always live.
`collect` reserves all temporary storage first, marks iteratively, destroys
unreachable parentless trees through `destroy_subtree` (their IDs go stale),
frees unreachable objects, and prunes entries naming nodes destroyed or adopted
away by the engine.

The gate half adds `Document::check_invariants`, a linear structural checker,
and `tests/conformance.rs`, which runs all 25,230 (parent, node, reference
child) combinations over a fixture with every node kind, connected and detached
trees, template contents and four free fragments against an oracle transcribed
from the DOM Standard's pre-insert steps (reviewed 2026-11-08). It checks the
exception class and step precedence, resulting child order, unchanged trees after
a rejection and invariants after every call. It found and this change fixes two
gaps in `insert_before`: the document node was rejected before the ancestor and
reference steps, and re-inserting the existing document element or doctype into
the document was accepted as a reorder instead of `HierarchyRequestError`. The
`html-document` and `html-fragment` fuzz targets now also check invariants on
every completed tree; 600,000 mutated executions found no violations.

Ten tracing tests cover rooted detached trees, wrapper/listener cycles, connected
nodes keeping objects, template liveness both ways, engine destruction and
adoption pruning, foreign/stale/node-to-node rejections, exact limits, a
100,000-object ring through a 256-deep chain, and 200 seeded graphs compared with
an independent fixpoint model. A unit test corrupts links, kinds, accounting,
parent cycles and document order to show the checker detects each. Not covered:
a real JS heap, multi-document heaps, incremental collection, replace/remove
algorithms (no such APIs yet) and web-platform-tests, which need bindings.
## CSS-01 — CSS syntax tokenizer and parser

`rustic-css` implements CSS Syntax Level 3 tokenization, component values and
the stylesheet/rule-list/rule/declaration/block-contents/component-value entry
points, with CSS Nesting's block contents (declarations, at-rules and nested
qualified rules) and byte spans on every token, value, rule, declaration and
diagnostic. Rule blocks stay raw component values; `parse_block_contents` reads
them on demand. Input/token/nesting limits and cancellation are fatal with no
partial result; diagnostics are bounded with a dropped count. The algorithms are
iterative and the nesting limit bounds result depth.

The vendored CC0 css-parsing-tests corpus passes **139 of 149** cases (9
unicode-range cases excluded, 1 documented 2021-vs-Nesting difference) after the
adaptations listed in [CSS.md](CSS.md). Eight focused tests cover spans,
escapes, recovery, numbers, nested blocks, exact limits, cancellation and entry
point errors; a fifth fuzz target, `css`, checks span coverage and totality
(300,000 executions, no failures). Byte-stream encoding, properties and the
cascade are later CSS tasks.

## CSS-02 — Selectors, specificity and matching

`rustic-css` now parses Selectors Level 4 syntax from component values: type,
universal, ID, class and attribute selectors (all operators, `i`/`s` flags and
`@namespace` prefixes), the four combinators, the structural pseudo-classes
including `:nth-child(An+B of S)`, `:is()`/`:where()` (forgiving) and `:not()`,
`:scope`, the link and user-action pseudo-classes, and five pseudo-elements.
Unsupported syntax (`:has()`, `&`, `||`, form-state pseudo-classes and others)
is rejected with an error kind and span. Specificity follows Selectors 4, and
CSSOM serialization is a tested fixed point. `parse_an_plus_b` implements the
CSS Syntax microsyntax on tokens: all **128** pinned `An+B.json` cases pass.

The new `rustic-style` crate matches selectors over an `ElementView` trait, with
`DomElement` for `rustic-dom` documents and `query_selector_all`. Matching
honors HTML-document case rules (including HTML's legacy case-insensitive
attribute values), quirks-mode IDs and classes, XML documents and `:scope`. It
runs right to left with an explicit stack and restart pruning, so neither long
selectors nor deep descendant chains recurse or backtrack exponentially. A sixth
fuzz target, `css-selector`, checks serialization and matching invariants
(600,000 executions, no failures). `:nth-*` sibling counting is uncached
(TD-20). See [CSS.md](CSS.md#selectors).

## CSS-03 — Typed property values

`rustic-css` parses `<color>` (named, hex, `rgb`, `hsl`, `hwb`, `lab`, `lch`,
`oklab`, `oklch` and `color()` with `none`), `<length>` (absolute,
font-relative and viewport units) and `<length-percentage>`, and turns
declarations into typed longhands for `display`, colors, margins, padding,
borders, sizes, `box-sizing` and font basics, expanding the box shorthands and
applying CSS-wide keywords. Every longhand has an initial value and an
inheritance flag. All **8,041** pinned css-parsing-tests color cases pass (76
within a float tolerance, because the expectations used 32-bit floats). Math
functions, `var()` and the wider property catalog are still open, so
`css.properties` has two of four steps. See [CSS.md](CSS.md#property-values).

## CSS-03b — Math functions

`rustic-css` parses `calc()`, `min()`, `max()` and `clamp()` in every
supported `<length>`, `<length-percentage>` and `<number>` slot: a bounded
recursive-descent parser (32 levels, 1,024 nodes) builds a calculation tree,
type-checks it against its context and simplifies it per CSS Values 4 §10.10,
and specified values serialize per §10.12. `rustic-style` computes trees with
pixel lengths and clamps single results to the property's range; a mix of
pixels and percentages stays a calculation for layout. Degenerate results are
censored (NaN to 0, infinities to the largest finite value). Typed division,
the Level 4 stepped/trigonometric/exponential functions and math functions in
colors are not supported yet. See [CSS.md](CSS.md#math-functions).

## CSS-03c — Property catalog

`rustic-css` adds typed longhands for positioning (`position`, the four
offsets, `z-index`, `float`, `clear`), `overflow-x`/`-y`, `visibility`,
`opacity`, `vertical-align`, text (`direction`, `text-align`, `text-indent`,
`text-transform`, `letter-spacing`, `word-spacing`, `white-space`,
`text-decoration-line`/`-style`/`-color`), fonts (`font-variant-caps`,
`font-stretch`) and the seven list-valued background longhands, with the
`inset`, `overflow`, `text-decoration`, `font` and `background` shorthands
(67 longhands in all). `rustic-style` computes them (blockification of floats
and absolutely positioned boxes, overflow pairing, `match-parent`, background
positions as offsets) and the HTML user-agent sheet uses them for `pre`,
links, decorations, `sub`/`sup`, table alignment and `dir`. Gradients, system
fonts, the Text 4 `white-space` longhands (TD-27) and `url()` resolution
(TD-26) remain. See [CSS.md](CSS.md#property-values).

## CSS-04 — Cascade and inheritance

`rustic-style` turns stylesheets into cascade-ready rules (`StyleSheet::parse`,
with diagnostics for every dropped rule, declaration or unsupported at-rule and
`@namespace` support) and computes each element's specified style: origin and
importance tiers, style attributes, specificity, order of appearance, the
CSS-wide keywords including `revert` across origins, and inheritance.
`style_tree` styles a document subtree. Computed values are CSS-05, so
inheritance passes specified values for now. The same change paid TD-21:
nested `:is()`/`:where()`/`:not()` matching is memoized per element and no longer
exponential. A seventh fuzz target, `css-cascade`, checks cascade invariants.
Cascade layers, shadow encapsulation and animation origins remained for
CSS-07b. See [CSS.md](CSS.md#cascade).

## CSS-07b — Cascade layers, shadow-tree contexts and animation origins

`@layer` statements, named and anonymous blocks and nested layers parse into
per-sheet layer lists; the cascade merges them per origin and tree into a layer
tree and applies the layer criterion with the important reversal, and
`revert-layer` now rolls back by layer (style attributes and the animation
origin act as their own layers). The animation and transition origins have
their tiers, fed through `StyleInputs` once an animation engine exists.
`rustic-dom` gained shadow roots (`attach_shadow`, owned like template contents
through destruction, adoption, dumps, tracing and invariants), and the cascade
gained encapsulation contexts: sheets belong to a tree, `:host`/`:host()`
style the featureless host from inside, outer normal and inner important
declarations win, `<style>` elements in shadow trees style their tree, and
`style_tree` walks shadow-including order with shadow trees inheriting from the
host. `css.cascade` is complete and `dom.shadow` has one of four steps. Slot
assignment is pending, so light children of a host inherit from the host
(TD-23). See [CSS.md](CSS.md#cascade) and [DOM.md](DOM.md#shadow-roots).

## CSS-08 — Declared CSS conformance gate

`tools/import_wpt_css_tests.py` extracts the literal `test_valid_value`,
`test_invalid_value`, `test_valid_selector` and `test_invalid_selector` calls
from a pinned WPT checkout (2,916 cases; 219 non-literal calls counted, not
guessed) and `tests/wpt_conformance.rs` runs the in-scope ones: 1,073 of
1,455 pass, and every other in-scope case is listed with a reason in a ratchet
expectation list. Four serialization bugs the gate found were fixed first.
`css.conformance` is complete. See [CSS.md](CSS.md#conformance-gate).

## CSS-05 — Computed values, stylesheet attachment and style dump

`ComputedStyle::compute` resolves the CSS-03 longhands to computed values:
lengths in pixels (font-relative, `lh` and viewport units included), font-size
keywords, percentages and `em`/`rem`, relative font weights, `currentcolor`,
border widths (zero under `none`/`hidden`) and root blockification, while
containing-block percentages stay for layout. `style_tree` is now the
full-restyle reference path: it cascades and computes every element and
inherits computed values. `document_cascade` attaches the HTML user-agent
stylesheet subset and the document's `<style>` elements (type and media
checked, statuses and diagnostics reported), `dump_computed_styles` prints a
deterministic dump, and `rustic dump-style <file>` exposes it with the
`dump-dom` loader budgets and a checked fixture snapshot. The restyle shares one
match cache, which also paid TD-20: `:nth-*` positions are memoized and derived
from the previous counted sibling, so long lists match in linear time. The
`css-cascade` fuzz target gained computed-value invariants. `css.values` and
`css.queries` each have one of four steps; variables, media queries and
external sheets remain. See [CSS.md](CSS.md#computed-style).

## CSS-06 — Media queries, invalidation, custom properties and container queries

`rustic-css` parses, serializes and evaluates Media Queries 4 lists (with the
Level 5 preference and capability features), including range syntax,
`<general-enclosed>` as unknown and three-valued logic. `@media` rules in
`rustic-style` sheets, nested to any depth, flatten into rules that carry their
media conditions; `<style media>` restricts a whole sheet. A `Cascade` holds a
`MediaEnvironment`, caches query results and reports from `set_environment`
whether any changed. An eighth fuzz target, `css-media`, found and pinned two
serialization bugs.

`StyledDocument` restyles incrementally: callers report attribute, child-list
and text mutations, an `InvalidationMap` built from the cascade's selectors
picks the affected elements (the element, its subtree, following siblings,
siblings and parent for structural selectors), and the walk reuses every other
style, restyling children only when a parent's computed style changed. `:has()`
and HTML element states first fell back to a full restyle for the mutations
they can see (TD-32); targeted invalidation later restyles only the possible
`:has()` anchors and the elements a state change reaches (see
[CSS.md](CSS.md#invalidation)). A ninth fuzz target, `css-invalidation`, checks incremental
results against full restyles after scripted mutations.

Custom properties (`--*`) and `var()` come next. Custom values are kept as
component values, and declarations that use `var()` are kept unparsed until
computed-value time. Each element's custom properties are resolved in
dependency order, with every cycle member guaranteed-invalid. Each `var()`
longhand is then substituted and parsed, and a failure acts as `unset`. Each
element has a substitution budget and a nesting limit. Serialization no
longer escapes an `e`-leading unit unless it could read back as an exponent
(`2em` stays `2em`).

Container queries complete CSS-06. `@container` preludes parse with the media
condition grammar and the container feature set, plus `style()` queries. The
`container-type`, `container-name` and `container` properties are typed.
Rules flatten like `@media`, as an always-matching media entry plus a
container query list. The cascade picks the nearest eligible ancestor
container for each condition. Style queries read that container's custom
properties. Size features read sizes that the caller supplies until layout
exists (`style_tree_with_sizes`, `StyledDocument::set_container_size`). With
container rules present, a changed computed style restyles the whole subtree.
See [CSS.md](CSS.md#media-queries), [CSS.md](CSS.md#invalidation),
[CSS.md](CSS.md#custom-properties) and [CSS.md](CSS.md#container-queries).

## External stylesheets — `<link rel=stylesheet>` and `@import`

`document_cascade_with` loads linked and imported sheets in tree order through
a caller-supplied `StylesheetFetcher`, so fetch policy stays with the embedder
and `rustic-style` never touches files or the network. A `StyleLoader` applies
document-wide budgets (fetched sheets, import depth, total bytes), refuses
import cycles, caches fetches, resolves URLs against `<base href>` and each
importing sheet, and orders imported sheets before their importer. Every
refusal is a per-sheet status, never fatal. `rustic dump-style` now loads
stylesheets with a local-only fetcher confined to the document's directory
(lexical and symbolic-link checks), with three new budget options and a
`linked.dump` snapshot. `rustic-style` now depends on `rustic-url`. UTF-8 is
the only stylesheet encoding (TD-28). `css.queries` now has its
external-sheet step; import media lists restrict imported sheets. See
[CSS.md](CSS.md#external-stylesheets).

## M2 review — structural invalidation

The 2026-11-08 review reproduced stale styles in three differential scenarios:
removing a sibling when `:first-child`/`:nth-child()` occurs in an ancestor
compound, changing membership of `:nth-child(2 of .hot)`, and changing `:empty`
text/children before a sibling subject. Structural changes now reach descendant
and following-sibling subjects even when the ancestor's own style is unchanged.
Filtered `of S` selectors conservatively use full restyles after mutations
until counting-scope dependencies exist (TD-36). All new cases compare against
the independent full-restyle path; ordinary targeted invalidation remains.

## REN-01a — Checked geometry, principal boxes and block widths

M3 has started in `rustic-layout`: checked CSS/device geometry, an immutable flat
DOM/style-to-principal-box snapshot, raw UTF-16 text with correct boxless-parent
inheritance, resource/cancellation guards, and normal-flow non-replaced block
width measurement. `rustic dump-boxes` exposes a bounded deterministic snapshot
through the existing local HTML/stylesheet loader. [LAYOUT.md](LAYOUT.md) specifies
the supported width algorithm and exact limits. Anonymous boxes, vertical flow,
fragments, text layout and painted images remain pending; REN-01 is partial.

## REN-01b — Initial block-only used geometry (partial)

`BlockLayout::compute` now emits physical content/padding/border rectangles
through an iterative top-down/bottom-up reference pass. It handles nested
non-replaced blocks, auto/fixed/min/max heights, border-box adjustment, definite
and indefinite percentage heights, calc, horizontal positioning and resize.
`rustic dump-layout` exposes a deterministic geometry fixture through the same
local HTML/style loader. Fourteen flow scenarios include an independent prefix-
sum model, depth/budget/overflow boundaries and atomic failure; CLI tests check
chunking, exact output limits, linked CSS and unsupported text.

This is a margin-free static block subset (TD-38). General normalization,
anonymous/inline boxes, margin collapse, special/replaced content, positioning,
overflow semantics and shaped text remain pending. Unsupported input returns
explicit errors, and no full block/fragment progress gate is claimed. See
[LAYOUT.md](LAYOUT.md) for the exact contract and follow-up units.

## Queue status

The README now includes an automatically regenerated engine progress map. Its
21-subsystem, 172-capability engine-v3 catalog (sized capabilities, roadmap
milestones and evidence-backed partial credit) includes unfinished libraries, all five native
platform acceptance targets and final release/security/conformance gates. Source
and test references are validated; score calculations and safe Git publication
have Python regression tests. See [PROGRESS.md](PROGRESS.md) for definitions
and the automatic post-CI default-branch update path. Reporting infrastructure
does not itself add engine completion credit.

The accepted full-product requirements now include Brave-comparable security/
privacy, broad standards/site coverage, measured Chrome/Firefox improvements,
required GPU/Wasm, Rustic-only MV2 extensions and a uBO port. [PRODUCT.md](PRODUCT.md),
[EXTENSIONS.md](EXTENSIONS.md) and ADR 0007 define tasks/gates; these features remain
planned. The catalog retained all 142 prior capabilities and added 30 unfinished
gates. The renderer now sizes the progress map for 21 subsystems. Scope expansion
earns no completion credit; the reweighting is recorded in [PROGRESS.md](PROGRESS.md).

Platform scope now includes Windows, macOS, Linux, Android and iOS/iPadOS, with
Mozilla/Firefox/Gecko and SpiderMonkey explicitly excluded as engine foundations.
MOB-01 configures seven shared-library cross-checks; see [MOBILE.md](MOBILE.md) and
[ADR 0006](adr/0006-desktop-and-mobile-platforms.md). Native mobile hosts,
link/package checks, runtime tests and device support remain planned.

Rustic must support any device that can run a browser ([ADR 0013](adr/0013-any-capable-device.md),
[PLATFORMS.md](PLATFORMS.md)). CI now runs the full suite natively on Linux,
Windows and macOS for both x64 and ARM64, under QEMU for 32-bit ARM, RISC-V and
big-endian s390x (32-bit x86 natively), and cross-checks eight more targets. The
first 32-bit run found a DOM test that built a multi-gigabyte dump, impossible in
a 32-bit address space; TD-18's linear dump format removes it. Every later task
must keep all tiers green.

| First implementation queue item | Status |
| --- | --- |
| DOC-01a: typed document/node IDs and generation validation | Complete through the DOM storage layer |
| DOC-01b: explicit budgets and cooperative cancellation | Complete primitives; integrated into URL parsing and initial HTML tokenization |
| DOC-01c: structured diagnostics and redaction | Complete base contract; used for CLI error escaping; HTML diagnostics migrate after original-byte offset mapping |
| DOC-03: minimal document arena and node kinds | Complete internal subset described above |
| DOC-04a: mutation and deterministic tree inspection | Complete initial same-document subset |
| DOC-04b: character-data mutation primitives | Complete checked text/comment storage edits; live ranges, observers and bindings pending |
| DOC-04c: node kinds, cloning, adoption and lifecycle | Complete: general fragments, PI/CDATA, clone, cross-document adoption with ID maps, `ReadyState`; mutation records pending |
| DOC-02a: URL records and tuple/opaque origins | Complete initial declared subset plus UTS #46 IDNA (all 6,389 Unicode IdnaTestV2 cases); blob remains pending |
| DOC-02b: file URLs and validation diagnostics | Complete: platform-independent file rules, 887-case WPT subset (all but 12 blob records), standard-named diagnostics with offsets |
| DOC-05a: incremental tokenizer data/tag/comment states | Complete initial scope, with doctype and HTML-context declaration recovery |
| DOC-05b: references and alternate text modes | Reference/text modes complete; broader encoding cases remain pending |
| DOC-06a: initial/head/body tree construction | Done for document parsing; fragment parsing and scripting pending |
| DOC-06b: tables/formatting/template/foreign tree rules | Done: all 1,584 runnable html5lib document records at every split (with DOC-06d) |
| DOC-06d: `selectedcontent` mirroring | Done: last four known tree failures fixed; 1,776/1,776 runnable records |
| DOC-06c: fragment parsing algorithm | Done: all 192 html5lib fragment records at every split; DOM-string input and `innerHTML` bindings pending |
| DOC-07: bounded HTML loader/CLI | Complete: local-only `dump-dom` with file, parser and output budgets; tree coverage follows DOC-06 |
| DOC-08: parser corpus/fuzzing | Complete: tokenizer corpora, the full runnable html5lib tree corpus (documents and fragments) at every split, exact limits, and nine fuzz targets with a required CI smoke job; coverage-guided fuzzing and sanitizers await dependency review |
| DOC-09: traced node/JS ownership prototype | Complete prototype: `HostHeap` roots/edges/mark-sweep, `check_invariants`, 25,230-case pre-insert conformance gate; the real heap arrives with JS-03 |
| CSS-01: CSS syntax tokenizer/parser | Complete: tokens, component values, rules, declarations, nested block contents; 139/149 pinned corpus cases |
| CSS-04: cascade and inheritance | Complete: origins, importance, style attributes, specificity, order, CSS-wide keywords and inheritance; layers, shadow contexts and animation origins added by CSS-07b |
| CSS-07b: cascade layers, shadow contexts, animation origins | Complete: `@layer` and layered `revert-layer`, DOM shadow roots, per-tree sheets with `:host`, context precedence, animation/transition tiers; slots, `::slotted()`/`::part()` and `@import layer()` pending |
| CSS-06: media queries, invalidation, custom properties, container queries | Complete: Media Queries 4/5, incremental restyle checked against full restyles, `var()` with cycles and bounded expansion, `@container` with size and style queries, `@property` registration with font-relative unit cycles; container units and layout-provided sizes remain |
| CSS-05: computed values, attachment and style dump | Complete for the declared longhands: computed values, HTML UA sheet subset, `<style>` attachment, `dump-style` and a cached full-restyle path; `var()` and media queries followed in CSS-06; font-metric units move to M3 (REN-03) |
| CSS-03c: property catalog | Complete for the declared set: positioning, overflow, visibility, opacity, text, decoration, font shorthand and backgrounds with computed values; gradients and system fonts remain |
| CSS-03b: math functions | Complete for lengths, length-percentages and numbers: parsing, type checking, simplification, serialization, computed values; typed division and the newer math functions remain |
| External stylesheets (`css.queries` step 2) | Complete: `<link rel=stylesheet>` and `@import` over a policy-owning fetcher with budgets and cycle refusal; `dump-style` loads local files beside the document only; non-UTF-8 sheets (TD-28) and layer/supports imports remain |
| CSS-03: typed property values | Complete for the declared set: colors (8,041/8,041 pinned cases), lengths and the box-model, display and font longhands with shorthands; math functions and the wider catalog remain |
| CSS-02: selectors and specificity | Complete for the declared subset: selector syntax, specificity, serialization, An+B (128/128) and DOM matching in `rustic-style` |
| CSS-07a: remaining required selectors | Complete: `:has()` (linear for single-compound arguments, TD-30), HTML form states, `:lang()`, `:dir()` (Bidi_Class via TD-29), `&` with nested style rules expanded by `StyleSheet::parse`, more pseudo-elements and user-action states after them; `css.selectors` is complete. Constraint-validation states and form owners set by the parser (TD-31) remain |
| CSS-08: declared CSS conformance gate | Complete: pinned WPT parsing cases for selectors and supported properties, 1,073/1,455 in scope pass, the rest listed with reasons in a ratchet; shorthand serialization, named-color keywords and `calc()` wrappers in specified values remain |
| TD-32: targeted `:has()` and state invalidation | Complete: anchors within the relative selectors' reach and their preceding siblings, subtrees under changed `lang`/`dir`/`disabled`/`contenteditable`, `dir=auto` roots and form-group controls; `css.invalidation` is complete. A pragma language change and states in nested `:has()` ancestor positions still restyle everything |

M1 remains in progress. DOC-04c added node kinds, cloning and cross-document
adoption; mutation records remain for the broader DOC-04 task. With DOC-06c and DOC-08 done,
`html.conformance` is complete and DOC-06d removed the last known tree
failures. M2 has CSS-01 through CSS-05 and CSS-07a (selectors) done and CSS-06
complete (media queries, invalidation, custom properties and container
queries), and CSS-08 closed the declared conformance gate with known failures
recorded rather than hidden. The M2 catalog scope is complete, including its
math-function and invalidation steps; broader CSS compatibility remains open.
M3 has started with REN-01a, while full static rendering remains unimplemented. All project crates remain safe
Rust, unpublished, and free of third-party Rust dependencies.
