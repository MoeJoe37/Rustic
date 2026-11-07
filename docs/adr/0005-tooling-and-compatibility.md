# ADR 0005: Reproducible Rust foundation and staged CI

Date: 2026-11-07. Status: accepted initial design.

## Context

An initial plan needs an executable repository without pretending unimplemented
subsystems exist. A moving compiler alone makes lints/results drift; a pin alone
can conceal incompatibility. Running nonexistent WPT/fuzz infrastructure would
create false confidence or permanently broken CI.

## Decision

Use a Rust 2024 workspace with two real crates, a committed lockfile, safe Rust
lints, pinned development Rust 1.99.1 and MSRV 1.85.0. CI runs quality/doc checks,
native std-only tests on Linux/Windows/macOS, compiler compatibility at MSRV and
stable, and repository checks. Read-only permissions, full-SHA action pins,
timeouts and no deployment keep initial automation narrow. Add upstream suites,
fuzzing, advisories and releases when their real components exist.

## Alternatives and consequences

Dozens of placeholder crates add maintenance without behavior. A plan-only repo
cannot validate Rust setup. Latest-only Rust is convenient but makes reproduction
hard; pin-only Rust misses drift. The chosen matrix costs runner time but the
dependency-free scaffold is small. A moving stable job can expose upstream lint
or compiler changes; only the pinned toolchain enforces the Clippy gate.

## Validation and revisit

Require first committed CI results before calling M0 verified. The stable `CI`
aggregate is suitable for later repository rules; adding YAML does not configure
branch protection. Review toolchain/MSRV increases deliberately as dependencies
arrive. Revisit the matrix if measured runner cost or platform needs justify it.
