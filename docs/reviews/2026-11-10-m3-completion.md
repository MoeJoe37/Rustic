# M3 static renderer completion review — 2026-11-10

## Starting revision

Reviewed `feature` and `main` at
`5dfaf56c9e35c9895eccd48749358210e26cfa35` (tree
`e06e3dda99ac34de70bb6aa9f5658db52cfc8aad`). The latest commit fixes a rustdoc
link to private `TILE_SIZE`; its other changes refresh the generated progress
fingerprint. The change is correct and the final warning-denied rustdoc gate
passes. Prior progress commits use `progress/catalog.json` plus
`tools/progress.py --write`; this completion follows that same workflow.

The sandbox reset discarded an earlier unpushed implementation. This workspace
was rebuilt from all 818 baseline blobs with exact Git blob hashes verified,
and the baseline tree hash matched before development resumed. No stale source
snapshot was published. The evidence below is from this rebuilt workspace.

## Delivered behavior

- CFF/CFF2, WOFF1/WOFF2, bounded atomic discovery of host-granted local font
  roots, and explicit CLI `--font` grants feeding computed metrics and rendering.
- GSUB/GPOS/kern, Arabic joining, Indic reordering, whole-grapheme fallback,
  logical source clusters, glyph offsets through layout to paint, bounded runs,
  invalid-value failures and cooperative cancellation.
- Scoped CSS bidi embeddings/overrides/isolates/plaintext projected to original
  indexes, with HTML dir/bdi/bdo rules and plaintext paragraph alignment.
- CSS Text 3 wrapping/strictness/emergency sizing, discretionary soft hyphens,
  pinned English automatic hyphenation, DOM language inheritance and explicit
  refusal of unsupported Level 4 phrase segmentation.

[TEXT.md](../TEXT.md) describes the contracts and bounds.
[ADR 0014](../adr/0014-reviewed-font-adapters.md) reviews the three direct font
libraries and exact 24-package graph, including licenses and temporary-memory
limits. Rustic retains its own engine algorithms and forbids unsafe code in
its crates. No browser engine, JS runtime, native font FFI, implicit font reads
or page network authority is added.

## Sandbox merge gate

All listed commands passed on Linux x86-64. Hosted GitHub CI is not the merge
gate, following the maintainer's explicit instruction to validate in the sandbox
while hosted jobs are unavailable.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Pass |
| Rust 1.99.1 workspace all-target tests | 745 passed; two full-data tests run separately below |
| Rust 1.85.0 MSRV workspace all-target tests | 745 passed |
| Moving stable workspace all-target tests (1.99.1) | 745 passed |
| Workspace doctests | 19 passed |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --no-deps` | Pass |
| Repository consistency checks | Pass |
| Python tooling unit tests | 53 passed |
| `cargo deny --locked check advisories licenses bans sources` | All four checks pass; no ignored advisories |
| Independent native HarfBuzz 10.2.0 comparison | 30 glyph/advance/offset cases agree; offline Rust expectations pass |
| Unicode generator `--check` with exact pinned inputs | Pass; existing full segmentation fixtures retained |
| Full Unicode 17 bidi files with `RUSTIC_UCD_DIR` and `--ignored` | Both full-data tests passed |
| All 17 mutation targets, seed 1 | 20,000 mutations each plus all seeds; 340,000 mutations total, no finding |
| Cross compilation | All 20 target/scope pairs below pass |

The unchanged line-break/grapheme fixtures cover all 19,338/766 cases. CSS parsing
scope expands by 42 supported-property cases to 1,609 of the unchanged 2,916-case
source corpus; 1,241 pass with no expected-failure edits. This is a parsing gate,
not a full CSS Text layout conformance claim. New targeted tests cover selected
hyphen widths, CJK language distinctions, source-node joining, invisible controls,
mark positions, malformed wrappers and discovery failure atomicity.

The cross checks run `cargo check --locked --workspace --all-targets --target`
for these 13 triples:

```text
aarch64-unknown-linux-gnu
i686-unknown-linux-gnu
armv7-unknown-linux-gnueabihf
riscv64gc-unknown-linux-gnu
s390x-unknown-linux-gnu
arm-unknown-linux-gnueabihf
aarch64-unknown-linux-musl
aarch64-pc-windows-gnullvm
i686-pc-windows-msvc
powerpc64le-unknown-linux-gnu
loongarch64-unknown-linux-gnu
x86_64-unknown-freebsd
x86_64-unknown-linux-musl
```

The seven mobile checks use `--workspace --lib`:

```text
aarch64-linux-android
armv7-linux-androideabi
x86_64-linux-android
i686-linux-android
aarch64-apple-ios
aarch64-apple-ios-sim
x86_64-apple-ios
```

These checks prove compilation, not native target execution, device rendering,
signing, runtime platform services or acceptance.

## Render review

The committed M3 article fixture exercises an inline PNG, nested blocks, flex
cards, English automatic hyphenation, Arabic/Latin isolation, Indic text,
clipping and resizing. It was rendered and visually inspected at 720×720/zoom 1
and 400×1000/zoom 1.25. The cards reflow, the swatch paints, glyphs remain readable,
and the narrow overflow line clips at its box. Fonts were explicitly granted:
Noto Sans Regular plus the pinned renamed Arabic/Devanagari fixtures. The runtime
outputs are scratch artifacts, not self-derived golden expectations.
See [DEVELOPMENT.md](../DEVELOPMENT.md#local-static-renderer-demo-with-explicit-fonts)
for reproduction. Original swatch source is `tools/generate_m3_swatch.py`.

## Progress and debt

Only the four remaining M3 text capabilities change to complete. Existing step
evidence is preserved and augmented with source/test markers; the moved wrapping
evidence now names `tailoring::apply`. No capability identity, label, size,
milestone assignment, subsystem weight, or denominator changes.

| Metric | Before | After |
| --- | --- | --- |
| M3 declared catalog | 13/17, 91.32% | 17/17, **100.00%** |
| Overall weighted engine progress | 31.66% | **32.72%** |
| Verified complete + partial | 29.95% + 1.71% | 32.50% + 0.22% |
| Whole-engine capabilities | 49/172 complete, five in progress | 53/172 complete, one in progress |

`tools/progress.py --write` regenerates the README block and SVG/JSON/Markdown
assets; `--check` verifies freshness. The complete engine, native M8 acceptance
and broad M9 text/writing-mode/conformance work remain open in the original
catalog. The earliest unfinished catalog milestone becomes M4.

TD-42/47/50 retain their historical notes with paid portions identified. New
TD-85 records backend temporary/cache accounting, advanced fonts and native
services; TD-86 records cross-node optional ligatures, bounded context, selected
break reshaping and carets; TD-87 records locale resources, SA/phrase segmentation
and automatic-hyphen remainder policy. Each entry has a payoff trigger. M3
completion does not erase those limits or imply full browser compatibility.

## Publication

Publish the complete source and regenerated evidence through `feature`, with
its head checked against the reviewed baseline. Fast-forward `main` to that same
sandbox-tested commit without force. Inspect both refs and hosted run status
following publication; hosted job availability must not be reported as a passing
native OS/device test matrix.
