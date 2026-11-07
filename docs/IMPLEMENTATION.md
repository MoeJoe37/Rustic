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
text and an SVG-namespaced node. There is no parser involved, and no `dump-dom`
HTML command is advertised yet.

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

`html.references` is complete for its algorithmic scope; DOC-05b still needs
RCDATA/RAWTEXT/script/plaintext modes and broader encoding work. Foreign-content
CDATA, tree construction and loading remain pending. No full-parser claim is made.

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
| DOC-02a: URL records and tuple/opaque origins | Complete initial declared subset; IDNA/file/blob and broader DOC-02 remain pending |
| DOC-05a: incremental tokenizer data/tag/comment states | Complete initial scope, with doctype and HTML-context declaration recovery |
| DOC-05b: references and alternate text modes | Partial: named/numeric references complete; text modes and broader encoding cases pending |
| DOC-06a/b: tree builder | Not started |
| DOC-07: bounded HTML loader/CLI | Not started |
| DOC-08: parser corpus/fuzzing | Partial: pinned declaration corpus and chunk splits; tree corpus and fuzz targets pending |
| DOC-09: traced node/JS ownership prototype | Not started |

M1 remains in progress. Cross-document adoption, cloning and mutation records
are still needed for the broader DOC-04 task. All project crates remain safe
Rust, unpublished, and free of third-party Rust dependencies.
