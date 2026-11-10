# Reviewed Rust dependencies

[rust-dependencies.json](rust-dependencies.json) pins the 24-package font adapter
graph and retained license digests. [ADR 0014](../docs/adr/0014-reviewed-font-adapters.md)
records the boundary and alternatives. The copies in `licenses/` preserve the
published package notices verbatim. Wuff omits its standalone license file from
the package, so its copy is taken from upstream revision
`0bb77cb3448100b8f560abf796014c8b85d75e23`, recorded in the manifest.

Changing a direct version or feature, a transitive checksum, or a notice requires
a deliberate review update. Run `cargo deny --locked check advisories licenses
bans sources` and the required workspace/platform checks. Rust dependency source
archives and audit-tool dependencies are not vendored here.
