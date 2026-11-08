# M3 block reference — sandbox validation, 2026-11-08

The maintainer authorized sandbox-gated merges while GitHub Actions is
unavailable. The previously validated M2 fixes and REN-01a source at
`66feee253a81f0b7cfb87a8fd996331cd204b262` were fast-forward merged from
`feature` into `main`, retaining both original commits and their history.
Development then continued on `feature`. This follow-up records the next tested
unit; its exact source is the commit containing this report.

## Delivered behavior

The first REN-01b vertical reference consumes an immutable principal snapshot
and emits physical block content/padding/border rectangles with source and
parent links. It resolves nested widths/positions and auto, fixed, min/max,
percentage and calc heights, including border-box adjustment, containing
content height, RTL over-constraint and viewport resize with refreshed styles.
An iterative stack finalizes child heights before advancing following siblings;
visible overflow from a fixed-height child does not move those siblings.

`rustic dump-layout` shares the existing local HTML/stylesheet loading policy
and outputs deterministic geometry. It does not paint images. The supported
subset is deliberately explicit: non-replaced static block/flow-root boxes,
zero vertical margins and visible overflow. Pure collapsible whitespace under
normal/nowrap creates no fragment. Meaningful/preserved text, inline/list/flex/
grid/table contexts, nonzero vertical margins, positioning/floats, overflow
semantics, special/replaced/control elements and foreign content fail with a
typed reason. No partial layout/stdout is returned. TD-38 records the expansion
work, and [LAYOUT.md](../LAYOUT.md) defines the API and exact limits.

Fourteen new flow scenarios complement the thirteen existing geometry, snapshot
and width scenarios. They cover exact nested rectangles, fixed-height overflow,
definite/indefinite percentages, min/max precedence, box sizing, calc, RTL,
resize, whitespace, explicit unsupported errors, failure atomicity, exact limits,
coordinate overflow, an independent 64-block prefix-sum model and a 200-level
chain. Two new CLI integration cases cover chunk-independent geometry, exact
output caps, URL refusal, local linked CSS and unsupported text with empty stdout.

No catalog denominator, capability size or existing completed gate changed.
The general block-flow/fragment steps remain unfinished; the progress number is
not increased merely because this narrower reference subset landed.

## Completed sandbox checks

| Check | Outcome |
| --- | --- |
| Formatting and all-target Clippy with denied warnings | Passed |
| Pinned Rust 1.99.1 workspace all-target tests | Passed |
| Rust 1.85.0 MSRV workspace all-target tests | Passed |
| Moving stable workspace all-target tests (resolved to 1.99.1) | Passed |
| Doctests and rustdoc with denied warnings | Passed |
| CLI `info`, `dump-layout` golden geometry and local stylesheet integration | Passed |
| Python tooling tests | 39 passed |
| Progress generation/check, repository consistency and whitespace checks | Passed |
| Required fixed-seed fuzz smoke | All nine existing targets passed replay plus 20,000 mutations each |
| Cross-compilation | All 20 targets below passed |

| Command mode | Targets |
| --- | --- |
| `cargo check --locked --workspace --all-targets --target …` | `aarch64-unknown-linux-gnu`, `i686-unknown-linux-gnu`, `armv7-unknown-linux-gnueabihf`, `riscv64gc-unknown-linux-gnu`, `s390x-unknown-linux-gnu`, `arm-unknown-linux-gnueabihf`, `aarch64-unknown-linux-musl`, `aarch64-pc-windows-gnullvm`, `i686-pc-windows-msvc`, `powerpc64le-unknown-linux-gnu`, `loongarch64-unknown-linux-gnu`, `x86_64-unknown-freebsd`, `x86_64-unknown-linux-musl` |
| `cargo check --locked --workspace --lib --target …` | `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`, `i686-linux-android`, `aarch64-apple-ios`, `aarch64-apple-ios-sim`, `x86_64-apple-ios` |

Native execution in this sandbox is Linux x86-64. Cross checks compile; they do
not execute native Windows/macOS/ARM64, QEMU or mobile applications. Those remain
separate platform evidence, and the hosted workflow is left intact. In accordance
with the maintainer's instruction, its unavailable jobs are not used to block
this sandbox-tested merge. Source changes go through `feature` before an exact
fast-forward merge into `main`; no history is rewritten.
