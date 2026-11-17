# HTML document construction

`HtmlDocumentParser` in `rustic-html` turns UTF-8 bytes into an owned DOM using
the WHATWG tree-construction algorithm for complete documents, and
`HtmlFragmentParser` runs the fragment parsing algorithm for a context element
(as `innerHTML` would). Both use Rustic's original tokenizer and the checked
`rustic-dom` mutation API. Both default to scripting disabled. A caller can
opt into scripting-enabled tree construction.

## Implemented rules

| Area | Current behavior |
| --- | --- |
| Insertion modes | All 21 document modes: initial through after-head, in body, text, in table/table text/caption/column group/table body/row/cell, in template, after body, in frameset, after frameset and both after-after modes |
| Document shell | Explicit or implied HTML/head/body elements, late head elements re-entering the head, doctype retention and no-quirks/limited-quirks/quirks selection |
| Body rules | Block, heading, list, definition, form, button, pre/listing (leading newline skipped), plaintext, ruby, void, `image` renaming and "any other" start/end tag rules with special-element and scope checks |
| Formatting | List of active formatting elements with markers and the three-identical-entries limit, reconstruction, and the adoption agency algorithm including `a`/`nobr` start-tag recovery |
| Tables | Table context clearing, implied `tbody`/`tr`/`colgroup`, pending table character tokens and foster parenting of misplaced text and elements |
| Select | The current customizable-select rules: `select`, `option`, `optgroup`, `hr` and `input` recovery in body; there is no separate in-select mode. Option selectedness (last `selected`, else first non-disabled option) and `selectedcontent` mirroring: when a selected option is popped, including by stop-parsing at EOF, its children are deep-cloned into the select's enabled `selectedcontent` |
| Templates | Template contents fragments, the template insertion-mode stack, and template-aware form/body/html handling |
| Framesets | `frameset-ok` tracking, body replacement and frameset/noframes modes |
| Foreign content | SVG and MathML namespaces, adjusted SVG tag/attribute case, MathML `definitionURL`, xlink/xml/xmlns attribute namespaces, integration points, breakout tags and allowed CDATA sections |
| Processing instructions | Inert DOM nodes with case-preserved targets and data, using the insertion mode's document, element or template placement |
| Text modes | RCDATA (title, textarea), RAWTEXT (style, xmp, iframe, noembed, noframes), script data and plaintext, selected at the start-tag delivery boundary |

## Fragment parsing

