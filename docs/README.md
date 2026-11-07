# Rustic design and implementation guide

The documents in this directory are the initial engineering baseline. They
propose a design; only the inventory below describes implemented behavior.
Update the relevant document and decision record when implementation changes a
boundary. Status changes require test evidence rather than a checkbox alone.

## Reading order

1. [Architecture](ARCHITECTURE.md): how an engine fits together.
2. [Standards](STANDARDS.md): what behavior Rustic is targeting.
3. [Security](SECURITY.md): who may access which resources.
4. [Roadmap](ROADMAP.md): how to reach the design in usable increments.
5. [Testing](TESTING.md) and [performance](PERFORMANCE.md): how progress is measured.
6. [Development](DEVELOPMENT.md) and [dependencies](DEPENDENCIES.md): how to work here.
7. [Decision records](adr/README.md): why key choices were made.

See [implementation progress](IMPLEMENTATION.md) for completed tasks and their contracts.

## Implementation inventory

| Item | Status | Evidence |
| --- | --- | --- |
| Workspace, lockfile, pinned toolchain, lint policy | Implemented | Root Cargo/toolchain files |
| Viewport dimensions and pixel budget | Implemented | `rustic-engine` validation and boundary tests |
| CLI information/help/version and rejection of unsupported commands | Implemented | `rustic-cli` and subprocess tests |
| Document/node identity, budgets, cancellation, byte spans and clocks | Implemented | `rustic-base` and boundary/concurrency tests; DOC-01 |
| Bounded DOM node storage, names/attributes, UTF-16 data, checked moves and destruction | Implemented internal subset | `rustic-dom`; DOC-03 and first DOC-04 slice |
| Cross-platform build/test, formatting, Clippy, rustdoc, repository checks | Configured | [CI workflow](../.github/workflows/ci.yml) |
| Weekly Rust/action dependency update checks | Configured | [Dependabot](../.github/dependabot.yml) |
| HTML parsing, browser DOM bindings, CSS, layout, painting, JavaScript | Planned | M1–M5 |
| Network, storage, browser UI, accessibility, sandbox | Planned | M6–M8 |
| WPT, Test262, fuzzing, benchmark runners, release automation | Planned | Milestone-specific gates; no results claimed |

There are no external Rust dependencies yet. There are no WPT/Test262 pass
percentages, sandbox guarantees, performance measurements, or releases yet.
Configuration present in the repository is distinct from a successful Actions
run; inspect the [Actions page](https://github.com/MoeJoe37/Rustic/actions) for run results.

## Document conventions

“Must” states a design invariant or milestone gate. “Candidate” means a choice
still needs a focused evaluation. M0–M10 are defined in the roadmap. Task IDs
remain stable so they can later be copied into GitHub issues without losing
dependencies. Future file paths in prose are proposed; linked paths exist now.

Design baseline: 2026-11-07. Standards are living documents: each implementation
task records the exact section, upstream revision, and tests it used. Project
decisions are not claims that the standards prescribe this Rust architecture.
