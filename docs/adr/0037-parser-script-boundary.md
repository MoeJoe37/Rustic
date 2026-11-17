# ADR 0037: Suspend HTML parsing at script preparation

- Date: 2026-11-17
- Status: Accepted for the WEB-05 parser boundary. Execution and loading are subsequent slices.

## Context

[ADR 0009](0009-parser-owned-construction.md) keeps the tokenizer, tree builder
and partial document inside the parser. Running a parser-inserted script after
EOF exposes later markup too early and prevents the script from affecting
construction. Running it from inside a tree-builder mutation would leave a
borrow active when script calls the DOM again.

## Decision

Keep the parser as the sole owner of tokenization and insertion modes. With
scripting enabled, a host can advance one script boundary at a time. The HTML
tree builder pops the closed script, restores its previous insertion mode and
returns the element identity before the next token is processed. The host can
inspect the tree at that point. Template content and fragment parsers remain
inert, even when their scripting flag selects raw text for `noscript`. The
default document and fragment entry points keep scripting disabled.

In the execution slice, the engine will own the active parser and the realm
for one admitted document. While the parser is suspended, bindings must edit
the same document through checked, short borrows. The parser must release any
tree borrow before entering JavaScript or running the microtask checkpoint.
Parser-held node identities, including the open elements and active formatting
entries, must stay valid if a script detaches them and GC runs. Cross-document
adoption of a parser-held identity needs an explicit rule before enabling it.
Only the engine's load loop can resume the parser. Initially, reject
`document.write` on an active parser with an explicit exception.

The engine will classify the element, resolve eligible external sources via a
bounded host-supplied local loader, and own blocking, async and deferred work.
It will use the document's existing clock, task limits and cancellation token.
An inline or ready blocking classic runs against the partial document and
finishes its checkpoint before parsing resumes. Async work becomes eligible
when its source is ready. Defer and non-async modules wait for EOF and keep
document order. The engine owns readiness transitions and events as well as
`currentScript` for classic evaluation. A recoverable script error permits
later scripts to run. Cancellation and exhausted budgets stop further work.

## Validation

Check both scripting flags against pinned html5lib trees, including every byte
split and one-byte feeds. Prove that the host sees all earlier markup and the
complete script text, that markup after the script is still pending, and that
template contents and unterminated scripts do not suspend parsing. Exercise
cancellation while paused. The execution slice also requires GC and adoption
tests during suspension, microtask ordering, manual-clock tests with external
sources arriving out of order, and readiness and recoverable-error tests.

References: [WHATWG HTML parsing](https://html.spec.whatwg.org/multipage/parsing.html#parsing)
and [the script element](https://html.spec.whatwg.org/multipage/scripting.html#the-script-element).
