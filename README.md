# Rustic

[![CI](https://github.com/MoeJoe37/Rustic/actions/workflows/ci.yml/badge.svg)](https://github.com/MoeJoe37/Rustic/actions/workflows/ci.yml)

Rustic is an independent browser engine being built from scratch in Rust. The
goal is a standards-oriented, embeddable engine with its own HTML/CSS pipeline,
DOM, layout, painting, JavaScript runtime, and web platform integration.

**Current stage: early M1 document-core implementation.** This repository contains the project
architecture, implementation plan, shared engine primitives, and continuous
integration. It does not yet parse HTML, execute JavaScript, fetch web pages, or
render content. A complete modern browser engine is a sustained, multi-year
engineering effort; milestones below describe targets, not existing support.

## Start here

| Document | Purpose |
| --- | --- |
| [Documentation index](docs/README.md) | Reading order and current implementation inventory |
| [Implementation progress](docs/IMPLEMENTATION.md) | Completed tasks, tested contracts, and the next implementation work |
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
```

The toolchain file pins Rust **1.99.1** for reproducible development. The minimum
supported Rust version is **1.85.0**, tested separately in CI; the workspace uses
the 2024 edition. `rustic info`, `--help`, and `--version` are the only commands.
Other arguments, including URLs, return an error instead of implying navigation.

The current crates are `rustic-base` (identity, budgets, cancellation, source spans
and clocks), `rustic-engine` (bounded viewport configuration), and `rustic-cli`
(development entry point). Future crates listed in the architecture
are created only when their milestone needs them. All packages are unpublished.

## Development direction

1. Establish deterministic infrastructure and parser tests.
2. Render local static documents with an original HTML/CSS/layout pipeline.
3. Build the original JavaScript interpreter and connect it to DOM/event loops.
4. Add controlled networking, browser policy, storage, and platform integration.
5. Establish process isolation and security gates before general web browsing.
6. Expand modern platform compatibility, accessibility, performance, and embedding.

No Chromium, Blink, WebKit, Gecko, Servo, or existing JavaScript engine is embedded
as the implementation. Reviewed libraries for TLS, fonts, Unicode, image codecs,
graphics APIs, and operating system integration are allowed. See
[dependency boundaries](docs/DEPENDENCIES.md) for the precise policy.

## Contributing and license

Read [CONTRIBUTING.md](CONTRIBUTING.md) before making a change. Security reports
follow [SECURITY.md](SECURITY.md). The existing [GNU Affero General Public License
version 3](LICENSE) is preserved; workspace metadata uses `AGPL-3.0-only`.
