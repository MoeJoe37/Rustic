# Reviewed Rust dependencies

[rust-dependencies.json](rust-dependencies.json) pins every third-party package
(278 in all) with retained license digests: the font adapter graph
([ADR 0014](../docs/adr/0014-reviewed-font-adapters.md)), the TLS graph and
`brotli-decompressor` for every target
([ADR 0051](../docs/adr/0051-transport-and-tls.md)), `windows-sys` for the
Windows sandbox ([ADR 0060](../docs/adr/0060-windows-renderer-sandbox.md)),
the desktop window graph of winit and softbuffer
([ADR 0065](../docs/adr/0065-desktop-window-host.md)) and the GPU compositor's
wgpu graph ([ADR 0071](../docs/adr/0071-gpu-backend.md)). Its `direct` table lists
each crate's direct dependencies with exact versions and features. `jni`, `jni-macros`, `jni-sys-macros` and
`rustls-platform-verifier-android` omit license files from their packages, so
their copies come from upstream at the `source_revision` recorded. The Public
Suffix List notice (MPL-2.0) belongs to vendored data
([ADR 0054](../docs/adr/0054-sites-and-cookie-policy.md)). The copies in `licenses/` preserve the
published package notices verbatim. `gpu-alloc`, `gpu-alloc-types`, `gpu-descriptor`, `gpu-descriptor-types`, `naga`,
`wgpu-core-deps-*`, `profiling`, `spirv` and `malloc_buf` omit license files
from their packages, so their copies come from upstream at the recorded
`source_revision` (`malloc_buf`'s license was added after its 0.0.6 tag; its
copy is from the default branch). `block` (MIT) and `hexf-parse` (CC0-1.0)
have no license text anywhere upstream; their `NOTICE` files record the
declared license. Wuff omits its standalone license file from
the package, so its copy is taken from upstream revision
`0bb77cb3448100b8f560abf796014c8b85d75e23`, recorded in the manifest.

Changing a direct version or feature, a transitive checksum, or a notice requires
a deliberate review update. Run `cargo deny --locked check advisories licenses
bans sources` and the required workspace/platform checks. Rust dependency source
archives and audit-tool dependencies are not vendored here.
