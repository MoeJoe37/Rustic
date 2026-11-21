# ADR 0065 — Desktop window host (winit and softbuffer)

Date: 2026-11-21
Status: Accepted (maintainer decision 2026-11-21: winit + softbuffer); verified on Linux x86-64 under Wayland and X11 (XWayland); Windows and macOS untested until native runners return (TD-12)

## Context

M8 (UI-02) needs a native window, an event loop and a way to show the CPU
raster on Linux, Windows and macOS, with Android and iOS later. The trusted
browser shell (ISO-05 prompts and address display, TD-173) and the GPU
process both need a window. Writing Wayland, X11, Win32 and AppKit hosts
ourselves means a large amount of platform FFI and `unsafe`, which the
project forbids outside `rustic-sandbox`. [DEPENDENCIES.md](../DEPENDENCIES.md)
lists "a window/event library such as winit" as the desktop-host
candidate.

## Decision

1. **winit 0.30.13 and softbuffer 0.4.8**, pinned exactly, in a new crate
   `rustic-shell`. winit 0.30 is the current stable line (0.31 is in
   beta); softbuffer presents a CPU buffer on every winit platform. Both
   declare MSRV 1.70/1.71, and no package in the new graph needs more
   than Rustic's 1.85.
2. **Minimal features.** winit: `rwh_06`, `x11`, `wayland`,
   `wayland-dlopen`, `wayland-csd-adwaita-notitle` (client-side
   decorations without a title font, so no font crate); softbuffer: `x11`,
   `x11-dlopen`, `wayland`, `wayland-dlopen` (no KMS). Wayland and X11
   libraries are loaded at run time, so the binary starts on systems
   without them and fails with an error only when it opens a window.
3. **Boundary.** `rustic-shell` owns no web semantics: a `Content` trait
   paints a surface for a window size, device scale and scroll position,
   and the shell presents it (over white, as the page canvas), maps the
   wheel, arrows, Page Up/Down, space and Home to scrolling, and closes
   on the close button or Escape. Only `rustic-shell` and the CLI's
   `view` command depend on it; the engine crates never see winit.
4. **Isolation.** `rustic view` keeps the split: every frame is a `render`
   in a sandboxed renderer process at the window's size and scale, and
   the broker composites and hands the page back instead of writing a
   file. The window, display connection and input stay in the broker
   process, which a renderer never reaches. The renderer reports where it
   painted (`--report-scroll`); the broker passes only well-formed
   `scroll X,Y` lines through.
5. **Graph review.** 148 new packages (all targets; 66 on Linux) recorded
   in `third_party/rust-dependencies.json` with license notices. `dispatch`
   0.2.0 ships no license file anywhere; its manifest declares MIT, and
   the recorded notice says so. `r-efi` keeps its triple license in
   `AUTHORS`. BSD-2-Clause is allowed for `arrayref` (under tiny-skia,
   which draws Wayland decorations). Older majors that the window stack
   pins (rustix 0.38 and thiserror 1 under calloop, getrandom 0.3 under
   ahash, and macOS-, Android- and Redox-only duplicates) are listed in
   `deny.toml` with reasons. `cargo deny` passes.

## Consequences

The desktop host compiles for every target winit supports, but only Linux
is verified; Windows and macOS need their native runs. Each frame starts
a renderer and re-lays out the page (TD-182): fine for a first window, too
slow for a browser. The browser process now links platform windowing code,
which is attack surface outside the sandbox; it parses no web content.

## Revisit

When the long-lived renderer and the M8 shell UI arrive (persistent
renderer, incremental scenes per frame, input routed to the page), when
winit 0.31 is stable, and when the GPU backend (UI-05) replaces softbuffer
presentation.
