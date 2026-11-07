# Rustic

[![CI](https://github.com/MoeJoe37/Rustic/actions/workflows/ci.yml/badge.svg)](https://github.com/MoeJoe37/Rustic/actions/workflows/ci.yml)

Rustic is an independent browser engine being built from scratch in Rust. The
goal is a standards-oriented, embeddable engine with its own HTML/CSS pipeline,
DOM, layout, painting, JavaScript runtime, and web platform integration.

The target platforms are **Windows, macOS, Linux, Android, and iOS/iPadOS**. The
shared engine is being designed for desktop and mobile hosts from the beginning.
Mobile core cross-compilation is checked in CI; device shells, rendering, input,
and runtime validation are still planned. See the [mobile plan](docs/MOBILE.md).

**Current stage: early M1 document-core implementation.** This repository contains the project
architecture, implementation plan, shared engine primitives, and continuous
integration. Bounded DOM/URL foundations and an initial HTML input/tokenizer
subset are implemented. It does not yet build HTML documents, execute
JavaScript, fetch web pages, or render content. A complete modern browser engine is a sustained, multi-year
engineering effort; milestones below describe targets, not existing support.

<!-- rustic-progress:start -->

## Engine progress

[![Rustic engine subsystem progress](docs/progress/engine.svg)](docs/progress/summary.md)

**Overall engine completion: 6.19%** — 14/142 capabilities complete across 18 subsystems.

100% requires the full capability catalog and final engine acceptance on Windows, macOS, Linux, Android and iOS. This is weighted delivery progress; see [measurement and automatic updates](docs/PROGRESS.md).

<!-- rustic-progress:end -->

## Start here

| Document | Purpose |
| --- | --- |
| [Documentation index](docs/README.md) | Reading order and current implementation inventory |
| [Implementation progress](docs/IMPLEMENTATION.md) | Completed tasks, tested contracts, and the next implementation work |
| [Progress graph methodology](docs/PROGRESS.md) | Whole-engine completion criteria, weights, evidence and automatic commit updates |
| [URLs and origins](docs/URL.md) | Parser scope, identity rules, limits and pinned upstream test evidence |
| [HTML input and tokenization](docs/HTML.md) | Streaming UTF-8 and initial tokenizer subset, limits, evidence and remaining states |
| [Mobile support](docs/MOBILE.md) | Android/iOS architecture, compile checks and device milestones |
| [Architecture](docs/ARCHITECTURE.md) | Components, ownership, execution, IPC, embedding, and data flow |
| [Roadmap](docs/ROADMAP.md) | Dependency-ordered milestones, acceptance gates, first tasks, and risks |
| [Standards and compatibility](docs/STANDARDS.md) | Feature scope, authoritative references, and conformance reporting |
| [Security model](docs/SECURITY.md) | Trust boundaries, origin policy, isolation, and browsing gates |
| [Testing strategy](docs/TESTING.md) | Unit tests, conformance, rendering, fuzzing, and regression policy |
| [Performance plan](docs/PERFORMANCE.md) | Benchmarks, budgets, profiling, and optimization order |
| [Development and CI](docs/DEVELOPMENT.md) | Build instructions and exact automation coverage |
| [Dependencies and platforms](docs/DEPENDENCIES.md) | Meaning of “from scratch,” dependency policy, and platform rollout |
| [Architecture decisions](docs/adr/README.md) | Decisions, alternatives, and review triggers |

## Run the foundation

Install Rust through [rustup](https://rustup.rs/), then:

```sh
git clone https://github.com/MoeJoe37/Rustic.git
cd Rustic
cargo run --locked -p rustic-cli -- info
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
cargo run --locked -p rustic-dom --example tree
cargo run --locked -p rustic-url --example resolve
cargo run --locked -p rustic-html --example tokenize
```

The toolchain file pins Rust **1.99.1** for reproducible development. The minimum
supported Rust version is **1.85.0**, tested separately in CI; the workspace uses
the 2024 edition. `rustic info`, `--help`, and `--version` are the only commands.
Other arguments, including URLs, return an error instead of implying navigation.

The `tree` example builds nodes directly through the internal DOM API and prints
a deterministic inspection dump. It does not parse HTML or load a page.
The `tokenize` example reads a small UTF-8 stream in chunks and emits tokens;
comments, doctypes and processing instructions are included. Character references,
alternate text modes and tree building are pending.

The current crates are `rustic-base` (identity, budgets, cancellation, source spans
and clocks), `rustic-dom` (bounded node storage and checked tree mutation),
`rustic-url` (URL records, parsing and origins),
`rustic-html` (streaming UTF-8, tags/attributes, comments and declarations),
`rustic-engine` (bounded viewport configuration), and `rustic-cli`
(development entry point). Future crates listed in the architecture
are created only when their milestone needs them. All packages are unpublished.

## Development direction

1. Establish deterministic infrastructure and parser tests.
2. Render local static documents with an original HTML/CSS/layout pipeline.
3. Build the original JavaScript interpreter and connect it to DOM/event loops.
4. Add controlled networking, browser policy, storage, and platform integration.
5. Establish process isolation and security gates before general web browsing.
6. Expand modern platform compatibility, accessibility, performance, and embedding.

Rustic is not based on **Mozilla, Firefox, Gecko, or SpiderMonkey**. No Chromium,
Blink, WebKit, Servo, or existing JavaScript engine is embedded as its implementation.
That rule applies to desktop and mobile hosts alike. Reviewed low-level libraries
for TLS, fonts, Unicode, image codecs,
graphics APIs, and operating system integration are allowed. See
[dependency boundaries](docs/DEPENDENCIES.md) for the precise policy.

## Contributing and license

Read [CONTRIBUTING.md](CONTRIBUTING.md) before making a change. Security reports
follow [SECURITY.md](SECURITY.md). The existing [GNU Affero General Public License
version 3](LICENSE) is preserved; workspace metadata uses `AGPL-3.0-only`.
