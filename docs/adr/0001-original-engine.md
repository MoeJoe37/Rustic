# ADR 0001: Own the engine and language runtime

Date: 2026-11-07. Status: accepted initial design.

## Context

The project is a new browser engine built from scratch in Rust. Embedding an
existing browser or JS runtime could produce a demo quickly but would change
what the project is building. Reimplementing TLS/crypto and OS drivers would add
large risk without advancing the engine algorithms.

## Decision

Rustic owns HTML, DOM, CSS, style, layout, paint semantics, JavaScript execution,
bindings, host scheduling and browser policy. No existing web engine or JS VM is
the execution implementation. Reviewed low-level libraries may provide TLS,
HTTP transport, Unicode/IDNA data, shaping/font parsing, codecs, databases,
serialization, platform windows and GPU access behind adapters. No candidate is
adopted solely because this record mentions its category.

## Alternatives and consequences

A browser-engine wrapper is faster but misses the goal. A temporary external
JS VM creates an integration shortcut and migration burden, so the local static
renderer arrives before Rustic's original JS integration. A zero-dependency
policy would require dangerous reinvention of cryptography and unrelated systems.
The chosen scope is still a multi-year program; publish partial support honestly.
JIT, WebAssembly and nonessential browser products remain later scope decisions.

## Validation and revisit

Dependency review checks for hidden renderer/VM substitution; conformance tests
exercise Rustic behavior. Revisit only if the maintainer explicitly changes the
project mission or a specific low-level component demonstrably needs a different
boundary. Preserve the existing AGPL license; scope decisions do not relicense it.
