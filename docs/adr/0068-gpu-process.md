# ADR 0068 — The GPU process: display compositing out of the broker

Date: 2026-11-22
Status: Accepted (step 1: process boundary, protocol, confinement and recovery with the CPU compositor; hardware backends are a later record)

## Context

M8 (UI-05) needs a GPU process, and `compositor.recovery` (moved from
M7's ISO-07 by [ADR 0067](0067-mobile-devices-last.md)) asks that a
crashed GPU process recovers without stale authority. Today the broker
composites itself: a renderer sends its page surface with frame layers,
and `Broker::composite` draws each out-of-process frame's pixels into it
(ADR 0062). Every browser engine moves this work out of the browser
process, because GPU drivers are large native code reachable from content
(through layer geometry, pixel data and, later, WebGL/WebGPU) and crash or
misbehave more often than anything else in the browser.

Rustic has no GPU backend yet, and the choice of graphics API layer
(wgpu, or Direct3D 12/Metal/Vulkan adapters of its own) adds a large
dependency and `unsafe` boundary that needs its own record. The process
boundary does not depend on that choice: what crosses it is the
compositor's work, not a graphics API.

## Decision

1. **A `rustic __gpu` child composites.** In split runs the broker sends
   each page surface that has frame layers, with the frames' pixels, to a
   GPU process started on first use, and receives the composited surface.
   The child runs the same reference compositor (`Surface::add_frame`) the
   broker used, so results are byte-identical; a hardware backend replaces
   it inside the child later. Pages without frame layers need no
   compositing and do not cross. Single-process runs composite in place.
2. **Stateless protocol** (`rustic_ipc::gpu`, role `Gpu`). The broker
   sends `GpuCommand::Composite { id, page, frames }`, where `page` is the
   renderer's surface with its layers and `frames` the pixels the layers'
   handles name, renumbered so handle `n` is `frames[n]`. The child replies
   `GpuEvent::Composited { id, width, height, pixels }` or
   `GpuEvent::Failed { id }`. Nothing persists in the child between
   commands, so a restarted process has nothing stale to reuse; texture
   caching arrives with the hardware backend and must keep this property
   (a new process starts empty).
3. **The broker keeps every decision.** It validates the renderer's
   surface, resolves handles from its own table and checks each layer's
   geometry before anything is sent; the GPU process never sees a URL,
   grant, origin or handle table. A reply must have the request's id, the
   page's size and premultiplied pixels; anything else is a protocol
   violation that kills the process.
4. **Confinement.** The GPU process holds no authority and is confined
   like a renderer: bubblewrap with every namespace unshared and the
   renderer seccomp filter on Linux, the `Rustic.Renderer` AppContainer on
   Windows, the Seatbelt renderer profile on macOS (`RUSTIC_SEATBELT=gpu`).
   A setup failure is an error that falls back to in-process compositing,
   never an unconfined GPU process. Hardware access (device nodes, driver
   services) is added per platform by the backend record, as narrowly as
   the driver allows.
5. **Recovery and software fallback.** A GPU process that crashes, hangs
   up, breaks the protocol, or takes longer than an eighth of the renderer
   timeout (between 1 and 15 s; the parent renderer is waiting) is killed; the frame it was compositing is
   composited by the broker on the CPU (the same reference code, so the
   page is unaffected) and the next frame starts a fresh process. At most
   `MAX_GPU_STARTS` (4) processes start per run; after that, or when one
   cannot start, the run composites on the CPU, as browsers fall back to
   software compositing when the GPU process keeps failing. The broker
   prints one line per stopped process and one when it falls back.

## Alternatives considered

- **Wait for the hardware backend.** The process boundary, recovery and
  sandbox are independent of the API layer and are what M7's recovery item
  needs; landing them first keeps the backend change small.
- **Let renderers talk to the GPU process directly.** Needs shared memory
  or descriptor passing (not available without `unsafe` on every platform
  yet) and would let a renderer name another renderer's pixels. Routing
  through the broker keeps handles broker-owned; direct channels can come
  with shared memory (TD-170) under a later record.
- **Fail the command when the GPU process dies.** A GPU crash would then
  cost a page, which browsers avoid; the CPU path is exact, so falling
  back costs only time.

## Consequences

- A compromised GPU process can show wrong pixels in a composited page
  but cannot change their shape, read files or the network, or reach other
  documents' grants; the same holds in every engine, which display what
  the GPU process draws.
- Each composited frame copies the page and its frames over a pipe. That
  is acceptable for the CLI and the M8 alpha; shared memory removes it
  (TD-184).
- `rustic view` still composites in the broker until the window surface is
  presented by the GPU process (TD-185).

## Validation

`crates/rustic-cli/tests/gpu.rs`: pages with frames composite through the
GPU process with the same pixels as in-process compositing; a crashing,
garbage-sending or wrong-size GPU process is replaced and the output is
unchanged; one that always crashes stops being restarted after
`MAX_GPU_STARTS`; under the sandbox the GPU process cannot read files,
list directories, write, connect to loopback or start programs. The
`ipc-frame` fuzz target decodes both GPU message directions.

## Revisit trigger

The GPU backend record (Direct3D 12, Metal, Vulkan; the "GPU abstraction
and backends" entry of the ADR index), shared-memory surfaces, or a
renderer that rasterizes on the GPU.
