# ADR 0004: Gate remote browsing on isolation

Date: 2026-11-07. Status: accepted design; OS mechanisms remain unimplemented.

## Context

Documents, scripts, fonts and images are attacker-controlled. Rust does not
prevent policy mistakes or vulnerabilities in native dependencies. Building all
processes before any document pipeline would delay a testable vertical slice.

## Decision

Allow a single-process trusted-fixture harness first, then an explicit allowlist
network prototype with disposable profiles. Define service/capability interfaces
early. Before arbitrary remote browsing, enforce renderer/broker separation,
bounded validated IPC and the supported platform's sandbox. Authority comes from
the browser and is checked again at the service boundary. Failure to establish
required isolation blocks remote mode rather than silently reducing protections.

## Alternatives and consequences

Single-process remote browsing has an unacceptable authority surface. A nominal
process-per-tab model does not by itself isolate cross-origin frames or settle
agent semantics. Full multiprocess infrastructure on day one is expensive before
interfaces are understood. Staging accepts refactoring cost but keeps prototypes
clearly labeled and prevents tests from becoming accidental safety claims.
Linux, Windows and macOS require independent feasibility and enforcement evidence.

## Validation and revisit

Test hostile renderers, stale epochs, handle forgery, forbidden OS operations,
cross-site/profile access, crashes and sandbox launch failure. Record gates per
OS. Revisit process mapping as agent/frame requirements mature, without weakening
the requirement that remote browsing enforce the declared authority boundaries.
