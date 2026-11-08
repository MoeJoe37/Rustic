# Engine progress reporting

The README's progress map is inspired visually by
[AnyPS5's progress map](https://github.com/boykopovar/AnyPS5). Rustic's generator
and measurement model are original. The figure is a native SVG stored in the
repository, so private repositories do not need public Pages hosting or an
external badge service to display it.

## What the number means

[progress/catalog.json](../progress/catalog.json) defines `engine-v3`: 21 weighted
subsystems and 172 concrete capabilities covering the whole engine plan. Subsystem
weights sum to 100. Inside a subsystem each capability has a **size** of 1, 2, 3,
5 or 8 points describing its relative scope, so finishing the CSS property
catalog (8) moves the bar more than finishing the CSS tokenizer (3). Each
capability also names the [roadmap](ROADMAP.md) **milestone** (M1–M10) that
delivers it.

A capability earns **credit** of 1 when complete, a verified step fraction when
in progress (see below), and 0 otherwise. Then:

- subsystem progress = Σ(size × credit) / Σ size;
- overall progress = Σ(subsystem weight × subsystem progress);
- a capability's engine share = subsystem weight × size / Σ size, and a
  milestone's progress is the share-weighted credit of its capabilities.

Every report states the overall figure as **verified complete + partial
credit**, so in-progress work can never hide inside the headline number. Use the
[generated tables and evidence](progress/summary.md) to inspect every capability,
including unfinished libraries which have no crate yet.

**100% means all catalog capabilities and final full-engine acceptance are
complete**, including JavaScript/GC, rendering/layout, standards and site gates,
network/storage/security, accessibility, performance/reliability, embedding,
GPU/Wasm, Rustic MV2 extensions, the uBlock Origin port, comparative
security/performance evidence, signed artifacts and runtime acceptance on Windows, macOS, Linux, Android and
iOS/iPadOS. It cannot be reached by completing just the currently existing crates
or the first milestone. The final acceptance gate remains open until the actual
engine and required platform evidence are ready.

This is a weighted delivery model, not a measured percentage of all web standards,
code lines, elapsed effort, or time remaining. Weights and sizes are engineering
scope allocations documented in the catalog. The living web evolves; a scope change
requires a reviewed catalog version update and an explanation of denominator/
weight changes. Do not remove unfinished work, shrink sizes or rename partial
implementations to raise the number.

## Reading the chart

- **Header:** the overall percentage and one stacked bar: green is verified
  complete, blue is partial credit, gray is remaining.
- **Scope map:** one block per subsystem, its width equal to the subsystem's
  engine weight and its fill equal to that subsystem's progress. The total filled
  area is exactly the overall percentage, so a large unfinished area such as
  JavaScript is visible at a glance.
- **Roadmap milestones:** progress per milestone and the share of the engine
  scheduled in it. The README names the earliest unfinished milestone.
- **Subsystems:** weight, progress bar, percentage, a cell per capability whose
  width is its size, and engine points (weight × progress; all points add up to
  the overall percentage). An outlined cell is in progress; its blue fill is its
  verified step fraction.

## engine-v3 measurement change (2026-11-08)

engine-v3 keeps every capability ID, subsystem and weight from engine-v2. It adds
capability sizes, milestone assignments and evidence-backed partial credit; the
scope did not change. With the same 20 completed capabilities, engine-v2 measured
**9.04%** and engine-v3 measures **8.03% verified**: early work finished many
small foundation, URL and DOM items, which equal weighting overstated, while large
items such as the JS heap and CSS property catalog remain open. `html.conformance`
is the first in-progress capability, with three of five verified steps (pinned
tokenizer corpora at every split, the html5lib document tree corpus at every split,
and exact node/depth/payload limits), adding **0.37%** partial credit for an
**8.40%** total. Fragment-parsing records and fuzz targets remain open. The DOC-08
fuzz step later completed with its own evidence (four of five steps, 0.50%
partial credit), and DOC-06c's fragment records completed the last step, so
`html.conformance` is now complete (8.65%).

## REN-02a checkpoint (2026-11-08)

The same `engine-v3` denominator, weights, sizes and step definitions are kept.
`layout.block` advances from one of four steps (widths) to three of four
(widths, normal vertical block flow and adjoining/parent-child margin collapse).
Source and test markers cover used geometry, an independent sibling adjacency
model and boundary/empty/min-height regressions. Mixed inline/block anonymous
flow remains planned; no whole block, fragment or M3 capability is marked done.
As in the prior M3 commits, run `tools/progress.py --write` to regenerate the
README block and SVG/JSON/Markdown outputs, then validate with `--check`.

## engine-v2 scope change (2026-11-07)

[ADR 0007](adr/0007-product-scope.md) adds required WebAssembly, Rustic-only
Manifest V2 extensions, the uBO port and comparative security/privacy/performance
acceptance. All 142 earlier capability IDs are retained; 30 planned capabilities
are added, giving 172 across 21 subsystems. The existing JS/Wasm integration gate
remains separate from the new runtime's decoder/validator/execution work.

Weights remain a 100% scope allocation. Dedicated Wasm/extension/filtering tracks
receive weight and security/GPU receive more, with explicit reductions elsewhere:

| Subsystem | engine-v1 | engine-v2 |
| --- | --- | --- |
| Foundation | 2% | 2% |
| URLs & encoding | 4% | 3% |
| DOM | 7% | 6% |
| HTML | 7% | 6% |
| CSS & style | 10% | 9% |
| Text & fonts | 5% | 4% |
| Layout | 10% | 9% |
| Paint & raster | 7% | 6% |
| JavaScript & GC | 15% | 12% |
| WebAssembly | 0% | 4% |
| Bindings & web APIs | 7% | 6% |
| Network & navigation | 5% | 4% |
| Storage & workers | 4% | 3% |
| Security & IPC | 5% | 6% |
| Compositor & GPU | 3% | 4% |
| Platform hosts | 4% | 4% |
| Accessibility | 2% | 2% |
| DevTools | 1% | 1% |
| Extensions (MV2) | 0% | 4% |
| uBO port & filtering | 0% | 3% |
| Embedding & release | 2% | 2% |

At migration the same 15 completed capabilities measure **6.04%**, versus 6.89%
under engine-v1. That change represents a larger accepted product scope, not lost
implementation or new completion credit. Later source commits add credit only
when their declared gates pass. Historical snapshots remain reproducible in Git;
do not compare percentages across catalog versions without their weights/scope.

## Evidence and status

- **Green / complete:** the declared behavior exists, focused tests cover it, and
  the catalog points to source and test files with checked markers. Required CI
  must pass before the automation publishes a changed chart.
- **Blue / partial:** an `in_progress` capability may list two or more `steps`.
  Each complete step needs its own source and test evidence with checked markers,
  exactly like a complete capability. Credit is the fraction of complete steps.
  An in-progress capability without steps earns nothing, and one whose steps are
  all complete must be marked complete instead.
- **Gray / planned:** the capability is unfinished and contributes no credit.

The generator validates schema, all 21 subsystem identities, positive weights,
capability sizes and milestones, step rules, unique capability IDs, mandatory final/platform/conformance gates, and complete
items' evidence paths/markers. It does not prove semantics by reading source.
Maintainers must review the exact claim and tests; an empty function or placeholder
never qualifies. Full correctness cannot be inferred from a grep match. Evidence
markers detect deleted/moved evidence and prevent stale completion claims from
silently remaining green. Existing Rust tests, upstream selections and platform
checks provide the execution evidence in CI.

Percentages are calculated with exact rational arithmetic and truncated to two
decimals, so an incomplete engine cannot round up to 100%. The number of green
cells is not itself the overall percentage: subsystem weights and capability
sizes differ. Displayed verified and partial values are truncated so they add
up exactly to the shown total. Mobile
cross-compilation completes only its small build-check capability; native mobile
runtime acceptance remains separately pending.

## Updates on commits

Every CI run for a push or pull request regenerates and validates the report and
adds its table to the job summary. After successful CI on the default branch,
the `Publish progress` job refreshes the README block, SVG, JSON snapshot and
evidence summary. If they changed, it pushes a generated-only bot commit and
dispatches ordinary CI for that commit. The bot uses no personal token or secret
setup: its job alone receives contents/actions write permissions. Build/test and
pull-request jobs keep read-only permissions. Pull requests never publish with
that write token.

Deterministic source fingerprints exclude generated outputs and the README's
generated block, so the follow-up run sees identical output and stops. A source
or documentation commit updates the fingerprint; the percentage changes when
completed capabilities change. Superseded runs cannot overwrite newer source
heads, and pushes never force history. Push and dispatch concurrency groups are
separate so the follow-up validation does not cancel its source run.

The canonical README follows the default branch. Feature branches can include
fresh generated assets by running the tool before committing; their CI report is
also available on each commit. Failed source CI does not publish fresh completion
claims. Repository rules can independently restrict bot writes; if those are later
enabled, the publishing job reports the failure rather than bypassing them.

## Local workflow

1. Implement a bounded capability and meaningful tests.
2. Update its catalog status and source/test references alongside the code.
3. Run the required Rust, repository and Python tooling checks.
4. Regenerate the README and assets locally, or let successful default-branch CI
   publish them automatically. Review unexpected percentage or scope changes.

```sh
python3 -m unittest discover -s tools/tests -p 'test_*.py'
python3 tools/progress.py --write
python3 tools/progress.py --check
```

Only the marked README block and `docs/progress/engine.svg`, `data.json`, and
`summary.md` are generated. Do not hand-edit them. No timestamps or self-referential
commit SHAs are embedded: the source fingerprint identifies the exact content
snapshot while avoiding an endless update loop. Publication tests exercise a
local bare Git remote to verify changed/unchanged updates, CI dispatch, exclusion
of unrelated files and protection against a newer source head.
