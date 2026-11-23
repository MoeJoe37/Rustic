# ADR 0071 — GPU backend: wgpu inside the GPU process

Date: 2026-11-23
Status: Accepted (desktop backends; window surfaces with TD-185, mobile devices in M14)

## Context

[ADR 0068](0068-gpu-process.md) moved frame compositing into a confined,
stateless `rustic __gpu` child that runs the CPU reference compositor
(`Surface::add_frame`) and left the graphics API layer to this record
(UI-05, `compositor.backend`). Rustic must run on Windows (x64, ARM64),
macOS, Linux (x64, aarch64) and later Android and iOS, so the backend
needs Direct3D 12, Metal and Vulkan. Each of those is a native API reached
only through `unsafe` FFI, which the workspace forbids outside reviewed
crates.

Two properties from ADR 0068 must survive: a restarted GPU process holds
nothing stale, and the page is never affected by which compositor ran it
(the CPU path is the fallback for every failure, so the two must agree).

## Decision

1. **wgpu 26.0.1, pinned exactly** (`crates/rustic-gpu`), with only the
   `std`, `wgsl`, `vulkan`, `metal` and `dx12` features: no GL backend, no
   SPIR-V/GLSL front ends, no tracing. wgpu is the WebGPU implementation
   Firefox ships, so its validation layer is hardened against hostile
   input, and it covers every target in one API. 26 is the newest major
   that builds with the workspace MSRV (1.85); 27 and later need a newer
   compiler. Every package of its graph is pinned with checksums and
   retained notices in `third_party/rust-dependencies.json`; `rustic-gpu`
   itself has no `unsafe` code.
2. **Byte-exact parity by construction.** A WGSL compute shader runs the
   reference compositor's integer arithmetic (one invocation per pixel of
   a layer's rectangle, pixels packed as little-endian `u32`s), so the GPU
   output equals the CPU reference byte for byte. The parity gate compares
   exact bytes, never a tolerance.
3. **One device per GPU process, opened on first use.** `RUSTIC_GPU`
   chooses: unset uses hardware adapters only (a software Vulkan or WARP is
   slower than the CPU reference), `any` allows software adapters (CI),
   `cpu` composites on the CPU. No adapter means CPU compositing, not an
   error. Buffers live for one composition, so a new process or device
   starts empty.
4. **Device loss.** A lost device (driver reset, removal) is reopened and
   the same composition retried once; a process reopens at most
   `MAX_DEVICE_OPENS` (3) devices, then composites on the CPU for the rest
   of its life. Any other GPU error (allocation refused, too large to bind)
   composites that frame on the CPU. Validation and device errors are
   caught through error scopes; none can panic the process. Crashes and
   hangs inside the driver are ADR 0068's process recovery.
5. **Linux confinement for drivers.** The GPU process keeps the renderer's
   bubblewrap namespaces and gains read access to what Mesa and vendor
   drivers probe (`/dev/dri` device nodes, `/sys/dev/char`,
   `/sys/devices`, `/sys/bus/pci`, `/etc/vulkan`) and a seccomp profile
   that adds `memfd_create` and `ftruncate` (anonymous memory files for
   driver allocations) to the renderer allowlist. It still has no network,
   no home directory, no writable file system and cannot start programs.
6. **Windows and macOS stay on the renderer profiles for now.** The
   confined GPU process uses the `Rustic.Renderer` AppContainer and the
   Seatbelt renderer profile, which grant no driver services; there the
   device may fail to open and the confined process composites on the CPU
   (`--no-sandbox` reaches the hardware). GPU-specific profiles are TD-186.

## Alternatives considered

- **Own adapters per API (`windows`, `metal`, `ash` crates).** Three
  `unsafe` FFI surfaces to write and review instead of one maintained by a
  browser vendor, for a compositor that needs one compute pass.
- **A render pipeline with blending.** Fixed-function blending rounds
  differently per vendor, so parity would need a tolerance and the CPU
  fallback would change pixels.
- **Newer wgpu with a higher MSRV.** Raising the MSRV is a separate
  decision; 26 has the backends and fixes this work needs.
- **Software adapters by default.** lavapipe and WARP work everywhere but
  are slower than the reference compositor; they serve CI and opt-in use.

## Consequences

- The GPU process links large native drivers. A driver crash or hang kills
  the GPU process only, and ADR 0068 replaces it; the broker validates
  every layer before it crosses, and the device validates again.
- Pixels still cross the pipe twice per composition (TD-184), so the GPU
  path is not yet faster end to end; performance and energy gates
  (`compositor.parity`) are measured once shared memory lands.
- Frame layers are composited on the GPU; the page itself is still
  rasterized on the CPU in the renderer.

## Validation

`crates/rustic-gpu/tests/parity.rs`: odd sizes, overlapping layers,
partial and zero coverage and frames smaller than their rectangles composite byte-identically to the CPU
reference; bad layers are refused; a lost device reopens.
`crates/rustic-cli/tests/gpu.rs`
(`the_hardware_compositor_matches_the_reference_and_recovers_a_lost_device`):
through the sandboxed GPU process the output equals `RUSTIC_GPU=cpu`, and
a device lost mid-run is reopened. CI installs lavapipe on Linux x64 and
ARM64 and sets `RUSTIC_REQUIRE_GPU=1`, so both gates must find an adapter
and cannot pass by skipping. The seccomp unit test checks the GPU filter
adds only anonymous memory files.

## Revisit trigger

GPU sandbox profiles for Windows and macOS (TD-186), shared-memory
surfaces (TD-184), presenting the window surface from the GPU process
(TD-185), GPU rasterization of page content, a wgpu major upgrade or
MSRV change, or mobile GPU adapters (M14).
