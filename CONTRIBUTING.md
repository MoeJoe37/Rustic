# Contributing to Rustic

Rustic is an original browser engine written in Rust. The desktop alpha includes
rendering, scripting, network and storage services, and process isolation.
Compatibility work continues across the engine and web platform. Check the
[implementation ledger](docs/IMPLEMENTATION.md) for what works today and the
[roadmap](docs/ROADMAP.md) for dependencies and upcoming work.

## Set up a checkout

Install Git, [rustup](https://rustup.rs/), and Python 3.11 or later. Rustup reads
[rust-toolchain.toml](rust-toolchain.toml) to select the pinned compiler, rustfmt,
and Clippy. CI also tests Rust 1.85.0 and the current stable release. Keep
`Cargo.lock` committed and use `--locked` for builds and tests.

```sh
git clone https://github.com/MoeJoe37/Rustic.git
cd Rustic
git switch feature
cargo run --locked -p rustic-cli -- --help
```

On Linux x86-64 and ARM64, renderer isolation tests require bubblewrap
(`bwrap`) on `PATH` and access to unprivileged user namespaces. See the
[development guide](docs/DEVELOPMENT.md#toolchain-and-local-checks) for setup
and local test options. The [platform matrix](docs/PLATFORMS.md) distinguishes
native tests, emulated tests, and cross-compilation checks.

## Prepare a focused change

- Read [AGENTS.md](AGENTS.md) for repository rules and the
  [architecture](docs/ARCHITECTURE.md) before moving responsibilities across
  crates. Use the [documentation index](docs/README.md) to find the subsystem
  guide for your change.
- Describe the behavior you are changing and the relevant roadmap item or
  specification section. Add a small regression test for behavior changes.
  Cover malformed input, resource limits, cancellation, or reentry when the
  changed code handles them.
- Keep shared code portable across pointer widths, byte orders, and operating
  systems. Keep OS-specific code in adapters and add evidence from the
  affected platforms for native changes. Use safe Rust by default and document
  the boundary and invariants of any proposed `unsafe` code for review.
- Keep the browser engine, JavaScript runtime, and Wasm implementation in
  Rustic. Use reviewed libraries for support services through explicit adapter
  boundaries. Follow the [dependency policy](docs/DEPENDENCIES.md) for
  dependency, lockfile, and license-notice changes.
- Update the implementation ledger and affected subsystem docs when behavior
  changes. Use an [ADR](docs/adr/README.md) for changes to ownership, execution,
  process authority, public API guarantees, engine scope, or major dependencies.
  Record deliberate shortcuts in the [technical debt ledger](docs/TECH_DEBT.md).

## Verify the change

Run the repository checks from the checkout root. On systems that provide
Python 3.11 or newer as `python`, use that command in place of `python3`.

```sh
python3 tools/progress.py --write
python3 tools/progress.py --check
python3 tools/check_repository.py
python3 -m unittest discover -s tools/tests -p 'test_*.py'
```

For the Rust checks, use the following commands in a POSIX shell. In PowerShell,
set `$env:RUSTDOCFLAGS = '-Dwarnings'` before running the `cargo doc` command.
The [development guide](docs/DEVELOPMENT.md) gives the pinned `cargo-deny`
installation command and the rest of the CI checks.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --no-deps
cargo deny --locked check advisories licenses bans sources
```

Run the affected integration harnesses as well. Use the pinned Test262 checkout
for JavaScript runtime changes and the pinned Window WPT selection for changes
to browser-exposed behavior. Explain each expected-failure or reference-output
update and record the suite revision and results. Keep the reference path
available when introducing an optimization. See the
[testing guide](docs/TESTING.md) and [standards reporting](docs/STANDARDS.md).

## Submit and review

Base contributions on `feature` and target it when opening a pull request.
Include the original problem, the observable result, and the checks you ran.
Record any skipped tests or platform limits so reviewers can see what still
needs verification.

The maintainer integrates changes through `feature` and merges to `main` after
the aggregate **CI** check passes. Check the failed job and step when a run
fails. A new push can cancel an in-progress run for the same branch. The
default-branch workflow can publish refreshed progress assets after CI, so
sync those commits into local work while preserving branch history.
[GitHub Actions](https://github.com/MoeJoe37/Rustic/actions)

## Report issues and preserve notices

For public bug reports, include the affected commit, OS and architecture,
steps to reproduce, and what you expected and observed. Use a reduced fixture
and remove credentials and private browsing data from logs and screenshots.
[GitHub Issues](https://github.com/MoeJoe37/Rustic/issues)

Follow the private reporting steps in [SECURITY.md](SECURITY.md) for
vulnerabilities.

Rustic is licensed under [AGPL-3.0-only](LICENSE). Preserve required notices
and record the upstream revision and license when importing test fixtures,
fonts, or other third-party material. See the
[reviewed dependency notices](third_party/README.md).
