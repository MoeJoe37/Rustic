# Initial HTML document construction

`HtmlDocumentParser` in `rustic-html` now turns UTF-8 bytes into an owned DOM for
an explicit initial/head/body subset. It uses Rustic's original tokenizer and
the checked `rustic-dom` mutation API. No existing engine or JavaScript VM is
embedded. This is an inert local document parser, not a browser or complete HTML
parser. The broader DOC-06 task and `html.tree` capability remain in progress.

## Implemented rules and boundaries

| Area | Current behavior |
| --- | --- |
| Initialization | Initial, before-html, before-head, in-head, in-head-noscript, after-head, text, in-body, after-body and after-after-body modes within the declared element set |
| Document shell | Explicit or implied HTML/head/body elements, including reprocessing EOF through omitted nodes |
| Doctypes | Retained name/public/system strings; no-quirks, limited-quirks or quirks selection; missing/late-doctype recovery |
| Head elements | `base`, `basefont`, `bgsound`, `link`, `meta`, `title`, `style`, `noframes`, `script`, and scripting-disabled head `noscript` |
| Text modes | RCDATA for title, RAWTEXT for style/noframes, script data for script; mode feedback before the next token, including the same submitted chunk |
| Body elements | `p`, `div`, `span`, `br`, `hr`, `plaintext`; supported head-family elements also follow their head rules when encountered in body |
| Recovery | Paragraph closure before another paragraph/div/hr/plaintext, implied empty paragraph for unmatched `</p>`, supported div/span end-tag recovery and `</br>` recovery |
| Attributes | Ordered first values from tokenizer; repeated html/body tags add missing attributes without overwriting existing values; HTML attributes remain unnamespaced |
| Characters/comments | Placement follows insertion mode; grouped characters split at significant leading whitespace; adjacent text coalesces, preserving comment boundaries; body NUL is ignored with a recovery |
| Self-closing syntax | Void tags acknowledge the flag; non-void or ignored tags diagnose an unacknowledged flag and do not implicitly close an element |
| Completion | EOF recovers unclosed text elements and returns an owned document with separate bounded tokenizer/tree diagnostics |

All constructed elements use the HTML namespace. Within the supported body set,
no button, table, template or foreign scope boundary can occur above a paragraph;
scope predicates must expand with the element set. A text element temporarily
opened after head retains the actual head as its DOM parent even though head is
no longer on the retained open-element stack. Depth checks use the DOM path.

An unsupported body start tag returns `Unsupported(BodyElement)`. Templates,
framesets and processing-instruction nodes have distinct unsupported errors.
Examples include formatting elements, tables, lists, headings, forms/controls,
textarea, SVG/MathML and custom/unknown elements. Standard ignored end-tag
recovery within the supported tree is still applied. Unsupported start tags are
not silently turned into generic elements or dropped as successful parsing.

Scripting is always disabled. Script elements retain text without execution,
microtask checkpoints, preparation/loading or parser reentry. Links, base URLs,
stylesheets, meta declarations and other attributes remain stored data. Input
encoding is explicitly UTF-8 and already fixed; meta charset declarations do not
change it. There is no network, script execution, encoding sniff/restart,
fragment parser, template/formatting/table machinery, foreign-content CDATA,
custom-element construction, sanitizer, shadow tree, event dispatch or rendering.

## Ownership and failure

`ParserLimits` contains the existing tokenizer limits, `DocumentLimits` and a
separate tree-diagnostic limit (default 1,024). The document defaults remain
100,000 live nodes including its root, 256 parent-child edges and 16 MiB of
retained name/UTF-16 payload. Implied nodes, attributes, comments and coalesced
text consume the same budgets as caller-created nodes. Open-stack entries are
bounded by checked node/depth insertion; no content-controlled recursion is used.

