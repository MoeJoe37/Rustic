# M3 anonymous flow normalization checkpoint — 2026-11-08

Continues `feature`/`main` at `7b6660b7fd6aba2380863147b2f03bb12aaca684`.
The maintainer's sandbox-gated workflow remains in effect: validate locally,
push the exact checked source through `feature`, then fast-forward merge it
into `main` without rewriting history. Hosted Actions is inspected separately.

## Implemented

REN-01c derives a borrowed `FormattingTree` from the existing principal snapshot.
It wraps inline runs beside blocks in minimal anonymous blocks and splits inline
ancestors around nested block groups. Leading/trailing empty continuations stay,
with original start/end edge flags for later decoration layout. Atomic inline-
blocks normalize internally without allowing their blocks to escape. Principal
source indexes retain validated DOM identity and raw UTF-16; formatting parents
can differ from DOM parents without mutating either source snapshot or live DOM.

Anonymous blocks inherit supported inherited values from the enclosing non-
anonymous source, and reset other values. Text gets anonymous inline style, not
its parent's background/borders. Existing computed-value machinery resolves
initial currentcolor and border widths; registered custom properties respect
inheritance flags. Only direct normal/nowrap whitespace-only boundary runs are
omitted, with their source indexes reported and original units still retained.
Other whitespace processing and all text metrics remain pending.

Flat arenas and independent box/depth/work limits bound generated continuations
and repeated sequence visits, with cancellation and atomic errors. This is not
a linear-time or total-process-memory claim (TD-39). Unsupported displays,
positioning/floats, replaced/control/foreign content fail explicitly. Flattened
shadow input and pseudo boxes remain pending (TD-37). No external dependency or
browser engine was introduced; the fuzz crate adds only a workspace layout edge.

`dump-formatting` exposes this structural stage through the existing bounded
local HTML/stylesheet loader. Existing `dump-boxes` and `dump-layout` fixtures
stay unchanged. Used mixed inline/block geometry, fonts/shaping, line boxes,
general fragments and painting remain unfinished. `layout.block`'s anonymous
used-flow step stays planned; M3 is not complete.

## Evidence

- Sixteen normalization scenarios cover minimal wrappers, nested/multiple/empty
  splits, atomic inline-blocks, inheritance/currentcolor/custom registrations,
  boxless/hidden/template input, non-Latin/lone-surrogate preservation,
  unsupported content, exact budgets/cancellation and bounded deep expansion.
- An independent token/block-mask model checks 508 short trees (all masks through
  six tokens, zero through three inline ancestors), plus homogeneous children,
  preorder parents and original text-order invariants.
- Three CLI tests cover the reviewed structural fixture at chunks 1/7/65536,
  exact output/normalization limits, local stylesheets, failure without stdout
  and consistency between help defaults and library budgets.
- The new `layout-formatting` fuzz target mutates HTML/styles and checks source
  text partition/order, UTF-16 preservation, style presence and child roles.
  The required all-target smoke picks it up; the nightly matrix includes it.

Specification basis: CSS 2.2 §9.2.1.1–9.2.2.1, CSS Display 3 anonymous boxes and
CSS Text 3 white-space processing, reviewed 2026-11-08. See [LAYOUT.md](../LAYOUT.md)
for the exact subset and references. No layout-WPT or browser-comparison pass is
claimed from these structural fixtures.

## Progress graph

Reviewed commits `7b6660b` and `71a2590` and followed their existing catalog/
generation workflow. Only the existing normalized-box step of `layout.boxes`
advances, with source and test evidence. All 172 capabilities, 21 subsystems,
weights, sizes and step definitions stay intact. `layout.boxes` now has two of
three steps; general fragments remain planned. No whole capability advances.

`python3 tools/progress.py --write` regenerates the marked README block and
SVG/JSON/Markdown assets; `--check` validates their final source fingerprint.
The SVG was rendered and visually inspected. Overall progress moves from
**21.42% to 21.61%** (20.27% verified complete + 1.34% verified partial steps).
M3 moves from **7.59% to 9.19%**. Completed capabilities stay **36/172**.

## Sandbox validation

- Formatting, denied-warning Clippy and rustdoc, all workspace doctests, CLI
  fixtures and `rustic info`.
- **487 all-target tests per compiler:** pinned Rust 1.99.1, MSRV 1.85.0 and
  moving stable (1.99.1).
- **39 Python tooling tests**, repository checks and generated progress checks.
- **10 fuzz targets**, each replaying its seeds and running 20,000 seed-1
  mutations with 10-second per-input timeout; no findings. The new formatting
  target replayed 1,799 seeds.
- **20 cross-compilation checks:** thirteen non-mobile all-target checks and
  seven Android/iOS library checks, matching the preceding review's modes and
  [target list](2026-11-08-m3-block-flow.md#completed-sandbox-checks).

Native execution here is Linux x86-64. Cross compilation does not replace
native Windows/macOS/ARM64/QEMU/mobile runtime evidence. The maintainer has
authorized these sandbox results as the merge gate while hosted CI is unavailable.