`HtmlFragmentParser::new(&FragmentContext, ParserLimits, CancellationToken)` follows
WHATWG [parsing HTML fragments](https://html.spec.whatwg.org/multipage/parsing.html#parsing-html-fragments).
`FragmentContext::html`, `::svg` and `::mathml` name the context element, with
optional attributes (`encoding` matters for `annotation-xml`). The context is
standalone: it has no ancestors, so there is no ancestor form, and the fragment
is parsed in no-quirks mode.

The parser creates a scratch document with a root `html` element and a detached
context element. The context selects the initial tokenizer state (RCDATA for
title/textarea, RAWTEXT for style/xmp/iframe/noembed/noframes, script data,
plaintext, and RAWTEXT for `noscript` when scripting is enabled) with no last
start tag. A `template` context pushes "in template"; a `form` context becomes
the form element pointer. The insertion mode is reset using the context in place
of the root, and the adjusted current node (the context while only the root is
open) drives the HTML/foreign dispatch, CDATA permission (including before the
first token) and foreign element namespaces. Fragment-case rules ignore `</html>`
after body, keep the root after `</frameset>`, and ignore `input`/`select` start
tags in a `select` context. Foreign breakout reprocesses with the insertion mode's
HTML rules, as the standard says, rather than through the dispatcher.

`finish` destroys the context element and returns `ParsedFragment`, whose
`root()` is the scratch `html` element: its children are the fragment. Budgets,
diagnostics, lifecycle and failure atomicity match the document parser; the
detached context and root count toward the node limit while parsing. Input is
UTF-8 bytes; DOM-string/UTF-16 fragment input remains separate future work.

## Tokenizer feedback

The tree builder tells the tokenizer after every token whether CDATA sections are
allowed (the adjusted current node is not HTML). Markup flushes earlier text, so
the tree builder has processed all prior tokens before that decision is needed.

Character tokens arrive grouped; the builder splits each group into homogeneous
whitespace, NUL and other runs so every insertion mode applies its
per-character rules. Adjacent text coalesces into one node, including at
foster-parented locations.

## Boundaries

With `Scripting::Enabled`, `noscript` uses RAWTEXT in head and body. The
document pull interface pauses after a closed HTML `script` outside template
content. Script classification, execution and subresource loading belong to
the engine. The opt-in engine load runs inline classics on the partial tree
and traces parser-held nodes during collection. Fragment scripts remain inert.
[ADR 0038](adr/0038-parser-inserted-inline-classics.md)

Input is fixed UTF-8.

Template contents are a separate `DocumentFragment` root owned by the template
element ([ADR 0010](adr/0010-template-contents.md)). They are not children,
`Document::walk` does not visit them, and `dump_tree` prints them after the
template's attributes as `#template-contents`.

## Ownership and failure

`ParserLimits` contains the tokenizer limits, `DocumentLimits` and a separate
tree-diagnostic limit (default 1,024). The document defaults remain 100,000 live
nodes including its root, 256 parent-child edges and 16 MiB of retained
name/UTF-16 payload. Implied, reconstructed and adoption-agency clones,
attributes, comments and coalesced text consume the same budgets as
caller-created nodes. Template contents are separate roots, so their nesting is
bounded by node and token budgets rather than edge depth; construction,
completion, dumps and destruction are iterative.

Exceeding a resource budget is fatal: a document deeper than `max_depth` fails
rather than being truncated ([TD-02](TECH_DEBT.md#td-02--depth-and-node-budgets-abort-the-whole-parse)).
Diagnostic limits only bound what is retained: parse errors never stop parsing,
and recoveries past `max_tree_diagnostics` or the tokenizer's `max_diagnostics`
are counted by `dropped_diagnostics()` on the parsed document or fragment. The
tree is identical whatever the retention limits are.

The parser owns all construction state. `feed` pumps available tokens.
`write` queues bytes and `advance_to_script` returns the script element and a
read-only view of the tree before the parser consumes the next token.
`close_input` lets a pull caller drain outstanding boundaries before
`finish` returns `ParsedDocument` after successful EOF.
The result exposes document/mode/diagnostic views; `into_document` transfers
ownership for later checked mutations.
[ADR 0009](adr/0009-parser-owned-construction.md), [ADR 0037](adr/0037-parser-script-boundary.md)

Resource and cancellation errors discard the partial tree
and poison the stream; subsequent calls return `Poisoned`. A successfully finished
parser returns `AlreadyFinished` for later feeds/finishes. Cancellation is
cooperative through the tokenizer between processing steps and tokens.

Tokenizer diagnostics retain normalized scalar offsets. Tree diagnostics use a
separate zero-based delivered-token index, including EOF. Reprocessed tokens keep
their index; grouped text may produce several tree recoveries at that index.
Tree categories are internal recovery labels, not WHATWG parse-error IDs.

## Standards and reference data

The implementation follows WHATWG HTML
[tree construction](https://html.spec.whatwg.org/multipage/parsing.html#tree-construction),
reviewed 2026-11-08 against the living document last updated 2026-11-06. The
compatibility identifier and SVG/MathML adjustment tables are standards data with
the retained [WHATWG notice](../crates/rustic-html/data/WHATWG-LICENSE.md). Where
the corpus and a reading of the standard disagreed, the corpus decided: `select`
is not a special element for the adoption agency, and `keygen` does not close an
open `select`.

All 57 top-level html5lib `tree-construction/*.dat` files at revision
`9329e64694e7835d0dcff9811e22856ef6ad16f9` are vendored with the existing MIT
notice and a per-file digest [manifest](../crates/rustic-html/tests/fixtures/tree/manifest.json).
The runner passes **1,792 of 1,792 records**, including all 192 fragment
records and all eight marked scripting-on. Complete trees, namespaces,
sorted attributes and template contents match the pinned expectations in
**75,471 two-chunk byte splits** and one-byte feeds. The pinned files are
unchanged. Four exact expectations in the runner account for changes in
[WHATWG HTML processing-instruction tokenization](https://html.spec.whatwg.org/multipage/parsing.html#processing-instruction-open-state).
Three now produce instruction nodes and one discards an unfinished instruction.

Focused Rust tests additionally cover template contents ownership and deep
nesting, CDATA permission, foreign name/attribute adjustment, foster parenting,
adoption, framesets, newline skipping, mode selection, attribute merge order,
exact budgets, cancellation, processing-instruction placement and lifecycle.
Eight fragment tests cover context tokenizer states, the absent context node,
table/template/select/form contexts, foreign contexts and integration points,
CDATA before the first token, every split, budgets, cancellation and lifecycle.
The `html-fragment` fuzz target picks one of 28 contexts per input.

```sh
cargo test --locked -p rustic-html --test document --test tree_construction --test tree_rules --test fragment
cargo run --locked -p rustic-html --example document
python3 tools/import_html_tree_tests.py --check
```

Use `--vendor <html5lib-tests checkout>` only for a deliberate corpus update at a
reviewed revision. A user-facing bounded `dump-dom` loader/CLI is DOC-07 work.

## Local `dump-dom` command

DOC-07 exposes this parser through a bounded command-line loader:

```sh
cargo run --locked -p rustic-cli -- dump-dom crates/rustic-cli/tests/fixtures/malformed.html
```

The command accepts one local path (never a URL), reads it in chunks within a
file byte limit that also bounds the tokenizer, uses the default parser limits
and caps dump output. Its output is the same diagnostic dump as above, so its
coverage is exactly this document's. See
[IMPLEMENTATION.md](IMPLEMENTATION.md) for options, exit codes and tests.