The parser owns all construction state and exposes no partial DOM. `feed` pumps
available tokens, pausing to apply mode changes before more tokenization.
`finish` closes input and returns `ParsedDocument` only after successful EOF.
The result exposes document/mode/diagnostic views; `into_document` transfers
ownership for later checked mutations. The script-disabled construction boundary
is recorded in [ADR 0009](adr/0009-parser-owned-construction.md).

Resource, cancellation and unsupported-feature errors discard the partial tree
and poison the stream; subsequent calls return `Poisoned`. A successfully finished
parser returns `AlreadyFinished` for later feeds/finishes. A feed is not a
rollback transaction over tokens; failed private state is discarded instead.
Cancellation is cooperative through the tokenizer between processing steps and
tokens; there is no background worker or preemption. Logical payload and token
budgets exclude allocator capacity, overhead and caller buffers and do not bound
total process memory or guarantee recovery from all allocation failures.

Tokenizer diagnostics retain normalized scalar offsets. Tree diagnostics use a
separate zero-based delivered-token index, including EOF. Reprocessed tokens keep
their index; grouped text may produce several tree recoveries at that index.
Indexes depend on configured token grouping and are not byte/UTF-16 offsets.
Tree categories are internal recovery labels, not WHATWG parse-error IDs.

## Standards and reference data

The implementation follows the supported portions of WHATWG HTML
[tree construction](https://html.spec.whatwg.org/multipage/parsing.html#tree-construction),
especially the initial/head/body/text insertion modes and character insertion.
Reviewed 2026-11-08 against the living document last updated 2026-11-06. The
compatibility identifier table is standards data with the retained
[WHATWG notice](../crates/rustic-html/data/WHATWG-LICENSE.md). Layout does not yet
use the selected document mode.

A conservative corpus selection checks **45 of 112 records** from html5lib
`tree-construction/tests1.dat` at revision
`9329e64694e7835d0dcff9811e22856ef6ad16f9`. This earlier revision retains the tree
data, which upstream subsequently moved to WPT. The complete source file and
[selection manifest](../crates/rustic-html/tests/fixtures/tree_manifest.json) are
vendored with the existing MIT notice. The generator's fixed element whitelist
and fragment/scripting exclusions do not depend on runtime pass/fail results.
The lexical filter is conservative and can exclude markup-like text literals.

The runner compares complete trees and sorted attributes against the unchanged
upstream expectations. It runs **905 two-chunk byte splits** and **45 one-byte
feeds**, plus the unbroken path. It adapts Unicode fixture input using a transport
BOM, preserving any intentional leading U+FEFF. It does not compare upstream
parse-error IDs/positions or compatibility-mode metadata. Excluded records and
all other corpus files remain unrun; this is not a full tree/WPT conformance result.

Sixteen focused Rust tests additionally cover mode selection, attribute merge
order, malformed structures, whitespace/comment placement, UTF-8 recovery,
same-chunk mode feedback, multiple token sizes, exact budgets, cancellation,
unsupported features and lifecycle. The existing tokenizer/reference suites
remain unchanged. Four Python tests validate corpus selection, record framing,
escaping through the shared literal writer and reproducible provenance; source
and generated fixture integrity are checked in ordinary CI.

```sh
cargo test --locked -p rustic-html --test document --test tree_corpus
cargo run --locked -p rustic-html --example document
python3 tools/import_html_tree_tests.py --check
```

Omit `--check` only for a deliberate source/selection/expectation update. The
example parses a fixed local fixture.

## Local `dump-dom` command

DOC-07 exposes this parser through a bounded command-line loader:

```sh
cargo run --locked -p rustic-cli -- dump-dom crates/rustic-cli/tests/fixtures/malformed.html
```

The command accepts one local path (never a URL), reads it in chunks within a
file byte limit that also bounds the tokenizer, uses the default parser limits
and caps dump output. Its output is the same diagnostic dump as above, so the
supported/unsupported element set is exactly this document's. See
[IMPLEMENTATION.md](IMPLEMENTATION.md) for options, exit codes and tests.
