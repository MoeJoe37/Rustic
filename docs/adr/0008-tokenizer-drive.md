# ADR 0008: Token-at-a-time HTML drive and explicit text contexts

- Date: 2026-11-07
- Status: Accepted internal contract; tree construction remains pending

## Context

HTML tree construction selects RCDATA, RAWTEXT, script and plaintext modes after
relevant start tags. Tokenizing a whole supplied chunk into a vector before
returning control cannot honor a mode change when the rest of the element is in
that same chunk. Inferring modes from tag names inside the tokenizer would lose
namespace, insertion-mode and scripting context. Script completion also requires
a future parser to pause before consuming following input.

## Decision

Separate bounded input submission (`write`, `end`) from token consumption
(`next_token`). The caller may choose a text mode immediately after receiving a
start tag, before the driver processes another input scalar. `with_context`
supplies explicit initial mode/last-start-tag context for tokenizer tests and
future fragment integration; it is not a fragment parser. Existing `feed` and
`finish` remain collecting conveniences for fixed-context consumers.

The decoder retains its raw UTF-8/BOM/newline contract. The tokenizer owns queued
decoded input, moves new buffers where possible and compacts consumed prefixes
amortized over progress. A bounded token queue preserves multiple emissions from
one input scalar/EOF recovery. State-machine modules retain responsibility for
syntax; the driver owns submission, pause boundaries, lifecycle and delivery.
The last delivered start tag supplies appropriate-end-tag matching. Invalid mode
control calls preserve state; resource/cancellation failures terminate the stream.

## Alternatives and consequences

Automatic tag-name switching is incorrect outside a known HTML fixture. Requiring
one-byte feeds would make semantics depend on an integration workaround. A
callback-only interface without input retention cannot pause halfway through an
already decoded chunk. The pull interface permits both same-chunk feedback and
bounded pause/resume while keeping allocation/ownership explicit.

Queued decoded bytes, token buffers, retained context and temporary probes have
documented logical limits, not a process-wide memory guarantee. Token grouping
remains deterministic across chunk boundaries. `None` means no token is currently
available; the single EOF token identifies completion. `end` closes submission
before queued input is exhausted. This is an unstable internal interface, not a
new stable embedding promise.

## Validation and revisit triggers

Preserve prior pinned declaration/reference behavior and test new text states,
all byte splits, initial contexts, Unicode/EOF handling, same-chunk mode feedback,
partially read input/appends, invalid controls, budgets and cancellation. Record
the distinction between Unicode tokenizer fixtures and UTF-8 transport BOMs.

Tree-builder insertion modes, foreign-content CDATA context, parser-blocking
script execution, `document.write` insertion/reentry, and encoding restart are
separate pending work. Add their explicit controls at the owning boundary when
implemented; do not claim that pausing alone implements them. Revisit this record
if those algorithms require different input ownership or stable public contracts.
