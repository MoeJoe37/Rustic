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
cargo run --locked -p rustic-html --example text_modes
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
scaffold or claim rendering support. The CLI accepts only `info`, `--help`,
`--version`, the local-only `dump-dom` command described in
[HTML_TREE.md](HTML_TREE.md#local-dump-dom-command), and `dump-style`, which
prints computed styles for the same input ([CSS.md](CSS.md#style-dump)),
and `dump-boxes`, which prints an initial principal-box snapshot
([LAYOUT.md](LAYOUT.md)) without positions or painting. `dump-formatting`
derives anonymous flow boxes and split inline continuations, with independent
box/depth/work limits; it preserves unshaped UTF-16 text. `dump-layout` adds used
rectangles and margin collapse for an explicitly limited static block subset; it rejects
unsupported inline/text and other formatting rather than emitting partial output.
`dump-paint` prints the display list for that layout and `render` writes it as a
PNG through the CPU rasterizer ([PAINT.md](PAINT.md)).
While hosted Actions is unavailable, the maintainer has authorized sandbox-gated
merges: run the local gates and compatibility/compile checks, record their
actual scope, push the checked source to `feature`, and then merge that exact
source into `main`. Hosted runner results remain separate evidence.

The DOM `tree` example constructs nodes through the storage API, then emits an
inspection dump. It is a runnable check of document ownership and traversal, not
an HTML parser or the `dump-dom` command. The dump is capped at 64 KiB,
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
| `Test (platform)` | Linux x64/ARM64 (Ubuntu 24.04), Windows x64 (2022)/ARM64 (11), macOS ARM64/x64 (15); pinned toolchain | Workspace all-target tests, doctests, CLI info smoke run |
| `Emulated test (target)` | Ubuntu 24.04 with cross linkers and QEMU user mode | All-target tests and CLI smoke run for 32-bit ARM, 32-bit x86, RISC-V 64 and big-endian s390x |
| `Cross-check (target)` | Ubuntu 24.04 | `cargo check --all-targets` for ARMv6, musl ARM64/x64, Windows ARM64 (gnullvm) and 32-bit, POWER, LoongArch and FreeBSD |
| `Compatibility (toolchain)` | Ubuntu 24.04; 1.85.0 and stable | Workspace all-target tests under MSRV and current stable |
| `Mobile core (target)` | Ubuntu for Android ARM64/ARMv7/x86-64/x86; macOS for iOS ARM64 device/simulator and x86-64 simulator | Check all workspace libraries with pinned Rust; no application linking or device execution |
| `Repository checks` | Ubuntu 24.04, Python 3.11+ | Local Markdown file links, whitespace, TOML/JSON parse, manifest/toolchain consistency |
| `CI` | Aggregate gate | Fails if any prerequisite fails, is cancelled, or is unexpectedly skipped |
| `Publish progress` | Successful default-branch CI only | Regenerates README/chart; safely pushes changed outputs and dispatches bot-commit validation |

The target matrix and portability rules are in [PLATFORMS.md](PLATFORMS.md).
Checkout is pinned to the verified v7.0.1 commit. Rust toolchains are installed
with rustup already present on GitHub-hosted runners (the test job installs
rustup first on any image that lacks it). No additional third-party
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
tests, 1,079 pinned html5lib declaration cases, all 4,626 cases in the pinned
reference files, the 109-record/282-context text corpus and two rustdoc examples. The
`tokenize` example exercises chunked UTF-8, tags, comments, doctypes and processing
instructions; it does not build a DOM. [HTML.md](HTML.md) records corpus provenance,
reproduction commands, text-mode/pull-driver contracts and unimplemented tree builder. The
`--suite references` importer uses a separate recorded policy without changing the earlier
declaration selection. Repository checks validate all three fixture digests/counts/
indices and reproduce the vendored WHATWG entity table offline. Run
`python3 tools/import_html_entities.py --check` to check that table directly.
The text corpus uses `--suite text`, explicit initial modes and recorded last-tag
context, with record/state-run counts checked independently. The `text_modes`
example demonstrates feedback for a known fixture; it is not a tree builder.

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
