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

DOC-09 first tested the DOM side of the tracing contract against a prototype
`HostHeap` inside `rustic-dom`
([ADR 0012](adr/0012-dom-tracing-prototype.md), [DOM.md](DOM.md)). JS-03
replaced that stand-in with the real JavaScript heap: the DOM keeps tree
marking (`tree_marks`, `mark_tree`, `sweep_unmarked_trees`), and
`rustic-bindings` runs the shared collection. The rules are unchanged: holding
any node keeps its whole tree; a template and its contents keep each other; the
document's tree is always live; unreachable parentless trees are destroyed
through `destroy_subtree` (their IDs go stale).

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
guessed) and `tests/wpt_conformance.rs` runs the in-scope ones: 1,115 of
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

That initial slice was a margin-free static block subset (TD-38); REN-02a
below removes the margin restriction. General normalization,
anonymous/inline boxes, margin collapse, special/replaced content, positioning,
overflow semantics and shaped text remain pending. Unsupported input returns
explicit errors, and no full block/fragment progress gate is claimed. See
[LAYOUT.md](LAYOUT.md) for the exact contract and follow-up units.

## REN-02a — Static block margin collapse

The block reference now resolves adjoining sibling, parent-child and empty-
through margins, including negative values, root/flow-root and border/padding
boundaries, fixed/min/max heights and the nonzero-minimum-height exception.
Three iterative passes separate measured constraints, bottom-up collapsed
struts/relative positions, and absolute geometry; no completed subtree needs
repeated translation. The former `VerticalMargins` refusal is removed.

Ten margin scenarios, an independent adjacency-graph model over 192 generated
sixteen-sibling layouts, strut grouping tests, deep/budget/overflow cases and a
new CLI golden fixture provide evidence. The original zero-margin fixture is
unchanged. `layout.block` now has width, vertical flow and margin-collapse
steps complete; anonymous mixed inline/block flow remains planned. The full
capability, general fragments, text layout, floats/clearance and image output
are not marked complete. [LAYOUT.md](LAYOUT.md) defines the exact subset.

## REN-01c — Anonymous flow boxes and inline continuations

`FormattingTree` now derives a separate borrowed normal-flow forest from the
principal snapshot. Mixed block/inline children get minimal anonymous blocks;
block descendants split inline ancestors into source-linked continuations with
open-edge metadata, including empty before/after pieces. Inline-blocks remain
atomic outside while normalizing their own contents. Anonymous styles inherit
supported inherited values, reset other values, and respect custom-property
registration flags. Original UTF-16 stays in the principal snapshot; omitted
collapsible boundary whitespace is reported by source index.

Independent box/depth/work limits, cancellation and flat arenas bound expansion
without recursive traversal or destruction. Sixteen scenarios include a 508-tree
independent grouping model, deep splits, exact limits and atomic rejection;
three CLI tests cover the reviewed fixture, local sheets and documented defaults.
A new `layout-formatting` mutation target checks source/text partition and roles.
`dump-formatting` is structural; `dump-layout` remains the block-only geometry
reference. See [LAYOUT.md](LAYOUT.md) for the exact contract and TD-39's follow-up.

Only the existing normalized-box step of `layout.boxes` advances, with source
and test evidence. General fragments and mixed inline/block *used flow* remain
planned until REN-03 supplies shaping/line layout. M3 is still in progress.

## REN-04a/REN-05a — First display list, CPU raster and PNG output

`rustic-paint` builds an immutable display list from the block reference:
canvas background with root/`body` propagation, background colors clipped to
`background-clip`, and solid borders (one ring when the sides match, mitred
quads otherwise). Computed colors in every supported color space convert to
8-bit sRGB. `rustic-raster` clears a checked premultiplied surface, fills
shapes with exact-area antialiasing and composites source-over using only IEEE
`f64` arithmetic, so a golden checksum holds on every target. `encode_png`
writes stored-block RGBA PNG within an exact size budget. The CLI gains
`dump-paint` and `render --output <png> [--scale N]`; a `paint-raster` fuzz
target checks determinism, opacity and PNG framing.

Text, images, opacity, non-solid borders and stacking contexts fail with typed
errors. Catalog steps credited: `paint.display` (block display list),
`paint.raster` (fills/compositing), `paint.images` (color/alpha conversion) and
`paint.conformance` (deterministic PNG output). Shortcuts are TD-41. See
[PAINT.md](PAINT.md). M3 is still in progress.

## REN-03a — Font adapter and font-relative units

The new `rustic-text` crate parses TrueType (`glyf`) sfnt fonts from scratch in
safe Rust with no dependencies. Loading validates the table directory and the
`head`, `hhea`, `maxp`, `hmtx`, `cmap` (formats 4 and 12), `name`, optional
`OS/2` and `loca` tables with checked reads; outlines (simple and composite,
with transforms) are parsed on demand through an explicit work stack under
`FontLimits`. `FontDatabase` implements CSS Fonts 4 face matching (family list,
generic mapping and fallback, then stretch, style and weight). Face metrics
supply x-height and cap height (OS/2, else the `x`/`H` glyph tops) and the `0`
and 水 advances, with CSS fallbacks when absent.

