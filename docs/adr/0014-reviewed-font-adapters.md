# ADR 0014: Reviewed low-level font adapters

Date: 2026-11-10. Status: accepted within ADR 0001's low-level library boundary.

## Context

M3 requires CFF/web-font formats and OpenType layout with Arabic and Indic
behavior. The existing original TrueType parser and Unicode algorithms do not
provide these. Implementing production GSUB/GPOS, CFF and WOFF2 from scratch
would duplicate specialist work and broaden the untrusted-input surface.

## Decision

Use exact **HarfRust 0.15.0** (`std`), **Skrifa 0.49.0** (`std`) and
**Wuff 0.2.10** (`brotli`), with default features disabled. HarfRust provides
GSUB/GPOS/kern and script shapers. Skrifa provides unhinted CFF/CFF2 outlines.
Wuff reconstructs WOFF/WOFF2; WOFF1 DEFLATE uses Rustic's existing bounded codec.
Wuff's pure safe Rust Brotli backend is enabled; its native/unchecked backends
are disabled. No font download, filesystem service, existing browser engine or
JavaScript runtime is imported. The Rust 1.85 MSRV remains unchanged.

Rustic retains face matching, per-grapheme fallback, CSS metrics, Unicode 17
segmentation/bidi, CSS policies, source indexes, layout and painting. Font bytes
are shared with lazy backend caches. Containers are preflighted against table,
input, transformed-data and reconstructed-data bounds before decompression.
Output outline points are capped, with iterative cubic subdivision. Shape input,
returned glyphs and backend runs are bounded; cancellation is polled between
runs. Temporary backend allocations and caches are not a process-wide memory
quota (TD-85). Fonts are local files granted explicitly by the host.

The 24-package registry graph, exact checksums, enabled direct features, upstream
source revisions and retained notices are recorded in
[the reviewed manifest](../../third_party/rust-dependencies.json). The repository
checker rejects unreviewed graph, feature and notice drift. `cargo-deny 0.20.2`
checks advisories, licenses, duplicate versions and sources locally and in the
new audit workflow. No advisory is ignored. MIT, Apache-2.0, BSD-3-Clause,
Unicode-3.0 and Zlib notices are retained alongside the existing AGPL license.
Procedural macros run at build time. Reviewed transitive zero-copy internals
include unsafe code; Rustic's own crates continue to forbid unsafe Rust.

## Alternatives

Rustybuzz is archived in favor of HarfRust; its older layout implementation would
not be the maintained choice. Native HarfBuzz/FreeType would require FFI and
platform deployment work. Native HarfBuzz 10.2.0 is used solely as an independent
test oracle through Python tooling, with 30 committed glyph/position cases.
A second complete font parser replacing the existing validated TrueType path
would unnecessarily change that path's contracts. General platform discovery
libraries would introduce authority and native services beyond this local slice.

## Validation and revisit

Format/outline equivalence, GSUB/GPOS/Arabic/Indic tests, hostile mutations,
exact resource failures, source-cluster invariants, native-reference comparison,
workspace checks, MSRV/stable and cross-platform compilation gate adoption.
The audit command must pass before publication. Larger shaping corpora and
native device acceptance remain the existing M9/M8 gates. Revisit for dependency
updates, a security advisory, glyph-cache pressure, collection/variation/color
font support, or native font service integration. TD-85/86/87 record these limits.

### Accepted update (2026-11-14)

Use exact Skrifa 0.50.0 and HarfRust 0.16.0 with their existing `std`
features and defaults disabled. They share `read-fonts` 0.47.0.
The build-script, platform, resource, MSRV and authority boundaries of
this decision continue to apply. The release notices and checked package
identities are recorded in the reviewed manifest.
