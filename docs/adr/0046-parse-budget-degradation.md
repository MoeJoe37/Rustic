# ADR 0046 — Parse budgets degrade instead of failing

Date: 2026-11-18
Status: Accepted for TD-02 (M6 entry)

## Context

`DocumentLimits` bounds every parser-built document: 100,000 live nodes,
256 parent-child edges and 16 MiB of retained names and text. Until now,
reaching any of them failed the whole parse, and the host got no document
at all ([TD-02](../TECH_DEBT.md#td-02--depth-and-node-budgets-abort-the-whole-parse)).
That was safe for local fixtures, but M6 loads real pages, and browsers
do not fail on them:

- **Depth.** Blink and WebKit cap tree construction depth. Their
  `HTMLConstructionSite::attachLater` reads: "Add as a sibling of the parent
  if we have reached the maximum depth allowed", and replaces the parent
  with its own parent once the stack of open elements is deeper than the
  cap. Gecko adopted the same rule at "the Blink-defined magic depth for
  compatibility" (Mozilla bug 256180, fixed in Firefox 66). Content past
  the cap stays in the document, flattened.
- **Size.** No browser caps the node count of a page; memory runs out
  first. Rustic must keep a budget, because the DOM, style and layout charge
  memory per node and the engine promises bounded work, but a page over
  the budget should still show what fit.

The candidates were: keep failing (simple, but a real page fails to load);
flatten past the depth cap and truncate at the node and payload budgets; or
remove the budgets (unbounded memory for hostile input, against the M7
direction).

## Decision

**Depth: insert higher up, as Chromium does.** The tree builder computes
each insertion point as before, then moves it up while the new node would
not fit under `max_depth`. An element or comment keeps one edge free for
its own text, and text may use the last edge. This matches Chromium, which
caps elements and comments by stack depth and lets text go one level
deeper. Appending stays appending, so later nodes follow earlier ones; an
insertion before a reference (foster parenting) lands right after the
would-be parent. The adoption agency and `selectedcontent` cloning move
whole subtrees through the same cap. Each capped insertion records a
`NestingTooDeep` tree diagnostic; parsing never fails on depth.

With `max_depth` = 514 the trees are identical to Chromium 157's on the
recorded cases (nested blocks past the cap, the adoption agency, foster
parenting, formatting elements, lists with comments). The default stays at
256 edges for now, so pages between 256 and 514 levels deep are flattened
earlier than in Chromium ([TD-147](../TECH_DEBT.md#td-147--the-default-depth-limit-flattens-earlier-than-chromium)).

**Node and payload budgets: truncate the document.** When creating or
growing a node would exceed `max_nodes` or `max_data_bytes`, a document
parser stops tree construction and keeps the tree built so far, as if the
input had ended there but without EOF's implied nodes (which would need the
exhausted budget). Later input is accepted and ignored, no further script
boundaries surface, and `ParsedDocument::truncated()` is true with a
`Truncated` tree diagnostic. Hosts must surface it: the CLI prints a
warning on stderr and still produces its output.

Fragment parsers (`innerHTML` and friends) keep failing at the budgets, so
DOM APIs can throw `QuotaExceededError` and leave the target unchanged.
Tokenizer budgets, cancellation and allocation failure stay fatal for both.

## Consequences

- Deep and large pages load. A page over the node budget is visibly
  incomplete rather than absent, which is what M6 needs to judge real-page
  compatibility.
- The bounds are unchanged: memory per document is still at most the
  configured budgets, and every algorithm stays iterative.
- Each insertion walks the insertion point's ancestors once more to find
  its depth (at most `max_depth` steps), next to the walk the DOM already
  does.
- A truncated tree may end inside an element or a table, and an adoption
  agency run interrupted by the budget may leave a detached node, which the
  collector reclaims. The tree is still a valid DOM.

## Validation

- `depth_cap_places_nodes_like_chromium` compares five deep documents with
  `tests/fixtures/depth-cap-chromium.txt`, recorded by
  `tools/html_depth_reference.js` in Playwright's Chromium.
- The exact-budget tests in `crates/rustic-html/tests/document.rs` check
  that one edge less flattens, one node or byte less truncates, and that the
  late-head and 700-level cases follow the actual DOM depth.
- The html5lib tree-construction corpus is unchanged at the default
  limits.
- CLI tests: `dump-dom` succeeds on a 300-level document at the default
  depth, and warns when `--max-nodes` truncates a page.

## Revisit when

- Layout, paint and bindings are shown to handle 514-level trees on every
  tier (TD-147), or
- the node budget becomes a per-page memory charge (M7), or
- a browser changes its depth rule.
