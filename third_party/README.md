# Reviewed Rust dependencies

[rust-dependencies.json](rust-dependencies.json) pins the 24-package font adapter
graph ([ADR 0014](../docs/adr/0014-reviewed-font-adapters.md)) and the 53-package
TLS graph for every target ([ADR 0051](../docs/adr/0051-transport-and-tls.md)),
with retained license digests. `jni`, `jni-macros`, `jni-sys-macros` and
`rustls-platform-verifier-android` omit license files from their packages, so
their copies come from upstream at the `source_revision` recorded. The Public
Suffix List notice (MPL-2.0) belongs to vendored data
([ADR 0054](../docs/adr/0054-sites-and-cookie-policy.md)). The copies in `licenses/` preserve the
published package notices verbatim. Wuff omits its standalone license file from
the package, so its copy is taken from upstream revision
`0bb77cb3448100b8f560abf796014c8b85d75e23`, recorded in the manifest.

Changing a direct version or feature, a transitive checksum, or a notice requires
a deliberate review update. Run `cargo deny --locked check advisories licenses
bans sources` and the required workspace/platform checks. Rust dependency source
archives and audit-tool dependencies are not vendored here.
