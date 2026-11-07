# Working on Rustic

Rustic is an original browser engine in Rust. Read `docs/IMPLEMENTATION.md` for
current behavior, `docs/ROADMAP.md` for task dependencies, and
`docs/ARCHITECTURE.md` before changing component boundaries. A planned feature is
not implemented merely because it has a type, placeholder, crate or document.

The target platforms are Windows, macOS, Linux, Android and iOS/iPadOS. Keep the
shared engine independent of desktop event loops and OS-specific process/graphics
assumptions. Read `docs/MOBILE.md` before adding platform dependencies. Do not
substitute Mozilla/Firefox/Gecko, SpiderMonkey, Servo, Chromium/Blink, WebKit or an
existing JS runtime, including in mobile hosts.

## Code organization and ownership

- Keep modules focused on one responsibility. Share a helper when there is a real shared contract; avoid speculative frameworks and placeholder crates.
- Keep storage, tree mutation, traversal, parsing, policy and rendering boundaries explicit. Do not let parsers edit private node links or bypass resource accounting.
- Prefer private fields, checked constructors and small documented public APIs. Project crates are unpublished; do not promise stable embedding APIs prematurely.
- Preserve the owning document and slot generation on every node lookup. Never wrap identities, resurrect stale IDs, or treat an ID as an authorization capability.
- Validate an entire operation before changing observable state. Limit, hierarchy and reference failures must preserve the original tree and resource usage.
- Distinguish detach from destruction. Script-reachable detached nodes will require tracing; explicit engine destruction is not JS-visible DOM removal.
- Preserve UTF-16 code units for DOM values and future JS strings. Keep byte offsets, UTF-16 offsets and glyph/character indexes distinct.
- Avoid content-controlled recursion. Use bounded iterative algorithms and checked arithmetic before allocation.
- Maintain safe Rust and the declared minimum compiler. Do not add blanket lint suppressions to hide defects or compiler incompatibility.
- Add dependencies only for a concrete need after the documented boundary/license/security review. Do not embed an existing browser engine or JavaScript VM.

## Evidence and documentation

- Add focused regression and negative tests for actual behavior. For stateful structures, test invariants and compare against an independent model where useful.
- Test exact budget boundaries, overflow, invalid handles, cancellation and failure atomicity when relevant.
- Keep deterministic reference paths and fixtures for future optimized behavior. Expected-failure or golden-output updates require an explanation.
- Update the implementation ledger and affected architecture/feature docs in the same change. Record specification sections/revisions without claiming unrun conformance suites passed.
- Introduce an ADR for changed ownership/GC, authority/process boundaries, public API guarantees, major dependencies or engine scope.
- Maintain `progress/catalog.json` when a declared capability is actually completed, with source/test evidence. Read `docs/PROGRESS.md`; do not infer completion from placeholders, function counts or crate existence, or shrink the full-engine denominator to raise the score.

## Before committing a finished unit

Run the following from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --no-deps
python3 tools/check_repository.py
python3 -m unittest discover -s tools/tests -p 'test_*.py'
```

Use the platform's Python 3.11+ command if it is named `python`. Run affected
examples or integration harnesses too. GitHub Actions verifies the declared MSRV,
moving stable and the native OS matrix. Inspect CI for every pushed completion.
Commit coherent finished units with their tests and docs; push when the user has
authorized it. Preserve existing history and unrelated work. Do not claim that
code will never need refactoring; keep contracts clear and change them with
evidence when requirements become concrete.

Use `python3 tools/progress.py --write` to include current README assets locally;
successful default-branch CI also refreshes and pushes those generated files.
Verify the publishing job and follow-up CI whenever exercising automatic refresh.
