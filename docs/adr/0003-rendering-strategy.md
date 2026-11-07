# ADR 0003: Headless CPU reference first

Date: 2026-11-07. Status: accepted initial design.

## Context

Layout, text, paint order and platform graphics failures are hard to diagnose
together. Early GUI/GPU integration can obscure whether the engine understands
the document at all. Cross-platform font differences can destabilize tests.

## Decision

Build local headless DOM/style/layout dumps and an immutable display list before
a desktop browser. Implement a deterministic CPU display-list interpreter with
bounded surfaces and pinned test fonts as the reference path. Font shaping and
codec adapters may reuse reviewed low-level implementations. Add GPU/compositor
execution behind the same semantic boundary after static rendering is correct.

## Alternatives and consequences

A GPU-first approach can deliver fast primitives but increases setup, driver and
reproducibility problems. Rendering HTML through a system WebView would hide the
engine and is rejected. CPU-first rendering may be slower and requires a real
raster implementation; it provides inspectable fixtures and a fallback backend.
Text shaping/bidi remain real requirements, even when early debug output is basic.

## Validation and revisit

Use structural dumps, reftests, exact primitive pixels and documented per-test
tolerances. Compare GPU output to the reference path and test device loss.
Revisit the backend boundary if profiling shows a specific representation blocks
performance; preserve observable paint semantics and a useful reference path.
