# Contributing to Rustic

Rustic has a working static renderer (M1–M3) and is building its JavaScript
engine (M4). Start with the [documentation index](docs/README.md), the
[architecture](docs/ARCHITECTURE.md), the [roadmap](docs/ROADMAP.md) (its
current queue lists the next tasks) and the [development commands](docs/DEVELOPMENT.md).
Existing core browser or JavaScript engines must not be introduced as hidden
implementation shortcuts.

## Make a reviewable change

Work lands through the `feature` branch and is merged into `main` once every
check passes ([branches and merging](docs/DEVELOPMENT.md#branches-and-merging)).
Keep each change focused. State the observable problem
and resulting behavior, relevant roadmap task, standards section/revision, in/out
scope, and test evidence. Include a reduced regression fixture when changing
behavior. Security-sensitive issues follow [SECURITY.md](SECURITY.md), rather
than a public exploit report. Do not put credentials or private browsing data in
issues, fixtures, logs or screenshots.

Run the local checks in the development guide. Add negative/resource-limit tests
for untrusted inputs and update feature status honestly. Preserve the correct
reference path when adding incremental/optimized behavior. Do not rewrite test
expectations just to hide failures. Every change must keep all [platform tiers](docs/PLATFORMS.md)
green, including 32-bit and big-endian targets; native changes need tests on the
affected platform.

Use an [ADR](docs/adr/README.md) for changes to engine scope, ownership/GC,
execution, authority, public APIs, or major dependencies. Keep internal APIs
private until proven. Create component crates when needed, not as placeholders.
Default project code remains safe Rust; exceptions require an explicit reviewed
boundary and invariants. New dependencies follow the
[dependency policy](docs/DEPENDENCIES.md).

The maintainer decides scope and merges. There is no CLA or automated publishing
flow established by this initial setup. Preserve the existing license and notices
in contributed or imported material.
