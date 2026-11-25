# Performance and measurement plan

Comparative results so far are the
[first cross-engine JavaScript report](reviews/2026-11-19-perf-01-m5.md)
(PERF-01, M5 slice: the JS-09 corpus against V8, SpiderMonkey and
JavaScriptCore shells on x86-64) and the
[M8 reference-device report](reviews/2026-11-23-perf-01-m8-reference-devices.md)
(six hosted x64 and ARM64 desktops, with Rustic's page pipeline budgets);
there are no browser page-load or memory comparisons yet. The first internal
rendering measurement is the `frame_budget` raster example (a one-box repaint took 3.5 ms
at scale 1 and 11 ms at scale 2 in the sandbox, [PAINT.md](PAINT.md)); it is a
sanity check, not a controlled benchmark. Budgets below are initial engineering
targets to calibrate; they are not user-facing promises or measurements.
Correctness and security gates remain prerequisites to optimization.

Beating all three established engines (Blink in Chrome, Gecko in Firefox and
WebKit in Safari) in measured speed, memory and energy is a required product
target and the M12 gate. [PRODUCT.md](PRODUCT.md) defines the 10% targets and
guardrails; [cross-engine gates](#cross-engine-gates) below defines the
benchmarks, devices and the ratchet that tracks the gap from M5 to M12. Record
exact browser versions, native engines, feature parity and both
default/equivalent-blocking configurations. Results must include uncertainty
and regressions; no present performance or universal superiority claim is
implied by the target.

## Measurements and corpus

Measure each stage independently and end to end: byte decoding, tokenization,
tree building, CSS parse/matching/cascade, layout, shaping, display-list building,
raster/composition, JS execution/GC, loading and navigation. Track wall time,
CPU time, peak/resident memory, allocations, decoded-resource bytes, task latency,
cache sizes/hit rates and canceled work. Use median and p95 where repeated trials
are meaningful; record distributions instead of one best result.

At M3 establish versioned fixtures: tiny page, text-heavy article, large DOM,
selector-heavy stylesheet, deep nesting, image-heavy page, mixed Arabic/Latin
text, resize, scrolling and repeated navigation. Add scripted updates and network
fixtures with M5/M6. Benchmark input size scaling as well as a single document;
unexpected quadratic growth is a defect even if the smallest fixture is fast.

Record engine commit, Rust version, build flags, machine/CPU/OS, backend, scale,
font hashes, fixture revision, cold/warm state, sample count and tool versions.
Separate download time from CPU render time. Avoid shared hosted-runner timing as
a hard release gate until variance is understood; use a stable controlled runner
for small regression thresholds.

## Initial budget proposals

| Workload | Initial target / rule | Calibration point |
| --- | --- | --- |
| Warm simple-page frame update | p95 within 16.7 ms at 60 Hz on named reference hardware | M3/M5 |
| Input responsiveness | No engine-owned continuous task over 50 ms without a documented checkpoint strategy | M5 |
| Viewport surface | M0 accepts at most 16,777,216 pixels and 16,384 per dimension; future allocator also checks byte/stride limits | Implemented configuration; allocator M3 |
| Script runaway | Bounded instruction/wall-time execution; chrome remains responsive | M4/M7 |
| JS heap | Every JS allocation charged to a per-agent budget; exhaustion is a catchable error or clean termination, never a process abort | M4 (JS-03) |
| Navigation lifecycle | Repeated open/close reaches a stable resource plateau; no unexplained retained DOM/listener graphs | M5/M6 |
| Memory/resource caps | Per-renderer, decoded image/font, network queue and profile limits have explicit failure behavior | Establish measured defaults M3–M7 |
| Regression triage | Investigate sustained >10% time or memory regression on stable fixture/hardware; require explanation or fix | After stable baseline |

A 16.7 ms frame is not a promise that every document can meet it. Hardware and
fixture definitions are part of the target. Define memory caps from real peaks
and failure tests rather than inventing a universal browser memory number now.

## JavaScript runtime

The JS engine is planned for low memory and fast interpretation from its first
value type; [ROADMAP §7](ROADMAP.md#memory-and-performance-design) lists the
design commitments (8-byte values, `u32`-indexed arenas, shapes with inline
caches, Latin-1/UTF-16 strings and ropes, register bytecode with lazy
compilation, frames off the Rust stack, budgeted mark/sweep). JS-09 adds the
measurements that keep them honest:

- **Heap-size report.** A deterministic test prints the bytes the engine
  charges for representative values: an empty object, an object with four
  properties, a 1,000-element dense array of small numbers, a 1,000-character
  Latin-1 string and its UTF-16 counterpart, a closure capturing one variable,
  and a compiled function per bytecode instruction. The report is committed;
  a change that grows any line needs an explanation in the same change.
- **Benchmark corpus.** Original, licensed-for-redistribution workloads in the
  repository: property access (monomorphic and polymorphic), calls and
  recursion, closure creation, string building, array push/iterate/sort,
  object-allocation churn (GC pressure), JSON-like tree building and a
  DOM-free "framework diff" loop. Each reports median and p95 time, peak heap
  and GC count and total pause.
- **Comparison.** For the PRODUCT.md targets, the same corpus runs on V8,
  SpiderMonkey and JavaScriptCore in their interpreter-only modes and with JIT, with exact
  versions recorded. Those engines are measured from outside the repository
  only; they are never linked or embedded (ADR 0001).
- **Initial targets** (to calibrate once JS-04 runs code): no workload is
  quadratic in its input size; peak heap for the corpus within the
  interpreter-only V8 figure; GC pause p95 under 5 ms for a 32 MiB heap on the
  reference ARM64 and x64 machines; collection at every allocation (stress
  mode) changes no observable result.

Measure on native x64 and ARM64; the emulated 32-bit and big-endian tiers check
correctness only, never timing.

**Status (JS-09).** The heap-size report, the corpus and its runner exist and
are gated by `crates/rustic-js/tests/limits.rs`; [JS.md](JS.md#limits-and-performance-gate-js-09)
has the recorded x86-64 results. Against the initial targets: no workload is
quadratic (a test doubles the input and bounds the growth of steps and of
bytes allocated); stress mode changes no result; the longest collection pause
is about 0.17 ms per MiB of live heap, so it meets the 5 ms target up to
roughly 30 MiB live and misses it above (TD-95, paid by `perf.gc`). Still
open: the ARM64 run on a native machine, and the comparison against V8,
SpiderMonkey and JavaScriptCore, which belongs to the cross-engine harness
below (PERF-01).

## Cross-engine gates

These are the measurements behind the M12 [performance-leadership gate](ROADMAP.md#15-m12--performance-leadership).
The harness (PERF-01, catalog `perf.harness`) starts with M5 and grows as
features land; a benchmark Rustic cannot run yet is reported as "not runnable",
never as zero or skipped silently.

### Benchmarks

| Area | Benchmark (pinned revision) | Metric |
| --- | --- | --- |
| Web app responsiveness | [Speedometer 3](https://browserbench.org/Speedometer3.0/) | Score (geometric mean of subtests) |
| JavaScript and Wasm | [JetStream 2](https://browserbench.org/JetStream/) | Score; also Rustic's JS-09 corpus |
| Graphics | [MotionMark 1.3](https://browserbench.org/MotionMark1.3/) | Score |
| Page load | Recorded site corpus served by a local replay server, cold and warm | First contentful paint, largest contentful paint, load complete |
| Interaction | Scripted clicks, typing and navigation on the site corpus | p50/p95 input-to-next-paint latency |
| Scrolling | Long pages, sticky/fixed content, image-heavy feeds | Dropped and late frames |
| Memory | Fixed set of 10 tabs with equivalent retained state | Peak and steady resident/private memory |
| Energy | 30-minute scripted browsing loop on battery | Energy (mWh) from platform counters or external meters |
| Startup | Cold and warm launch; new tab | Time to first paint |

Rustic's own fixtures are original or licensed for redistribution. Recorded
sites use local replay; no live site or personal account is measured.

### Reference devices

**CI reference devices (M8).** The [Performance workflow](../.github/workflows/perf.yml)
runs weekly and on demand on six GitHub-hosted runners, which are the named
reference devices for the desktop families until physical machines exist:

| Device | Runner image | Family and architecture | Engines compared |
| --- | --- | --- | --- |
| Linux x64 | `ubuntu-24.04` | Linux, x86-64 | Rustic, V8 (the image's Node) |
| Linux ARM64 | `ubuntu-24.04-arm` | Linux, aarch64 | Rustic, V8 (Node) |
| Windows x64 | `windows-2022` | Windows, x86-64 | Rustic, V8 (Node) |
| Windows ARM64 | `windows-11-arm` | Windows, ARM64 | Rustic, V8 (Node) |
| macOS ARM64 | `macos-15` | macOS, Apple silicon | Rustic, V8 (Node), JavaScriptCore (system `jsc`) |
| macOS x64 | `macos-15-intel` | macOS, Intel | Rustic, V8 (Node), JavaScriptCore (system `jsc`) |

Each job records the CPU model, logical cores, memory, OS release and the
exact engine versions in its report (`perf-<device>` artifact, Markdown and
JSON), and the report job merges every device into one page (`perf-report`
artifact and the run summary). Hosted runners are shared virtual machines:
their numbers name a device class, they are not controlled hardware, so they
never gate a merge. SpiderMonkey has no shell on these images and is "not
compared" there; the [first report](reviews/2026-11-19-perf-01-m5.md) measured
it on a controlled x86-64 host. Physical reference machines (a Windows ARM64
laptop, an Apple-silicon laptop, an aarch64 board with fixed clocks) and
browser-hosted comparisons with Chrome, Firefox and Safari are an external
step: they need hardware the project does not have yet.

Each class below must exist by the stage that needs it:

| Platform family | Reference classes |
| --- | --- |
| Windows | x64 desktop; ARM64 laptop |
| macOS | ARM64 (Apple silicon) laptop; x64 (Intel) machine while supported |
| Linux | x64 desktop; aarch64 board or server |
| Android | ARM64 mid-range phone; ARM64 low-end phone; 32-bit ARM device for correctness and memory only |
| iOS/iPadOS | Current iPhone; older supported iPhone or iPad |

Comparison engines: current stable Chrome (Blink/V8), Firefox
(Gecko/SpiderMonkey) and Safari (WebKit/JavaScriptCore). Where Safari does not
run (Windows, Linux, Android), WebKit is measured through the WebKitGTK/WPE
reference browser where one exists and recorded as "not compared" otherwise. On
iOS outside regions that allow other engines every competitor uses WebKit; the
report says so. Emulated 32-bit and big-endian tiers check correctness only.

### Ratchet

Each stage compares Rustic with the **best** of the three engines on each
metric, on every reference device where Rustic runs at that stage. Ratios are
Rustic ÷ best engine for times and memory, and best ÷ Rustic for scores, so
lower is better and 1.0 is parity.

| Stage | Speed (benchmarks, load, interaction) | Memory | Energy |
| --- | --- | --- | --- |
| M5 scripted documents | Harness runs; ratios recorded, no target | Recorded | Not yet measured |
| M8 alpha | At most 2.0 | At most 1.5 | Recorded |
| M10 beta | At most 1.25 on every benchmark | No worse than the median engine | No worse than the median engine |
| M12 leadership | At most 0.90 (10% better than the best) | At most 0.90 of the lowest | At most 0.90 of the lowest |

Additional M12 rules: 95% confidence intervals must not overlap the best
engine's; no subtest may be worse than the best engine by more than 5% without
a reviewed, published exception; p95 interaction latency is never worse than the
best engine. A stage gate missed on one device class blocks the stage. Results
are published per device and engine; an aggregate never hides a lost metric.

## M8 budgets

The desktop alpha adds page-level budgets measured by the page pipeline
benchmark (`crates/rustic-raster/examples/page_bench.rs`): five generated
fixtures, each parsed, styled, laid out, painted and rasterized at 1280×720
with the bundled test font. The budget is the median of 11 runs of the whole
pipeline on **every** CI reference device; `tools/cross_engine_bench.py`
(`M8_PAGE_BUDGETS_MS`) holds the same table and the Performance workflow fails
the device's job when one is missed.

| Fixture | What it stresses | M8 budget (median, ms) | First measurement (sandbox x86-64, 2026-11-23) |
| --- | --- | --- | --- |
| `page_tiny` | Start-up cost of every stage | 16.7 (one 60 Hz frame) | 3.8 |
| `page_article` | 200 wrapped paragraphs: shaping and line breaking | 250 | 123 |
| `page_large_dom` | 5,000 block elements | 600 | 244 |
| `page_selectors` | 600 descendant, attribute and `:nth-child` rules on 2,400 elements | 300 | 131 |
| `page_deep` | 400 nested blocks | 50 | 11.7 |

The budgets are about twice the first measurement so that shared runners
pass with room for their variance; they ratchet down as each device's
history shows its spread. A stage that grows by more than 10% on a device for
two consecutive runs needs an explanation, per the regression rule above.
Other M8 budgets are the existing ones: the warm frame update (the
`frame_budget` example's damage-limited repaint within 16.7 ms), the
50 ms task rule and the per-renderer memory caps of ISO-04.

**Where the M8 ratchet stands.** The [first reference-device run](reviews/2026-11-23-perf-01-m8-reference-devices.md)
put every device within every page budget above. On the JavaScript corpus the
ratio to the fastest JIT engine (V8, or JavaScriptCore on macOS) is 3.7 to 29
on the hosted x64 and ARM64 devices and up to 52 on the older Intel Mac, so
the 2.0 speed ratchet for M8 is missed on every workload. The workflow
records it per device, as the ratchet requires; closing the gap is the M12
interpreter and JIT work (`perf.interpreter`, `perf.baseline_jit`), which an
interpreter-only alpha cannot reach.

### GPU frame budgets

The required CI job gates byte-exact parity and frame time on lavapipe.
Hardware frame timing and energy remain external steps of `compositor.parity`
for the M10 beta. The GPU frame benchmark
(`crates/rustic-gpu/examples/gpu_frame_bench.rs`) composites two generated
scenes the way the GPU process does: it uploads the page and frame layers,
dispatches, and reads the result back. The CPU reference
(`Surface::add_frame`) composites the same scene on the same device. A
GPU sample counts only if its pixels match the reference. Scenes:
`frames_720p` is a 1280×720 page with an ad-sized frame and a video-sized
one; `frames_1440p` is a 2560×1440 backing store (device scale 2) with four
frames, two of them overlapping. `tools/cross_engine_bench.py --gpu` measures
11 runs and applies the budgets (`GPU_*_BUDGETS_MS`, `GPU_CPU_RATIO`).

| Metric | Hardware adapter | Software adapter (lavapipe, WARP) | First measurement (sandbox lavapipe, x86-64, 2026-11-23) |
| --- | --- | --- | --- |
| `frames_720p.gpu` median | 16.7 ms (one 60 Hz frame) | 33.4 ms | 5.7 ms |
| `frames_1440p.gpu` median | 16.7 ms | 200 ms | 49.4 ms |
| GPU ÷ CPU reference, each scene | at most 2.0 | at most 2.0 | 0.68 and 1.01 |

The required CI job **GPU frame-time gate (lavapipe)** runs the software
column on every push, with `--enforce-gpu`. It is the regression gate: the
software budgets are about four times the first measurement, so the job
tolerates shared-runner noise. The ratio row is independent of how fast the
runner is. The Performance workflow adds the same measurement to every
reference device's report but does not block on it.

The hardware column and energy per frame need a machine with a hardware GPU
and power counters. No GitHub-hosted runner has either: the Linux runners
have no GPU, Windows composites on WARP, and the macOS virtual machines have
no power-metrics access. Hardware frame times and energy are therefore an
external dependency of `compositor.parity`, recorded as such in the progress
catalog. They are measured with the same benchmark on a self-hosted or
contributor machine (`gpu_frame_bench` prints the adapter it ran on), and
energy with the platform's counters (RAPL or `powermetrics`).

## Optimization order

1. Build a correct, observable full-recompute/CPU reference path.
2. Remove proven algorithmic problems and avoid repeated parsing/copying.
3. Introduce invalidation, dirty subtrees and cache bounds with parity tests.
4. Reuse text shaping, images, styles and display-list data with explicit keys.
5. Add partial layout/painting and compositor scrolling where semantics permit.
6. Parallelize immutable independent work after profiling demonstrates benefit.
7. Evaluate GPU and language optimizations; JIT needs a separate security ADR.
8. Add JIT tiers, concurrent GC and parallel style/layout (M12) only behind the
   reference path, with differential tests against it.

Do not parallelize live DOM mutation or page JS by moving shared graphs behind
locks. Document agents serialize observable mutation; workers have defined agents
and messaging. Cancellation and epochs prevent optimization work for stale pages
from consuming unbounded resources or replacing current frames.

Use sampling profiles and allocation traces before changing architecture. Candidate
tools include platform profilers, Rust benchmark harnesses, and heap diagnostics;
select/pin tooling when the harness lands. Store benchmark definitions and reports,
and keep an unoptimized reference implementation long enough to validate complex
fast paths. Any unsafe/SIMD/FFI optimization needs an explicit invariant review.
