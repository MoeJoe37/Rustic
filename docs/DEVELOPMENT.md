# Development and CI

## Toolchain and local checks

Install [rustup](https://rustup.rs/), Git, and Python 3.11 or later. Opening the
workspace through Cargo selects `rust-toolchain.toml`: Rust 1.99.1 with rustfmt
and Clippy. The workspace is Rust 2024 with minimum compiler 1.85.0. Development
is pinned; MSRV and moving stable are additional compatibility checks, not the
local toolchain definition. All packages are private/unpublished.

```sh
cargo run --locked -p rustic-cli -- info
cargo run --locked -p rustic-dom --example tree
cargo run --locked -p rustic-url --example resolve
cargo run --locked -p rustic-html --example tokenize
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
cargo doc --locked --workspace --no-deps
python3 tools/check_repository.py
```

Use `python` instead of `python3` if that is the Python 3.11+ command on Windows.
Set `RUSTDOCFLAGS=-Dwarnings` when reproducing the CI rustdoc warning gate.
For compiler compatibility, install the additional toolchain explicitly:

```sh
rustup toolchain install 1.85.0 --profile minimal
cargo +1.85.0 test --locked --workspace --all-targets
rustup toolchain install stable --profile minimal
cargo +stable test --locked --workspace --all-targets
```

The first `Cargo.lock` is a dependency-free workspace lockfile. Keep it committed
and use `--locked` so implicit resolution changes fail visibly. Do not publish the
scaffold or claim rendering support. The initial CLI deliberately accepts only
`info`, `--help`, and `--version`.

The DOM `tree` example constructs nodes through the storage API, then emits an
inspection dump. It is a runnable check of document ownership and traversal, not
an HTML parser or the future `dump-dom` command. The dump is capped at 64 KiB,
contains no allocation IDs, preserves attribute insertion order, and quotes
names/UTF-16 values including lone surrogates. Its diagnostic format is not HTML
serialization or a stable external protocol. See [AGENTS.md](../AGENTS.md) for
repository-level coding and validation rules.

## Initial GitHub Actions

[ci.yml](../.github/workflows/ci.yml) runs on pushes to any branch, pull requests,
and manual dispatch. Build/test jobs use read-only contents permission, finite job timeouts,
concurrency cancellation for superseded runs, and a full-SHA-pinned checkout with
credential persistence disabled. No repository secrets are needed. It does not
use `pull_request_target`, publish packages, deploy a site, or auto-merge. The
post-CI default-branch progress publisher alone has contents/actions write access
to refresh generated README assets and dispatch CI for its bot commit.

| Job | Environment | Checks |
| --- | --- | --- |
| `Quality` | Ubuntu 24.04, pinned toolchain | rustfmt, Clippy with denied warnings, rustdoc with denied warnings |
| `Test (OS)` | Ubuntu 24.04, Windows 2022, macOS 15; pinned toolchain | Workspace all-target tests, doctests, CLI info smoke run |
| `Compatibility (toolchain)` | Ubuntu 24.04; 1.85.0 and stable | Workspace all-target tests under MSRV and current stable |
| `Mobile core (target)` | Ubuntu for Android ARM64/x86-64; macOS for iOS ARM64 device/simulator | Check all workspace libraries with pinned Rust; no application linking or device execution |
| `Repository checks` | Ubuntu 24.04, Python 3.11+ | Local Markdown file links, whitespace, TOML/JSON parse, manifest/toolchain consistency |
| `CI` | Aggregate gate | Fails if any prerequisite fails, is cancelled, or is unexpectedly skipped |
| `Publish progress` | Successful default-branch CI only | Regenerates README/chart; safely pushes changed outputs and dispatches bot-commit validation |

Checkout is pinned to the verified v7.0.1 commit. Rust toolchains are installed
with rustup already present on GitHub-hosted runners. No additional third-party
action is needed for setup or caching. The exact Rust release is intentionally
duplicated in workflow env and the toolchain file, and the repository check
verifies they agree. Update the pin and lockfile/tooling in a reviewed change;
do not silently drift development Rust when moving stable advances.

The `CI` aggregate job has a stable check name suitable for a ruleset. After a
successful first run, the maintainer can require `CI` for `main`, prohibit force
push/deletion, and require review if collaborators join. **Those repository
administration settings are not configured by this commit.** They are distinct
from adding a workflow and may require GitHub plan/administration support.

Dependabot configuration checks Cargo and GitHub Actions weekly, groups related
update PRs, and caps open requests. Scheduled dependency checks depend on GitHub
repository settings. Private-repository runner usage is subject to the account's
Actions allowance. There are no scheduled heavy build/fuzz jobs in the first CI.

## Failure diagnosis

The [progress reporter](PROGRESS.md) validates catalog/evidence and runs eight
Python regression tests in the repository job. The chart is regenerated for each
source snapshot; successful default-branch CI publishes changed output. Inspect
the publishing job as well as the native/check gate when testing automation.

The pinned URL-data subset runs offline as a normal Rust integration test. Use
`cargo test --locked -p rustic-url --test wpt -- --nocapture` for case counts.
[URL.md](URL.md) documents the fixed selection, limits, unsupported features and
fixture update/check commands. Repository checks validate the generated fixture
digest against its manifest; normal CI does not download upstream test data.

`cargo test --locked -p rustic-html` runs the streaming input/tokenizer subset
tests, 1,079 pinned html5lib declaration cases and its rustdoc example. The
`tokenize` example exercises chunked UTF-8, tags, comments, doctypes and processing
instructions; it does not build a DOM. [HTML.md](HTML.md) records corpus provenance,
reproduction commands and the unimplemented character references/text modes.
The importer has three Python regression tests; repository checks validate its
fixture digest, aggregate counts and per-source selection indices.

Read the first failed step, not just the aggregate gate. A lockfile failure means
the manifest/lockfile need a deliberate update. A format failure is resolved with
`cargo fmt --all`. A Clippy change from a compiler upgrade needs inspection; avoid
blanket lint allows. A stable-only failure may indicate compiler drift, while an
MSRV-only failure indicates use of a newer API or dependency requirement.

For repository checks, fix the named file/line or missing local link target.
External URLs and Markdown fragment anchors are intentionally not fetched or
validated by this lightweight script. It is not a Markdown renderer, a full
workflow linter, or a security audit. GitHub validates workflow syntax at load;
the initial configuration should also be parsed/reviewed before committing.

## Workflow growth and releases

See [MOBILE.md](MOBILE.md) for target triples, native host milestones and runtime
gates. Mobile metadata checks are included in the aggregate `CI` job, while
NDK/Xcode linking, packaging and device testing are staged with actual adapters.

Introduce parser corpora with M1, render artifacts with M3, Test262 with M4, and
integrated WPT with the host adapter. Add bounded fuzz smoke checks first, then
scheduled long campaigns when targets exist. Add dependency vulnerability/license
tools when external crates arrive, and sanitizer/native checks when their surface
exists. Keep action references pinned and permissions narrow in every workflow.

Release packaging/signing/publishing requires a separate reviewed workflow once
there is an actual release contract. Do not grant CI write permission or store
release credentials merely to make a first scaffold commit. See the
[roadmap](ROADMAP.md) for release gates and [contributing](../CONTRIBUTING.md) for
review expectations.
