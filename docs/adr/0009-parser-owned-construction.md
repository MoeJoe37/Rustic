# ADR 0009: Parser-owned inert document construction

- Date: 2026-11-08
- Status: Accepted internal subset; broader tree and script integration pending

## Context

The token-at-a-time driver can pause for text-mode feedback, and the DOM now
supports checked UTF-16 appends. Tree construction needs an owner for the DOM,
insertion modes, open-element stack and error budgets. Exposing a partial
mutable tree would introduce script/embedding interleaving before ownership,
callbacks and reentry are implemented. A partial body algorithm must also avoid
silently claiming that unimplemented recovery rules succeeded.

## Decision

`HtmlDocumentParser` owns the tokenizer and a private `TreeBuilder`. The builder
owns its `Document` and exclusively uses checked DOM APIs. The tokenizer has no
node access. The parser pumps tokens and applies requested text modes at the
delivery boundary. Tree syntax modules own initial/head/body/character rules;
DOM ownership, identity and payload accounting remain in `rustic-dom`.

Only successful EOF returns a `ParsedDocument` and its compatibility mode and
bounded diagnostics. The caller can then transfer document ownership. A fatal
tokenizer/document/budget/cancellation/unsupported condition discards private
construction state and poisons the parser. Recoverable syntax conditions follow
the explicitly supported rules; unsupported construction families fail rather
than producing an apparently complete generic tree.

This parser is inert with scripting disabled and fixed UTF-8 input. It has no
fetching, host callbacks or script preparation/execution. The new dependency is
the existing original `rustic-dom` crate, as anticipated by the architecture.
No third-party runtime or platform dependency is introduced. This is an unstable
internal contract, not a stable embedding API or a new GC design.

## Alternatives and consequences

A caller-mutated partial tree would require reentry/range/observer contracts now.
A DOM callback interface would complicate failure ownership without a second
consumer. Treating all unknown tags as generic would return wrong trees for
tables, formatting, templates and foreign content. Explicit errors preserve an
honest implementation boundary while those algorithms are added.

New element families must extend scope predicates, implied-end-tag behavior and
tests together. Future parser/script reentry, fragment construction, encoding
restart, live observers or incremental embedding access require their own
concrete ownership/control design; do not expose the private builder to bypass
that work. The selected document mode belongs to parser result metadata for now;
future document/loader/layout integration must retain it explicitly.

## Validation and revisit triggers

Require fixed expected trees, pinned independently selected tree fixtures, every
byte split, one-byte feeds, multiple token sizes, text-mode feedback, exact DOM
and diagnostic budgets, cancellation, EOF recovery and failed-stream lifecycle.
Preserve the earlier tokenizer corpora. Keep unsupported constructs and unrun
conformance scope explicit in [HTML_TREE.md](../HTML_TREE.md).

Revisit before script execution, externally observable incremental construction,
cross-document adoption, shared parser state or stable embedding guarantees.
