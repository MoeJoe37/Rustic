# Performance and measurement plan

There are no engine performance results yet. Budgets below are initial
engineering targets to calibrate after M3; they are not user-facing promises or
measurements. Correctness and security gates remain prerequisites to optimization.

Beating Chrome and Firefox in measured speed, memory and energy is a required
product target. [PRODUCT.md](PRODUCT.md) defines initial 10% aggregate improvement
targets and p95/correctness guardrails. Record exact browser versions, native
engines, feature parity and both default/equivalent-blocking configurations.
Results must include uncertainty and regressions; no present performance or
universal superiority claim is implied by the target.

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
| Navigation lifecycle | Repeated open/close reaches a stable resource plateau; no unexplained retained DOM/listener graphs | M5/M6 |
| Memory/resource caps | Per-renderer, decoded image/font, network queue and profile limits have explicit failure behavior | Establish measured defaults M3–M7 |
| Regression triage | Investigate sustained >10% time or memory regression on stable fixture/hardware; require explanation or fix | After stable baseline |

A 16.7 ms frame is not a promise that every document can meet it. Hardware and
fixture definitions are part of the target. Define memory caps from real peaks
and failure tests rather than inventing a universal browser memory number now.

## Optimization order

1. Build a correct, observable full-recompute/CPU reference path.
2. Remove proven algorithmic problems and avoid repeated parsing/copying.
3. Introduce invalidation, dirty subtrees and cache bounds with parity tests.
4. Reuse text shaping, images, styles and display-list data with explicit keys.
5. Add partial layout/painting and compositor scrolling where semantics permit.
6. Parallelize immutable independent work after profiling demonstrates benefit.
7. Evaluate GPU and language optimizations; JIT needs a separate security ADR.

Do not parallelize live DOM mutation or page JS by moving shared graphs behind
locks. Document agents serialize observable mutation; workers have defined agents
and messaging. Cancellation and epochs prevent optimization work for stale pages
from consuming unbounded resources or replacing current frames.

Use sampling profiles and allocation traces before changing architecture. Candidate
tools include platform profilers, Rust benchmark harnesses, and heap diagnostics;
select/pin tooling when the harness lands. Store benchmark definitions and reports,
and keep an unoptimized reference implementation long enough to validate complex
fast paths. Any unsafe/SIMD/FFI optimization needs an explicit invariant review.