`rustic-style` gains `FontMetricsSource` and an optional `ComputeContext::fonts`;
computed `ex`/`ch`/`cap`/`ic` and their root forms now use the first available
font, and `ComputedStyle::font_metrics` exposes the ratios. Without a source the
earlier fallback values are unchanged. Four original CC0 test fonts are generated
by `tools/generate_test_fonts.py` (checked byte for byte) and embedded by
`rustic_text::bundled`. Fourteen font tests cover metrics, cmap differential
lookup, short `hmtx`, outlines, matching, every truncation, 4,000 seeded
mutations and limit/cycle/format rejections; eight style tests cover units,
fallbacks, parent/root fonts, the query contract and incremental restyle
parity. A new `text-sfnt` fuzz target replays the fonts and their prefixes.
Three of four `text.fonts` steps are credited; discovery, CFF/WOFF, shaping and
query-unit fonts remain (TD-42, TD-43). See [LAYOUT.md](LAYOUT.md#font-adapter-ren-03a).

## REN-02b — Floats, positioning and overflow

The block reference is now one iterative top-down walk per block formatting
context, with an unresolved margin strut and a queue for floats and static
positions met before it resolves. REN-02a's margin fixtures and models pass
unchanged. New: floats with shrink-to-fit widths and §9.5.1 placement;
clearance against the hypothetical position; formatting roots that narrow
beside or move below floats, laid out again when an auto height meets a float;
relative offsets; absolute and fixed boxes solved by the §10.3.7/§10.6.4
equations against their containing block after it is laid out. Overflow is
supported without scrolling UI: used values with root/body-to-viewport
propagation, clips that follow the containing-block chain, and clamped
scrollable-overflow rectangles. Fragments gain `scheme()`,
`relative_offset()`, `containing_block()`, `overflow()`, `clip()` and
`scrollable_overflow()`. Dump lines add fields only for non-default values, so
the existing CLI goldens are unchanged.

Thirteen scenarios and an independent row-raster float model (1,200 positions)
are the evidence. Work is metered by `MAX_FLOW_WORK` (TD-44); spec-permitted
approximations are listed in TD-45. Sticky positioning stays an explicit
refusal. `layout.position` gains positioning, float and overflow steps; sticky
remains planned. See [LAYOUT.md](LAYOUT.md#floats-positioning-and-overflow-ren-02b).

## REN-03b (part 1) — Unicode segmentation and bidi

`rustic-text` gains generated Unicode 17.0.0 tables
(`tools/generate_text_data.py`, pinned inputs and outputs) and three modules:
`segment` (UAX #29 grapheme clusters and default UAX #14 line breaks) and
`bidi` (UAX #9 with isolates, bracket pairs and line rules L1–L2). The
complete LineBreakTest and GraphemeBreakTest suites and deterministic samples
of BidiTest and BidiCharacterTest are committed and pass; the full bidi files
also pass from an `#[ignore]`d test. Tailoring and L3 are TD-47. Shaping,
line boxes and text fragments are the next REN-03b units. See
[TEXT.md](TEXT.md).

## REN-03b (part 2) — Shaping, line boxes and text fragments

`rustic-text::shape` adds per-cluster font fallback, glyph advances, bidi
mirroring and letter/word spacing. `rustic-layout` gains inline formatting
contexts (`inline.rs`), line boxes (`lines.rs`) and public `LineFragment`,
`TextFragment` and `InlineFragment` results (`text.rs`) through the new
`BlockLayout::compute_with_fonts`. This covers white-space processing,
`<br>`/`<wbr>`, anonymous blocks, float avoidance by line, bidi reordering,
text-align/indent/justify, line-height and vertical-align, and text-based
shrink-to-fit. OpenType shaping, atomic inlines and the other gaps are TD-50.
See [LAYOUT.md](LAYOUT.md#inline-formatting-and-text-fragments-ren-03b) and
[TEXT.md](TEXT.md#shaping).
## REN-04b — Paint order, stacking contexts, overflow clips and opacity

The display list now follows CSS 2.2 Appendix E for the block reference:
stacking contexts from the root, positioned boxes with integer `z-index`,
fixed boxes and `opacity < 1`; negative, block, float, zero/auto and positive
layers; pseudo-contexts for floats and `z-index: auto` positioned boxes that
hand positioned descendants to the real context. Items carry the layout's
containing-block clip chain, and the rasterizer clips geometrically. Opacity
contexts are composited as groups (TD-46 for their memory shape). Steps
credited: `paint.display` order, `paint.stacking` contexts and clips, and
`paint.raster` clip/opacity layers. See [PAINT.md](PAINT.md).

## REN-05b — Bounded inflate and PNG decoding

`rustic-codecs` adds RFC 1950/1951 decompression and a PNG decoder for every
color type, bit depth, filter and Adam7, with CRC, chunk-order and exact-size
checks, limits applied before allocation, and cancellation. PngSuite images
match an independent decoder's references; a `codec-png` fuzz target covers
mutations. `paint.images` credits its PNG decode step; painting images waits
for replaced-element sizing. Shortcuts are TD-51. See [PAINT.md](PAINT.md).


## REN-05d — JPEG, GIF and WebP decoding and the image store

`rustic-codecs` adds bounded decoders for JPEG (baseline and progressive),
GIF (with animation) and WebP (lossless, lossy, alpha and animation), format
sniffing, header probing and `ImageStore`, which shares entries by URL,
decodes lazily within a memory budget with LRU eviction and keeps failures
sticky. 47 synthetic reference images match Pillow (JPEG within 3, GIF and
WebP exactly), three fuzz targets cover mutations, and two reftests show the
formats in `rustic render`. `paint.images` credits its formats and
lifecycle steps. Debt is TD-80 to TD-84. See [PAINT.md](PAINT.md).
## REN-07a — Reftest harness

`rustic reftest` renders a test and each `<link rel=match|mismatch>` reference
through the same pipeline and compares the surfaces (`rustic_raster::compare`),
honoring WPT-style `<meta name=fuzzy>` tolerances. References are confined to
the test's directory like linked stylesheets. A checked suite runs at scales 1
and 2 under `cargo test`. `paint.conformance` credits its reftest step;
deterministic clocks, allocation counters, reference chains and an imported WPT
reftest subset remain for later REN-07 work. See [PAINT.md](PAINT.md#reftests).

## REN-06a — Hit testing

The display list records every visible box's border rectangle and clip in
paint order, and `DisplayList::hit_test` finds the topmost one at a point;
`rustic hit-test` prints it. `paint.hit` credits its hit-testing step; resize
and zoom relayout remain. See [PAINT.md](PAINT.md#hit-testing).

## REN-04c — Text painting

The display list paints inline content after floats in each layer: inline
box backgrounds and borders, then one glyph path per text fragment, built
from `glyf` outlines and clipped by the block's own overflow. The rasterizer
flattens quadratic contours at device scale. `rustic` layout and paint
commands load the bundled test fonts, so `dump-layout`, `dump-paint`, `render`,
`reftest` and `hit-test` now accept text. Three text reftests run at scales 1
and 2. `paint.raster` credits glyph rasterization; gaps are TD-52. See
[PAINT.md](PAINT.md#text).

## REN-05c — Image painting

The display list paints decoded images for block-level replaced elements
(`Shape::Image`, bilinear sampling that copies exactly at one-to-one), and
the command line loads local PNG `img` sources under the stylesheet
confinement rules and image budgets, feeding natural sizes to layout. Image
reftests cover natural size, scaling, alpha and broken images.
`paint.raster` credits image rasterization; gaps are TD-53. See
[PAINT.md](PAINT.md#images).

## REN-04d — Text decorations

Underline, overline and line-through propagate from decorating boxes to the
in-flow text inside them and paint as rectangles under or over the glyphs,
with `solid` and `double` styles. Two decoration reftests and a propagation
test cover it; `paint.display` credits its content step, and outlines are a
new planned step. Approximations are in TD-52. See [PAINT.md](PAINT.md#text).

## REN-04e — Outlines

The outline longhands and shorthand parse and compute (width 0 when the style
is `none`), and each stacking context paints its boxes' outlines last, inside
its opacity group, as rings around the border box grown by `outline-offset`.
`solid`, `double` and `auto` (as solid) paint; other styles fail atomically.
Two reftests and a paint test cover order, negative offsets and failure;
`paint.display` credits its outline step. Gaps are TD-54. See
[PAINT.md](PAINT.md#outlines).

## REN-04f — Rounded corners

`border-radius` parses and computes, and paint rounds backgrounds, borders
(including mixed colors), outlines, overflow clips and image content, with
rounded clips chained along the containing-block chain and honored by hit
testing. Corner curves use constant tables so output stays identical on
every platform. Three reftests (exact clip and border matches and a
mismatch), a paint test for clip chains and hits, and unit tests for radius
scaling cover it. Adding the properties brought 34 more WPT parsing cases
into scope, all passing. Gaps are TD-55. See
[PAINT.md](PAINT.md#rounded-corners).

## REN-04g — Clip paths

`clip-path` parses and computes basic shapes and reference boxes, makes a
stacking context, and paints that context into a layer composited through
the shape's coverage; hit testing follows the shape for the box and its
descendants. Three exact reftests (circle against `border-radius: 50%`, a
rectangle polygon clipping an overflowing positioned child, and a rounded
inset in the padding box) and a paint test cover it. Writing them exposed a
rasterizer bug that predates this work: when an edge ended exactly on the
left of an item's pixel window, stepping could give x = -1e-16 and move the
edge's coverage one column over. Clamping fixes it, and a unit test covers
it. `paint.stacking` credits its rounded and clip-path step. Gaps are
TD-56. See [PAINT.md](PAINT.md#clip-paths).

## REN-04h — Transforms

Every transform property parses and computes, and transformed boxes paint
through plane homographies: paint keeps local coordinates inside a
transformed context and records its clips in the parent's coordinates, the
rasterizer projects shapes, clips and images with a stack of maps, and hit
testing inverts the chain. Perspective, singular matrices and hidden back
faces follow CSS Transforms 2. Trigonometry is plain arithmetic so matrices
match on every platform, and output without transforms is byte-identical to
before. Five exact reftests (translate against offsets, a 90° rotation, a
scale about the corner with a mirrored box, an overflow clip rotated 180°,
and a flipped box with a hidden back face) plus a paint test for 45°
coverage, outer clips, perspective, hits and degenerate cases cover it.
`paint.stacking` credits its transforms step. Gaps are TD-57. See
[PAINT.md](PAINT.md#transforms).

## REN-04i — Shadows, blending and filters

`box-shadow`, `mix-blend-mode`, `isolation` and `filter` parse, compute
and paint. Shadows are items over a side table, blurred deterministically
(exact Gaussian below two pixels, three box blurs above); blend modes and
filters ride on groups, with filters inside clip paths and blending
isolated by stacking contexts. Five exact reftests (offset and spread
shadows, inset shadows, `multiply` with and without isolation, color
filters with a drop shadow) and one fuzzy one (rounded spread, whose
corners differ only by coverage conflation), paint tests for shadow order
and blur, blend formulas and filter nesting, and unit tests for the blur
cover it. `box-shadow` brought 78 WPT parsing cases into scope; 50 pass and
28 fail only because named colors serialize as `rgb()`. `paint.raster`
credits its blend step. Gaps are TD-58. See
[PAINT.md](PAINT.md#shadows-blending-and-filters).

## REN-04j — Tiled raster, damage, resize and zoom

The rasterizer paints a fixed 256-pixel tile grid, each tile in a frame
grown by the list's filter reach, with fixed-point blur sums, so
`rasterize_region` gives exactly the full surface's pixels for any region.
`damage(old, new, scale)` lists the tiles whose pixels can change: fills
are keyed by resolved content and by the groups, clips and transforms
around them, and the fills outside the lists' common prefix and suffix
damage their device bounds, grown by shadow blur and enclosing filters.
`repaint` paints those tiles over the old surface. A paint test checks
that repainting equals a full raster for color, position, opacity,
transform, shadow, filtered-child, removal, filter and canvas changes at
two scales, with shadows and blurs crossing tile seams; `rustic reftest`
also repaints each reference into its test through damage and fails if
the pixels differ (the paint conformance gate), and the paint fuzz target
checks that an unchanged list reports no damage. Tiles skip fills and
whole layers whose bounds miss their frame; a paint test checks the culled
raster against `rasterize_reference` byte for byte (with and without
filters, at two scales, with shadows, transforms and text near seams) and
that culling keeps a sparse 1280×720 page within a work budget the
reference exceeds. The `frame_budget` example measured a one-box repaint
at 3.5 ms (scale 1) and 11 ms (scale 2) on the sandbox, so `paint.raster`
credits its performance step.

Atomic inlines from the M3 layout work now paint atomically after their
line in the inline phase instead of in the block phase; a paint test checks
they land above a later block and a float, after the line's text, with
their own content (TD-70's paint-order part is paid). See
[PAINT.md](PAINT.md#atomic-inlines).

`rustic_layout::View` (window size at 100% zoom, device pixel ratio, page
zoom from 25% to 500%) drives resize and zoom: styles recompute against its
viewport (media queries and viewport units), layout and paint rerun, and
rasterization uses ratio × zoom; `View::to_css` maps window device points
for hit testing. `rustic render` and `rustic reftest` take `--viewport` and
`--zoom`. A paint test checks reflow, paint size and hits across a resize
and a zoom, and a command-line test checks that a zoomed render equals the
pre-zoomed page in the smaller viewport, with a media query and `vw`
switching, and that a resize reflows. `paint.conformance` is complete and
`paint.hit` credits its resize step. Gaps are TD-59. See
[PAINT.md](PAINT.md#rasterizer-rustic-raster) and
[PAINT.md](PAINT.md#resize-and-zoom).

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

## JS-01 — ECMAScript lexer and parser

`rustic-js` parses scripts and modules from UTF-16 source into an AST with
source spans, reporting early errors, strict-mode rules and automatic
semicolon insertion during the parse; Annex B syntax is accepted in scripts
only. Cover grammars convert expressions to arrow parameters and patterns
without backtracking. Depth, measured stack use and cancellation bound every
parse, and left-nested chains build and drop iteratively.

Evidence: lexer and parser integration tests; a Test262 parse-phase gate
(22,956 of 23,123 `test/language` files at revision `48221d7c`, with the 167
RegExp-pattern failures committed as an exact expected list); and the 18th
fuzz target, `js-parse`, with a strict/sloppy differential. Identifier data is
generated from pinned Unicode 17 files. Nothing executes yet: values, heap and
interpreter are JS-02 to JS-04, and the Test262 runner with published results
is JS-07. See [JS.md](JS.md) and TD-88/89/90.

## JS-02 — Runtime data model

`rustic_js::runtime` implements the values, objects and environments the
interpreter will run on, designed for memory first (ADR 0015): values are 8
bytes, every heap item is a `u32` index into a typed arena charged to a byte
budget, Latin-1 strings use one byte per unit, and objects share shapes, so a
property costs one 8-byte slot. Ordinary, Array and String internal methods,
the §7.1/§7.2 conversions and comparisons, environment and Reference Records
and completion records follow ECMA-262. Tests check those rules and pin the
memory costs (72-byte empty object, 8 bytes per shared-shape property and per
dense element). BigInt conversions (TD-92) remain.

## JS-03 — Garbage collector (first part)

`rustic_js::runtime` now frees what script can no longer reach (ADR 0016): a
nonmoving, stop-the-world mark/sweep over the JS-02 arenas, with an explicit
worklist, side mark bits (no per-object header), free lists that reuse the
lowest index and return emptied arena tails, collection of unused shapes and
atoms, and the byte budget reset to the exact live footprint. Collection runs
at caller safe points with explicit `Roots`; hosts keep values alive through
generation-checked `HostRoot`s. Twelve tests and the 19th fuzz target,
`js-heap` (a shadow-model check), cover it. Weak references and DOM
host-object tracing, native-function safe points (TD-94) and incremental
marking (TD-95) remained after the first part.

## JS-03 — Weak references and DOM collection (second part)

Weak objects: the internal slots and operations of `WeakRef`, `WeakMap`,
`WeakSet` and `FinalizationRegistry`, with weak-map entries marked as
ephemerons to a fixpoint, dead targets cleared and held values queued for the
host. Host objects: an object created with `Agent::new_host_object` carries a
`u64` for the embedder, and `collect_garbage_with` asks a `HostTracer` for its
roots and each reachable host object's edges and reports collected ones.
`Heap::set_gc_stress` makes every safe point collect. The new
`rustic-bindings` crate uses these for the DOM: `DomHeap` keeps one wrapper per
node and node-held values, and collects a document and an agent together. The
DOC-09 scenarios, a seeded reachability model and a stress run pass against the
real heap, and the prototype `HostHeap` is removed (TD-17 narrowed, TD-96
paid). The constructors and prototype methods of the weak built-ins arrive with
JS-05.

## JS-04 — Bytecode compiler and interpreter (first part)

Scripts run. `rustic_js::vm` resolves scopes ahead of time (registers for
uncaptured bindings, indexed environment slots for captured ones), compiles
each function once to 12-byte register instructions kept in a collected arena,
and interprets them with frames on agent-owned stacks, so script recursion
never grows the Rust stack and ends in a catchable `RangeError` (ADR 0017).
Loop back edges and calls are safe points for cancellation, a deterministic
step budget and collection. Property and global sites have one-entry inline
caches for own, inherited and added properties, invalidated by a single epoch.
`Agent::evaluate` is the entry point. Twenty-two test functions run about 270
scripts, and the 20th fuzz target, `js-run`, compares cached, uncached and
collect-at-every-safe-point runs. Classes, `super`, private names, generators,
async functions, modules, regular expressions and tagged templates are not
compiled yet (TD-97);
see [JS.md](JS.md#bytecode-and-interpreter-js-04). A follow-up made function
`prototype` objects lazy (TD-98 paid) and scope environments share their name
lists: a closure costs 120 bytes instead of 200, and the environment for one
captured variable 64 instead of 112. Classes followed: constructors,
`extends`, `super`, methods and accessors, instance and static fields and
static blocks compile onto the same bytecode with a handful of new
instructions. Private names (`#x` fields, methods, accessors, statics and
`#x in o`) are symbols hidden from reflection: fields are ordinary shape
properties, and methods live once in the class scope behind a per-instance
brand check. Object-literal `super` (through the closure's home object),
tagged templates (one frozen template object per site, kept by the code) and
optional calls completed the JS-04 syntax.

## JS-07 — Test262 runner (first part)

`rustic_js::test262` now executes tests as well as parsing them: a fresh agent
per mode, the harness files and includes, strict and sloppy runs, negative
expectations by phase and error name, and a heap limit plus a step budget per
run so results do not depend on the machine. `examples/test262.rs` prints
counts per directory and a verdict per file. At the pinned revision 7,895 of
the 17,773 `test/language` files that run pass (44.4%), in 16 seconds, after
fixing the four interpreter bugs the first run found; the
passing files are committed as an exact list that the ignored gate test
compares. Modules, async files and `test/built-ins` are not run yet (TD-105).
See [JS.md](JS.md#test262-execution-js-07).

## JS-05 — Core built-ins (part 1)

The first library part installs `Error` and the native errors, `Object` with
its static methods and the `Object.prototype` methods, `Function.prototype`
(`call`, `apply`, `bind`, `toString`, `[Symbol.hasInstance]`), the global
functions and the `Boolean`, `Number`, `String` and `Symbol` constructors.
Native functions can now be constructors: `new` passes a new target, and they
work as `extends` bases and `super()` targets. Test262 `test/language` passes
went from 7,895 to 10,726 of 17,773 run files (60.4%). A new agent holds 83
objects in 17,390 bytes. See [JS.md](JS.md#built-ins-js-05).

Part 2 so far adds the `Function` constructor (parsed as one function
expression in the global scope), every `Array` method of ES2025, keys and
entries iterators with `%IteratorPrototype%`, `Math` with a seedable
`Math.random`, and the `Symbol` statics with a registry. Then the `String`
methods, `Number` statics and formatting (rounding the exact binary value),
and `JSON` with parsing on an explicit stack. Test262 `test/language` now
passes 12,404 of 17,773 run files (69.8%), and the gate also covers the
`test/built-ins` directories of these areas: 8,375 of 9,467 (88.5%). Then
`Map` and `Set` on an insertion-ordered table whose holes are squeezed out
when no iterator holds a position (the collector recounts open iterators),
the ES2025 set methods, `groupBy`, the weak collections, `WeakRef`,
`FinalizationRegistry` and `Reflect`: 12,441 `test/language` files and 9,439
of 10,565 `test/built-ins` files (89.3%). Then `Date` with integer
civil-date arithmetic, a parser that reads back every string a Date writes, a
fixed embedder-set local offset (no time zone database, TD-108) and an
embedder-fixable clock: 12,450 `test/language` files and 10,178 of 11,175
`test/built-ins` files (91.1%). Then the iterator helpers, `Iterator.from`
and `Iterator.concat`, each lazy helper a boxed state struct stepped by
`next` rather than a generator: with `test/built-ins/Iterator` gated too,
10,502 of 11,853 `test/built-ins` files (88.6%; the `Iterator` failures
need generators or test later proposals). Then direct and indirect `eval`:
a direct call compiles to `DirectEval`, the resolver keeps every binding
visible from it in environments and makes names dynamic in functions whose
sloppy eval may add `var`s, and EvalDeclarationInstantiation rejects vars
that would hoist past lexical bindings: 13,087 `test/language` files
(73.6%) and 10,534 `test/built-ins` files (88.9%).

## JS-08 part 1: generators

`function*` declarations, expressions and methods, `yield`, `yield*` and
`%GeneratorFunction%` run. A call instantiates the parameters, then
`GeneratorStart` moves the frame's registers into the generator object
(about 100 bytes plus 8 per register) and returns it; `next`, `throw` and
`return` copy them back onto the value stack and run the frame until the
next `yield`. Resumption modes are handled in compiled code after each
`yield`, so `return()` runs `finally` blocks through the ordinary exit path.
`yield*` is a bytecode loop around one `Delegate` op. Array destructuring now
closes its iterator when a target or default throws or a generator returns
from inside it (TD-100 paid). Test262:
16,150 of 17,773 `test/language` files (90.9%), and 10,802 of 11,937
`test/built-ins` files (90.5%) with `GeneratorFunction` and
`GeneratorPrototype` gated.

## JS-06 part 1: promises, jobs and async functions

`Promise` with `then`, `catch`, `finally`, `resolve`, `reject`, `try`,
`withResolvers` and the four combinators, `AggregateError`, and `async`
functions, arrows and methods run. Settled promises queue reaction jobs on
the agent; the embedder drains them with `Agent::run_jobs`. Internal
reactions (an `await`, or `then` on an intrinsic promise) allocate no
resolving function objects. An async function shares the generator
machinery: `await` saves the frame into its state object and the reaction
job resumes it. The Test262 runner now runs async tests through
`doneprintHandle.js`, which moved about 4,650 `test/language` files from
skipped to run: 16,723 of 22,427 run files pass (74.6%; most failures are
async generators and `for await`), and 11,487 of 12,713 `test/built-ins`
files (90.4%) with `Promise`, `AsyncFunction` and `AggregateError` gated.
Unhandled rejections are not reported to the host yet (TD-109).

## JS-08 part 2: async generators and `for await`

`async function*` in every form, `yield*` over async and sync iterables,
`for await`, and the async iteration intrinsics run. An async generator
keeps a queue of requests in its generator record; the driver runs the
body for the head request, settles it at each `yield` and serves the next
one without suspending, and suspends at `await` like an async function.
The spec's implicit awaits compile to ordinary `await` ops, so the
interpreter gained only five small ops (getting an async iterator, a
`yield*` call, the result check, the missing-`throw` error and the call
half of AsyncIteratorClose). Test262 `test/language`: 20,966 of 22,427 run
files (93.5%, from 16,723); `test/built-ins`: 11,599 of 12,826 (90.4%)
with the async iteration directories gated.

## JS-08 part 3: regular expressions

`rustic-js` has its own RegExp engine (`src/regexp/`): a parser for the whole
ES2025 pattern grammar with Annex B, `v`-mode set operations and modifier
groups; a compiler to a short instruction list; and a backtracking matcher
with explicit stacks that reads Latin-1 and UTF-16 strings in place. Case
folding closes classes once at compile time from the standard library's
case tables. The script parser validates every literal, which empties the
parse gate's expected failures (TD-88 paid). The library covers the
constructor, `RegExp.escape`, `exec`/`test`, the flag accessors, the five
`Symbol` methods with `d`-flag indices, and the string iterator; the String
methods use them. The matcher polls the step budget and cancellation, so a
runaway pattern stops like a runaway loop (TD-111). Unicode property
escapes parse but cannot run (TD-110). A 21st fuzz target, `js-regexp`,
compares Latin-1 and UTF-16 matching. Test262 `test/language`: 21,220 of
22,427 run files (94.6%); `test/built-ins` with RegExp gated: 13,175 of
14,722 (89.5%), RegExp alone 1,224 of 1,879, of which about 570 failures
are property escapes.

## JS-08 part 4: BigInt

`runtime/bigint.rs` adds the arithmetic of §6.1.6.2 to the sign-and-limbs
storage: addition, subtraction, schoolbook multiplication, Knuth division
with truncation toward zero, exponentiation by squaring, shifts that round
down, the bitwise operators on the two's complement form, `asIntN`/`asUintN`
wrapping and printing in any radix. `builtins/bigint.rs` adds `ToBigInt`,
`ToBigInt64`, `ToBigUint64`, `ToIndex`, the `BigInt` function and its
prototype. The interpreter routes numeric operators through one check that
both operands have the same numeric type. BigInt literals and literal
property keys compile. Results are capped at 2²⁰ bits, checked before
allocation (TD-112); TD-92 is paid. Test262 `test/language`: 21,313 of 22,427
run files (95.0%); `test/built-ins` with BigInt gated: 13,269 of 14,799
(89.6%).

## JS-08 part 5: Proxy and mapped arguments objects

Exotic objects now have one dispatch point: an object flagged exotic is
handed by the ordinary internal methods to `runtime/exotic.rs`. The five
internal methods a proxy can make throw (`[[GetPrototypeOf]]`,
`[[SetPrototypeOf]]`, `[[IsExtensible]]`, `[[PreventExtensions]]`,
`[[OwnPropertyKeys]]`) return `JsResult`. `builtins/proxy.rs` implements
`Proxy`, `Proxy.revocable` and the thirteen traps with the invariants of
§10.5; proxies for functions are function objects; nesting is bounded at 128
levels. Mapped arguments objects (§10.4.4) alias parameters that the
compiler moves into the function environment when a sloppy function with
simple parameters names `arguments`; `%ThrowTypeError%` and the restricted
`caller`/`arguments` accessors exist (TD-114). `for-in` over a proxy is
eager (TD-113); the arguments half of TD-99 is paid. Test262
`test/language`: 21,402 of 22,427 run files (95.4%); `test/built-ins` with
Proxy gated: 13,724 of 15,109 (90.8%), Proxy 273 of 310 and Reflect 153 of
153.

## JS-08 part 6: ArrayBuffer, DataView and typed arrays

`builtins/buffer.rs` holds `ArrayBuffer` (fixed and resizable, `slice`,
`resize`, `transfer`), `DataView` and the element codec; a buffer's bytes are
charged to the heap budget and capped at 2³¹ − 1. `builtins/typed_array.rs`
holds `%TypedArray%`, the twelve constructors (with `Float16Array`), every
prototype method and the integer-indexed exotic internal methods, dispatched
through `runtime/exotic.rs`. Elements are little-endian on every target.
The Test262 runner gained a `$262` with `global` and `detachArrayBuffer`.
`SharedArrayBuffer`, `Atomics` and element fast paths are TD-115. Test262
`test/language`: 21,465 of 22,427 run files (95.7%); `test/built-ins` with
the four buffer directories gated: 16,629 of 18,081 (91.9%).

## JS-08 part 7: SharedArrayBuffer and Atomics

`builtins/buffer.rs` adds `SharedArrayBuffer` (fixed and growable, `slice`,
`grow`) as the buffer record with a `shared` flag, and `builtins/atomics.rs`
the `Atomics` object with every operation, `wait`, `waitAsync` and `notify`.
Sharing is within one agent (TD-115): `wait` throws unless
`Agent::set_can_block`, then only times out; `waitAsync` waiters are woken by
`notify` or by `Agent::expire_waiters`. Test262 `test/language`: 21,467 of
22,427 run files (95.7%); `test/built-ins` with `Atomics` and
`SharedArrayBuffer` gated: 17,109 of 18,574 (92.1%).

## JS-06 part 2: promise rejection tracking

`RejectPromise` and `PerformPromiseThen` record the two
`HostPromiseRejectionTracker` operations as events on the agent.
`Agent::take_rejection_events` hands them to the host at a microtask
checkpoint; a `reject` whose promise is handled before that is withdrawn, as
HTML's about-to-be-notified list requires. Events are collector roots until
taken. TD-109 is paid; firing `unhandledrejection` is M5.

## JS-09: limits and performance gate

`tests/limits.rs` gates the frame, stack and heap limits, cancellation and
the step budget; the eight workloads of `benchmarks/` with pinned checksums
that do not depend on inline caches or on a collection at every safe point,
and whose work less than 2.5-folds when the input doubles; and the committed
heap-size report (`tests/fixtures/heap_report.txt`). `examples/js_bench.rs`
reports median and p95 time, peak heap, collections and pauses. Long
concatenations are ropes flattened on first read and by the collector
(TD-101 paid, TD-116 opened); the string index accessors no longer copy the
string; top-level `var` globals are cached by dictionary entry; and `for-in`
skips prototypes that contribute no keys. Recorded x86-64 results are in
[JS.md](JS.md#limits-and-performance-gate-js-09). Native ARM64 timings are
not recorded: the sandbox has no ARM64 host. Collection pauses pass the 5 ms
target up to about 30 MiB of live heap (TD-95, `perf.gc`).

## REN-03 completion: font formats, shaping and text policies

Reviewed HarfRust/Skrifa/Wuff adapters add CFF/CFF2 and WOFF1/2, GSUB/GPOS/kern,
Arabic/Indic shaping and glyph offsets in paint. Font discovery walks sorted
host-granted local roots under independent bounds and publishes accepted faces
atomically. `--font FILE` configures computed metrics and rendering explicitly;
there are no implicit system-font reads or network downloads.

CSS `unicode-bidi` scopes map virtual controls back to original source indexes;
HTML dir/bdi/bdo rules and paragraph plaintext alignment are integrated. CSS Text
3 line-break/word-break/overflow-wrap/hyphens control real line geometry and
intrinsic sizes. Soft hyphen glyphs are measured and painted only at the selected
break. Pinned English patterns require known `en`/`en-US`; DOM language survives
contents/subtree snapshots. Source UTF-16 text is unchanged.

See [TEXT.md](TEXT.md), [ADR 0014](adr/0014-reviewed-font-adapters.md) and
TD-85/86/87 for bounds, independent reference tests, exact dependency review and
remaining native services, break reshaping, carets and language resources.
Earlier REN-03a/03b descriptions above are historical slices; their former
OpenType/format/tailoring gaps are paid by this completion. Broad text standards
acceptance and native platform runtime gates remain separate M9/M8 work.

The table below is the status of the original M1–M2 queue items and is kept as
history; the [roadmap's current queue](ROADMAP.md#17-current-implementation-queue)
lists the next tasks.

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
| DOC-08: parser corpus/fuzzing | Complete: tokenizer corpora, the full runnable html5lib tree corpus (documents and fragments) at every split, exact limits, and parser fuzz targets (now twelve, including `paint-raster` and `text-sfnt`) with a required CI smoke job; coverage-guided fuzzing and sanitizers await dependency review |
| DOC-09: traced node/JS ownership prototype | Complete: tree marking and sweep in `rustic-dom`, collected with the JS heap through `rustic-bindings` (the prototype `HostHeap` is replaced), `check_invariants`, 25,230-case pre-insert conformance gate |
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
| CSS-08: declared CSS conformance gate | Complete: pinned WPT parsing cases for selectors and supported properties, 1,115/1,455 in scope pass, the rest listed with reasons in a ratchet; shorthand serialization, named-color keywords and `calc()` wrappers in specified values remain |
| TD-32: targeted `:has()` and state invalidation | Complete: anchors within the relative selectors' reach and their preceding siblings, subtrees under changed `lang`/`dir`/`disabled`/`contenteditable`, `dir=auto` roots and form-group controls; `css.invalidation` is complete. A pragma language change and states in nested `:has()` ancestor positions still restyle everything |
| JS-01: lexer, parser, early errors, strict mode, ASI and UTF-16 | Complete for parsing: scripts and modules to a spanned AST, 22,956/23,123 Test262 parse-phase files at a pinned revision (RegExp pattern early errors remain, TD-88; proposals skipped, TD-89), bounded depth/stack/cancellation and the `js-parse` fuzz target; see [JS.md](JS.md) |
| JS-02: values, coercions, references, completions, strings, shapes and object internal methods | Complete for the data model: 8-byte values, budgeted `u32` arenas, Latin-1 strings and atoms, shared shapes with dictionary fallback, dense/sparse elements, ordinary/Array/String internal methods, conversions and comparisons, environments and references; collection came with JS-03 and BigInt conversions with JS-08 part 4; see [JS.md](JS.md#runtime-data-model-js-02) and ADR 0015 |
| JS-04: bytecode compiler and interpreter | First part complete: scope resolution, register bytecode, functions, closures, calls/`new`, exceptions, iteration, limits, cancellation, inline caches and the `js-run` fuzz target; classes with private names, object-literal `super`, tagged templates and optional calls run; generators (JS-08 parts 1 and 2), async functions (JS-06 part 1) and regular expressions (JS-08 part 3) run; static modules run, with asynchronous module execution remaining (TD-97, TD-99..104); see [JS.md](JS.md#bytecode-and-interpreter-js-04) and ADR 0017 |
| JS-05: core built-ins | Part 1 complete: Error and native errors, `Object`, `Function.prototype` with bound functions, global functions and primitive wrapper constructors, native constructors usable with `new`, `extends` and `super`; part 2 so far adds the `Function` constructor, `Array`, `String`, `Number`, `Math`, `JSON`, `Map`, `Set`, the weak collections, `Reflect`, `Date`, the iterator helpers, `eval` and the `Symbol` statics; function source text remains (TD-106, TD-107); see [JS.md](JS.md#built-ins-js-05) |
| JS-07: Test262 runner | First part complete: execution with modes, negative tests and deterministic limits, exact passing list for `test/language` (20,966 of 22,427 run files with JS-05, generators, async functions and async generators so far, plus 11,599 of 12,826 `test/built-ins` files in the gated areas); modules and the remaining built-ins remain (TD-105); see [JS.md](JS.md#test262-execution-js-07) |
| JS-03: heap, roots and collection | Complete: nonmoving mark/sweep with side mark bits, free lists, collected shapes and atoms, exact budget return, generation-checked host roots, safe-point policy and stress mode, weak references with ephemerons, host-object tracing, the shared DOM/JS collection in `rustic-bindings` and the `js-heap` fuzz target; native safe points (TD-94) and incremental marking (TD-95) remain; see [JS.md](JS.md) |

M1's catalog scope is complete (28/28). DOC-04c added node kinds, cloning and
cross-document adoption; mutation records and observers are an M5 gate
(`dom.observers`). With DOC-06c and DOC-08 done,
`html.conformance` is complete and DOC-06d removed the last known tree
failures. M2 has CSS-01 through CSS-05 and CSS-07a (selectors) done and CSS-06
complete (media queries, invalidation, custom properties and container
queries), and CSS-08 closed the declared conformance gate with known failures
recorded rather than hidden. The M2 catalog scope is complete, including its
math-function and invalidation steps; broader CSS compatibility remains open.
M3's declared rendering scope is complete (REN sections above, with open debt in
the ledger). M4 has started with JS-01, the ECMAScript lexer and parser. All
project crates remain safe Rust and unpublished; the only third-party Rust
crates are the reviewed font adapters of ADR 0014.

## JS-06 — Static module records and graph resolution

The Module parser now feeds a static record and a host-loaded dependency graph.
Resolution preserves binding identity through re-exports, distinguishes missing
from ambiguous exports and terminates on cycles. The loader checks graph size,
total source length, resolution work and cancellation. The host supplies source
and decides request identity and import attribute policy. See [JS.md](JS.md#static-module-loading-js-06).
## JS-06 — Synchronous module evaluation

`Agent::evaluate_module_graph` compiles each record into an instantiation
entry point and an evaluation body. A traced module environment holds
declarations and direct live imports. The graph initializes functions and
`var` before running bodies in depth-first dependency order. Namespace
exotic objects expose the resolved exports, including later binding updates.
The focused regressions check cycles, lexical TDZ, namespace identity and
property semantics, garbage collection, and rejection before script side
effects when top-level await cannot compile. Asynchronous evaluation,
dynamic imports, `import.meta` and the module conformance gate remain in
TD-97 and TD-105.

## JS-07 — Static module Test262 baseline

The standalone evaluator creates a lazy per-module `import.meta` object.
The Test262 runner loads relative source text fixtures from the checkout and
records module verdicts separately from the existing script baselines.
The pinned `test/language` module selection has 382 passes, 314 failures
and 121 proposal skips, with zero engine panics reported. The failures cover
top-level await, dynamic import and fixtures with import attributes.
See [JS.md](JS.md#static-module-loading-js-06) and TD-97/105.

## JS-08 — Unicode string normalization

`String.prototype.normalize` implements all four Unicode forms over
UTF-16 code units. It preserves lone surrogates, checks coercion order and
invalid forms, and limits the decomposition workspace. The original IDNA
NFC implementation and canonical Unicode data moved to `rustic-base` so
both consumers share them. The pinned Test262 normalization selection
passes all 14 files. The heap report records a 193-byte increase for the
new method and its property name. Locale support remains in TD-107/108.

## JS-07 — Test262 script evaluation in the current global environment

`$262.evalScript` executes UTF-16 source through the regular script compiler
and returns its completion value or throws its exception. It uses the global
environment and the active run's limits. Global declaration instantiation
checks all lexical, function and `var` names before creating bindings, so
a failed declaration leaves the existing environment intact. Function
declarations install the winning closure with the specified property
attributes, including replacement of configurable accessors. See
[JS.md](JS.md#test262-execution-js-07) and TD-105 for the pinned results and
remaining hooks. The language gate adds nine passing cases, reaching 21,476
of 22,427 executed files. Built-in and module verdicts are unchanged.

## JS-06 — Host-provided module metadata

The standalone graph evaluator can accept UTF-16 string properties keyed by
the host's module identity. Each module that references `import.meta` sees its
own metadata object, shared with nested functions and retained by the traced
module environment. Each supplied property consumes graph work and heap
storage. The default evaluator retains its lazy empty object. See
[JS.md](JS.md#static-module-loading-js-06).

## JS-06 — Top-level await in static graphs

The asynchronous module entry point links the complete graph before execution.
It tracks cycles and pending dependencies with iterative traversal and resumes
module bodies through the existing promise queue. The pinned module gate gains
241 passes, reaching 623 of 696 executed files with 121 proposal skips.
Dynamic import and import-attribute fixtures remain open under TD-97/105.

## JS-06 — Synthetic JSON and text modules

Explicit host loaders can supply JSON and text data as default-only modules.
JSON is parsed before evaluation and shared by all importers of the same key.
Text preserves its UTF-16 units. The pinned module gate gains 24 passes from
data fixtures and rejected attribute requests, reaching 647 of 696 executed
files. Byte imports need immutable ArrayBuffer support and remain in TD-105.

## JS-06 — Host-driven dynamic imports

The compiler emits an import instruction that converts the request and queues
an explicit host job. Module referrers are retained with live environments,
and the import promise waits for the loaded module's completion promise.
The Test262 fixture host reuses pending and evaluated instances across graphs.
The pinned gates gain 44 module passes and 444 script passes. The five
remaining module failures require byte fixtures with immutable buffers.

## JS-06 — Byte modules

Byte sources export a Uint8Array backed by immutable storage. Aliases cannot
write or detach the buffer through indexed properties, typed array methods,
DataView, Atomics or transfer. The ArrayBuffer immutable accessor increases
the standard-library heap report by 286 bytes. The module gate passes all 696
executed files, with 121 proposal skips. The immutable accessor also adds
four passing built-in cases.
