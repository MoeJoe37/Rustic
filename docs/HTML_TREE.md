# HTML document construction

`HtmlDocumentParser` in `rustic-html` turns UTF-8 bytes into an owned DOM using
the WHATWG tree-construction algorithm for complete documents. It uses Rustic's
original tokenizer and the checked `rustic-dom` mutation API. No existing engine
or JavaScript VM is embedded. This is an inert local document parser with
scripting disabled; it is not a browser, fragment parser or script host.

## Implemented rules

| Area | Current behavior |
| --- | --- |
| Insertion modes | All 21 document modes: initial through after-head, in body, text, in table/table text/caption/column group/table body/row/cell, in template, after body, in frameset, after frameset and both after-after modes |
| Document shell | Explicit or implied HTML/head/body elements, late head elements re-entering the head, doctype retention and no-quirks/limited-quirks/quirks selection |
| Body rules | Block, heading, list, definition, form, button, pre/listing (leading newline skipped), plaintext, ruby, void, `image` renaming and "any other" start/end tag rules with special-element and scope checks |
| Formatting | List of active formatting elements with markers and the three-identical-entries limit, reconstruction, and the adoption agency algorithm including `a`/`nobr` start-tag recovery |
| Tables | Table context clearing, implied `tbody`/`tr`/`colgroup`, pending table character tokens and foster parenting of misplaced text and elements |
| Select | The current customizable-select rules: `select`, `option`, `optgroup`, `hr` and `input` recovery in body; there is no separate in-select mode |
| Templates | Template contents fragments, the template insertion-mode stack, and template-aware form/body/html handling |
| Framesets | `frameset-ok` tracking, body replacement and frameset/noframes modes |
| Foreign content | SVG and MathML namespaces, adjusted SVG tag/attribute case, MathML `definitionURL`, xlink/xml/xmlns attribute namespaces, integration points, breakout tags and allowed CDATA sections |
| Text modes | RCDATA (title, textarea), RAWTEXT (style, xmp, iframe, noembed, noframes), script data and plaintext, selected at the start-tag delivery boundary |

The tree builder tells the tokenizer after every token whether CDATA sections are
allowed (the adjusted current node is not HTML). Markup flushes earlier text, so
the tree builder has processed all prior tokens before that decision is needed.

Character tokens arrive grouped; the builder splits each group into homogeneous
whitespace, NUL and other runs so every insertion mode applies its
per-character rules. Adjacent text coalesces into one node, including at
foster-parented locations.

## Boundaries

Scripting is always disabled: `noscript` uses the scripting-disabled rules and
script end tags never prepare, run or block. There is no network, encoding sniff
or restart, fragment parsing algorithm, custom-element construction, form-owner
association, `selectedcontent` mirroring, shadow tree, sanitizer, event dispatch
or rendering. Processing-instruction tokens still return
`Unsupported(ProcessingInstructions)` because the DOM has no instruction nodes.

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

Exceeding a budget is fatal: a document deeper than `max_depth`, or with more
recovery diagnostics than `max_tree_diagnostics`, fails rather than being
truncated. Callers parsing untrusted pages should size those limits explicitly.

The parser owns all construction state and exposes no partial DOM. `feed` pumps
available tokens, pausing to apply mode changes before more tokenization.
`finish` closes input and returns `ParsedDocument` only after successful EOF.
The result exposes document/mode/diagnostic views; `into_document` transfers
ownership for later checked mutations ([ADR 0009](adr/0009-parser-owned-construction.md)).

Resource, cancellation and unsupported-feature errors discard the partial tree
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
Of 1,792 records, **1,584 are document records** without scripting or processing
instructions. The runner passes **1,580** of them: complete trees, namespaces,
sorted attributes and template contents match the unchanged expectations for the
unbroken input, **all 70,993 two-chunk byte splits** and one-byte feeds. The four
known failures are `selectedcontent` mirroring records, listed with that reason.
The 208 skipped records need the fragment algorithm (192), scripting (8) or
instruction nodes (8). Parse-error IDs/positions and compatibility-mode metadata
are not compared. This is not a WPT conformance result.

Focused Rust tests additionally cover template contents ownership and deep
nesting, CDATA permission, foreign name/attribute adjustment, foster parenting,
adoption, framesets, newline skipping, mode selection, attribute merge order,
exact budgets, cancellation, processing-instruction rejection and lifecycle.

```sh
cargo test --locked -p rustic-html --test document --test tree_construction --test tree_rules
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
