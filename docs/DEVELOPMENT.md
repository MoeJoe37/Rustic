# Development and CI

## Toolchain and local checks

Install [rustup](https://rustup.rs/), Git, and Python 3.11 or later. Opening the
workspace through Cargo selects `rust-toolchain.toml`: Rust 1.99.1 with rustfmt
and Clippy. The workspace is Rust 2024 with minimum compiler 1.85.0. Development
is pinned; MSRV and moving stable are additional compatibility checks, not the
local toolchain definition. All packages are private/unpublished.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --no-deps
python3 tools/check_repository.py
python3 -m unittest discover -s tools/tests -p 'test_*.py'
python3 tools/progress.py --write && python3 tools/progress.py --check
cargo run --locked --release -p rustic-fuzz -- run --all --seed 1 --iterations 20000 --timeout-ms 10000
cargo deny --locked check advisories licenses bans sources
```

These are the gates CI runs (the last needs `cargo install cargo-deny --version
0.20.2 --locked`). Runnable examples: `rustic-dom` `tree`, `rustic-url`
`resolve`, `rustic-html` `tokenize`/`text_modes`/`document`, `rustic-text`
`shape_font`, `rustic-raster` `frame_budget` and `rustic-js` `parse`.

Use `python` instead of `python3` if that is the Python 3.11+ command on Windows.
Set `RUSTDOCFLAGS=-Dwarnings` when reproducing the CI rustdoc warning gate.
For compiler compatibility, install the additional toolchain explicitly:

```sh
rustup toolchain install 1.85.0 --profile minimal
cargo +1.85.0 test --locked --workspace --all-targets
rustup toolchain install stable --profile minimal
cargo +stable test --locked --workspace --all-targets
```

The CLI tests run renderers in the default sandbox (M7). On Linux x86-64 and
aarch64 that needs `bwrap` (bubblewrap) on `PATH` and unprivileged user
namespaces; on Windows nothing extra. On other Linux architectures, and to
run tests without a sandbox deliberately, set `RUSTIC_SANDBOX=0`.

Keep `Cargo.lock` committed and use `--locked` so dependency changes are
explicit. The reviewed libraries and their ownership boundaries are listed in
[DEPENDENCIES.md](DEPENDENCIES.md) and the
[dependency manifest](../third_party/rust-dependencies.json).

```sh
cargo run --locked -p rustic-cli -- --help
cargo run --locked -p rustic-cli -- info
```

The CLI help is the command and option reference. `dump-dom`, `dump-style`,
`dump-boxes`, `dump-formatting`, `dump-layout`, `dump-paint` and `dump-a11y`
inspect different stages of a document. `render` produces a PNG, `reftest`
compares pixels and `hit-test --at X,Y` queries the output geometry. Use
`view` for the reference desktop shell, `drive` for a live headless page,
and `automate` for the brokered stdio inspection and input protocol.
The [automation decision record](adr/0072-automation-protocol.md) describes
its grants and endpoints. The [layout guide](LAYOUT.md) and [paint guide](PAINT.md)
cover supported geometry, diagnostics and reference tests.

## Branches and merging

The repository keeps two branches: `main` and one working branch, `feature`.
All work is pushed to `feature`; once its checks pass, the maintainer's standing
instruction is to merge it into `main` straight away. The merge gate is the
aggregate `CI` job of GitHub Actions on
[MoeJoe37/Rustic](https://github.com/MoeJoe37/Rustic/actions), which runs
every tier of [PLATFORMS.md](PLATFORMS.md) natively or under emulation. A full
run takes an hour or more because jobs queue on the free tier, and a new push
to `feature` cancels the run in progress, so do not push in the middle of a run
whose emulated results you need. Run the quick local gates above before
pushing.

Hosted CI is running on `MoeJoe37/Rustic`. The
[platform verification record](PLATFORMS.md#verification-status) links to
completed full-matrix runs and preserves the November billing history.
If hosted jobs cannot start, record the exact Linux native, MSRV, emulated
and cross-check coverage you can run locally. Return to the full hosted gate
when it is available. A successful default-branch run can also push a
`chore(progress)` commit to `main`. Incorporate that history into `feature`
with a fast-forward or merge as appropriate.

The DOM `tree` example constructs nodes through the storage API, then emits an
inspection dump. It is a runnable check of document ownership and traversal, not
an HTML parser or the `dump-dom` command. The dump is capped at 64 KiB,
contains no allocation IDs, preserves attribute insertion order, and quotes
names/UTF-16 values including lone surrogates. Its diagnostic format is not HTML
serialization or a stable external protocol. See [AGENTS.md](../AGENTS.md) for
repository-level coding and validation rules.

## GitHub Actions

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
| `Test (platform)` | Linux x64/ARM64 (Ubuntu 24.04), Windows x64 (2022)/ARM64 (11), macOS ARM64/x64 (15); pinned toolchain | Workspace all-target tests with the default renderer sandbox (bubblewrap is installed on Linux; Seatbelt denials are printed on macOS failures), doctests, CLI info smoke run; the pinned Window WPT selection on Linux; JS-09 benchmark results in the Linux ARM64 job summary |
| `Emulated test (target)` | Ubuntu 24.04 with cross linkers and QEMU user mode | All-target tests and CLI smoke run for 32-bit ARM, 32-bit x86, RISC-V 64 and big-endian s390x (renderers run unsandboxed, since bubblewrap would start the host's emulator); 100-minute limit |
| `Cross-check (target)` | Ubuntu 24.04, with Windows for the MSVC job | `cargo check --all-targets` for ARMv6, musl ARM64/x64, Windows ARM64 (gnullvm), 32-bit Windows (MSVC), POWER, LoongArch and FreeBSD. gnullvm and LoongArch omit crates that need `ring` (TD-12) |
| `GPU frame-time gate (lavapipe)` | Ubuntu 24.04 with software Vulkan | Repeated GPU-process frame timings against the M8 software-adapter budgets in [PERFORMANCE.md](PERFORMANCE.md#gpu-frame-budgets) |
| `Parser fuzz smoke` | Ubuntu 24.04 | Replays the saved seeds and runs 20,000 fixed-seed mutations of every fuzz target; reproducers are kept as an artifact on failure |
| `Compatibility (toolchain)` | Ubuntu 24.04; 1.85.0 and stable | Workspace all-target tests under MSRV and current stable |
| `Mobile core (target)` | Ubuntu for Android ARM64/ARMv7/x86-64/x86; macOS for iOS ARM64 device/simulator and x86-64 simulator | Check all workspace libraries with pinned Rust (Android C dependencies built with the runner's NDK); no application linking or device execution |
| `Android isolated renderer` | Ubuntu 24.04 with the NDK and an API 34 x86-64 emulator | Builds `rustic-android` and the `android/` test app, then runs its instrumented isolation and crash-recovery test (ADR 0059) |
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

The `CI` aggregate job has a stable check name for branch rules. The
maintainer manages any required checks, force-push restrictions and review
rules through repository settings.

Dependabot configuration checks Cargo and GitHub Actions weekly, groups related
update PRs, and caps open requests. Scheduled dependency checks depend on GitHub
repository settings. Private-repository runner usage is subject to the account's
Actions allowance. [fuzz.yml](../.github/workflows/fuzz.yml) runs a nightly
fresh-seed campaign over its selected target matrix and
[dependencies.yml](../.github/workflows/dependencies.yml) runs `cargo deny`.

## Failure diagnosis

The [progress reporter](PROGRESS.md) validates catalog/evidence and has Python regression
tests (`tools/tests`) that run in the repository job. The chart is regenerated for each
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
reproduction commands and text-mode/pull-driver contracts. [HTML_TREE.md](HTML_TREE.md) describes the tree builder. The
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

CI runs the pinned Window WPT selection through `rustic automate` on Linux
and the Android isolation test in an emulator. The Test262 gates still use
an external pinned checkout (TD-145). The M7 AddressSanitizer run was manual
([security review](reviews/2026-11-20-m7-security-review.md)). The broader
browser WPT runner, a hosted sanitizer job, physical-device Android/iOS
checks and iOS runtime tests remain future work. See [TESTING.md](TESTING.md)
and [MOBILE.md](MOBILE.md) for the boundaries of each gate.

Release packaging/signing/publishing requires a separate reviewed workflow once
there is an actual release contract. Do not grant CI write permission or store
release credentials merely to make a first scaffold commit. See the
[roadmap](ROADMAP.md) for release gates and [contributing](../CONTRIBUTING.md) for
review expectations.


## Local static renderer demo with explicit fonts

After building `rustic`, render the M3 article with a granted Latin font and the
committed renamed Arabic/Indic fixtures:

```sh
cargo run --locked -p rustic-cli -- render --viewport 720x720 --font /path/to/NotoSans-Regular.ttf --font crates/rustic-text/tests/fonts/shaping/Arabic.ttf --font crates/rustic-text/tests/fonts/shaping/Devanagari.ttf --output target/m3-article.png crates/rustic-cli/tests/fixtures/m3/article.html
```

Try `--viewport 400x900` and `--zoom 1.25` for relayout. The original image swatch is generated
by `tools/generate_m3_swatch.py` using only Python's standard library. The Latin font is a host grant,
not downloaded by the engine. The first supplied family maps the CSS generics;
additional supplied families provide fallback. Inputs and decoded bytes are
bounded as described in [TEXT.md](TEXT.md). Without grants, bundled geometric test
fonts make tests reproducible but are not intended for reading articles.
