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
not JS UTF-16 indices or persistent source-revision handles.

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
not a conformance claim: public DOM APIs, fragments, processing instructions,
shadow trees, cross-document adoption, cloning, mutation records/observers and
script bindings remain unimplemented. Internal parsed names do not perform the
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
and reference parsing, ASCII hosts, IPv4/IPv6, encoding/serialization and structured
origins. It depends only on `rustic-base`. File/blob URLs and Unicode IDNA return
explicit unsupported errors. Limits and cooperative cancellation are integrated;
the supplied base is immutable even on failure. Debug/error output omits userinfo
and private URL payloads.

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

## DOC-07 — Bounded local loader and `dump-dom`

`rustic dump-dom [--max-input-bytes N] [--max-output-bytes N] [--chunk-bytes N] [--] <file>`
loads exactly one local file granted on the command line and prints the
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
64 MiB. Cancellation is checked between reads. Any failure prints nothing on
stdout: usage/URL errors exit 2, load/parse/budget failures exit 1. Unsupported
constructs from the DOC-06 subset fail explicitly rather than dumping a partial
tree. No network, script, subresource, encoding sniffing or base-URL resolution occurs.

Nine loader unit tests cover URL/path classification, exact file and dump byte
limits, chunk-size invariance at every size up to the input length, parser
budgets, unsupported markup, directories, `/dev/zero`, missing files, zero limits
and cancellation. Seven CLI tests compare a malformed fixture against a checked
snapshot for 19 chunk sizes and check URL rejection, usage errors, exact budgets
and clean failures. `html.loader` is complete for this scope; tree coverage
still grows with DOC-06.

## Queue status

The README now includes an automatically regenerated engine progress map. Its
21-subsystem, 172-capability engine-v2 catalog includes unfinished libraries, all five native
platform acceptance targets and final release/security/conformance gates. Source
and test references are validated; score calculations and safe Git publication
have eight Python regression tests. See [PROGRESS.md](PROGRESS.md) for definitions
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
MOB-01 configures four shared-library cross-checks; see [MOBILE.md](MOBILE.md) and
[ADR 0006](adr/0006-desktop-and-mobile-platforms.md). Native mobile hosts,
link/package checks, runtime tests and device support remain planned.

| First implementation queue item | Status |
| --- | --- |
| DOC-01a: typed document/node IDs and generation validation | Complete through the DOM storage layer |
| DOC-01b: explicit budgets and cooperative cancellation | Complete primitives; integrated into URL parsing and initial HTML tokenization |
| DOC-03: minimal document arena and node kinds | Complete internal subset described above |
| DOC-04a: mutation and deterministic tree inspection | Complete initial same-document subset |
| DOC-04b: character-data mutation primitives | Complete checked text/comment storage edits; live ranges, observers and bindings pending |
| DOC-02a: URL records and tuple/opaque origins | Complete initial declared subset; IDNA/file/blob and broader DOC-02 remain pending |
| DOC-05a: incremental tokenizer data/tag/comment states | Complete initial scope, with doctype and HTML-context declaration recovery |
| DOC-05b: references and alternate text modes | Reference/text modes complete; broader encoding cases remain pending |
| DOC-06a: initial/head/body tree construction | Partial: shell/head rules and a declared small body set; broader body algorithms pending |
| DOC-06b: tables/formatting/template/foreign tree rules | Not started |
| DOC-07: bounded HTML loader/CLI | Complete: local-only `dump-dom` with file, parser and output budgets; tree coverage follows DOC-06 |
| DOC-08: parser corpus/fuzzing | Partial: tokenizer corpora and 45 pinned tree records with chunk splits; broader tree corpus and fuzz targets pending |
| DOC-09: traced node/JS ownership prototype | Not started |

M1 remains in progress. Cross-document adoption, node cloning and mutation records
are still needed for the broader DOC-04 task. All project crates remain safe
Rust, unpublished, and free of third-party Rust dependencies.
