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
   [Platform support](PLATFORMS.md): how code stays portable to any device that can run a browser.
7. [Decision records](adr/README.md): why key choices were made.

See [implementation progress](IMPLEMENTATION.md) for completed tasks and their contracts,
and the [technical debt ledger](TECH_DEBT.md) for known shortcuts and the rules that
keep them from becoming bad debt.
The accepted [product requirements](PRODUCT.md) and [extension/uBO design](EXTENSIONS.md)
define mandatory full-engine scope beyond the first document milestones.
The [README graph](PROGRESS.md) measures whole-engine delivery using a weighted
capability catalog, with source/test evidence and automatic default-branch updates.

## Implementation inventory

| Item | Status | Evidence |
| --- | --- | --- |
| Workspace, lockfile, pinned toolchain, lint policy | Implemented | Root Cargo/toolchain files |
| Viewport dimensions and pixel budget | Implemented | `rustic-engine` validation and boundary tests |
| CLI information/help/version and rejection of unsupported commands | Implemented | `rustic-cli` and subprocess tests |
| Document/node identity, budgets, cancellation, byte spans and clocks | Implemented | `rustic-base` and boundary/concurrency tests; DOC-01 |
| Bounded DOM node storage, names/attributes, UTF-16 data, checked moves and destruction | Implemented internal subset | `rustic-dom`; DOC-03 and first DOC-04 slice |
| Checked UTF-16 text/comment edits and direct UTF-8 append | Implemented storage subset | [DOM contracts and model/budget tests](DOM.md); observers/ranges/bindings pending |
| Bounded deterministic subtree dump and executable tree example | Implemented diagnostic tool | `rustic-dom` example and golden/escaping/boundary tests |
| Android/iOS shared-library cross-compilation | Configured | Seven mobile target checks across ARM64, 32-bit ARM, x86-64 and x86; runtime and host work in [mobile plan](MOBILE.md) |
| Any-device portability (x64, ARM64, 32-bit ARM/x86, RISC-V, big-endian) | Configured and required | Native tests on six desktop OS/CPU pairs, four emulated test targets and eight cross-checks; [platform support](PLATFORMS.md), ADR 0013 |
| URL records, parser, tuple and opaque origins | Implemented declared subset | `rustic-url`; [scope, file URLs, diagnostics and 815 pinned cases](URL.md) |
| Incremental UTF-8, tags/attributes, declarations, references and text modes | Implemented declared tokenizer scope | `rustic-html`; [53 focused tests and three pinned corpora](HTML.md) |
| Token-at-a-time input drive and text-mode feedback | Implemented internal contract | Same-chunk feedback, queued-input, control/lifecycle tests; [ADR 0008](adr/0008-tokenizer-drive.md) |
| Document tree construction: all 21 insertion modes, tables, formatting, templates and foreign content | Implemented for document parsing | [Scope and 1,772/1,776 runnable html5lib records, all 192 fragments included](HTML_TREE.md); scripting and `selectedcontent` pending |
| HTML fragment parsing algorithm (scripting disabled) | Implemented | `HtmlFragmentParser`; html5lib fragment records and focused tests |
| CSS syntax tokenizer, component values, rules, declarations and nested block contents | Implemented (CSS-01) | `rustic-css`; [139 pinned corpus cases and focused tests](CSS.md) |
| Deterministic parser fuzz targets, seed replay and reproducers | Implemented blind mutation fuzzer | `rustic-fuzz`; required CI smoke job and nightly campaign |
| Cross-platform build/test, formatting, Clippy, rustdoc, repository checks | Configured | [CI workflow](../.github/workflows/ci.yml) |
| Weekly Rust/action dependency update checks | Configured | [Dependabot](../.github/dependabot.yml) |
| Remaining HTML tokenizer/tree rules, browser DOM bindings, CSS, layout, painting, JavaScript | Planned | M1–M5 |
| Network, storage, browser UI, accessibility, sandbox | Planned | M6–M8 |
| Pinned WPT URL data subset | Implemented offline runner | 815 selected URL records, including file URLs; no full browser harness |
| Pinned html5lib declaration tokenizer subset | Implemented offline runner | 1,079 selected cases; token fields/error codes and all fixture splits |
| Pinned html5lib character-reference corpus | Implemented offline runner | All 4,626 cases in three pinned files; fields/diagnostics and all fixture splits |
| Pinned html5lib text-mode corpus | Implemented offline runner | 109 selected records expanded into 282 contexts; initial modes/last tag and all fixture splits |
| Whole-engine progress graph | Implemented and automated | 21 subsystems, 172 capability gates; [methodology](PROGRESS.md) |
| Brave comparison, competitive browser targets, Wasm, native MV2 extensions and uBO port | Accepted product requirements; implementation pending | [PRODUCT.md](PRODUCT.md), [EXTENSIONS.md](EXTENSIONS.md), ADR 0007 |
| Browser WPT harness, Test262, fuzzing, benchmarks, releases | Planned | Milestone-specific gates; no broad results claimed |

There are no external Rust dependencies yet. URL subset evidence is recorded in
[URL.md](URL.md); there are no whole-WPT/Test262 pass percentages, sandbox
guarantees, performance measurements, or releases yet.
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
