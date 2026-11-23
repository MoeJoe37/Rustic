# M8 desktop alpha completion review — 2026-11-23

## Starting revision

Reviewed `feature` from `d52291c` (M7 completion) through `25028db`, the
commit whose hosted CI run (45277636382) passed every job that got a runner.
That covers the native x64 and ARM64 Linux and Windows jobs and the macOS x64
job. It also covers the emulated 32-bit ARM, 32-bit x86, RISC-V and big-endian
runs, MSRV, the cross and Android compile checks, the Android emulator job, the
lavapipe GPU frame-time gate and parser fuzzing. The macOS ARM64 and iOS jobs
received no runner and were cancelled. The maintainer accepted merging without
them on 2026-11-23.

M8 ran as five parallel workstreams:
- renderer and GPU
- desktop hosts and shell
- accessibility
- DevTools and automation
- performance harness and platform services

## Scope change at closing

Two M8 capabilities depend on people or hardware the project does not have.
They move to M10 (beta) as external dependencies, as M7's outside security
review moved to `security.brave`:

- `compositor.parity`: byte-exact GPU parity and the lavapipe frame-time gate
  run in CI. Hardware frame times and energy need a machine with a GPU and
  power counters; no GitHub-hosted runner has either.
- `a11y.acceptance`: automated probes run through UI Automation,
  NSAccessibility and AT-SPI in CI. Acceptance needs screen-reader users
  (NVDA, Narrator, VoiceOver, Orca).

Neither move drops work. Both capabilities keep their completed steps and their
`external` descriptions.

## Delivered behavior

| Task | Result | Records |
| --- | --- | --- |
| UI-05 compositor | Scenes and scrolling, a confined `rustic __gpu` process with crash, hang and protocol recovery, a wgpu backend (Vulkan, Metal, Direct3D 12) with byte-exact CPU parity, device-loss reopen, window-surface recreation | ADR 0064, 0068, 0071 |
| Long-lived renderer | One renderer per page across frames, input routed to the page, scripts between frames | ADR 0069 |
| UI-01/UI-02 shell and hosts | `rustic view`: tabs, history, address entry, error pages, zoom, find, permission and download UI over the live page. Native windows on Linux (X11, Wayland), Windows and macOS, with fractional scales, IME, background tabs and a curated site corpus | ADR 0074 |
| UI-03 accessibility | Semantic tree, AccessKit bridges on all three desktops, assistive actions and editing flows | ADR 0073 |
| UI-04/UI-06 DevTools and automation | `rustic automate` (inspection, console, debugger, profiling, network diagnostics) over stdio with broker-enforced grants. The WPT runner drives it; the baseline is recorded | ADR 0072 |
| Platform services | DirectWrite and Core Text font lookup; Windows `Rustic.Gpu` AppContainer and macOS Seatbelt GPU profile | ADR 0078, TD-186 |
| Performance harness | Cross-engine benchmark harness, reference devices and published reports; GPU frame-time budgets | `perf.harness`, docs/PERFORMANCE.md |

## Gate

The roadmap gate is: critical accessibility flows, crash recovery, profile
separation, supported-platform safety gates and agreed performance budgets
pass, without claiming broad site compatibility.

- **Accessibility flows.** These pass in the native jobs of every desktop:
  `live_pages_answer_assistive_technology_and_ime_input`,
  `render_reports_the_tree_the_window_hands_to_the_platform_bridge`
  and the platform probes (`tools/a11y_probe_*.py`). Acceptance with real users
  moved to M10, as above.
- **Crash recovery.** Renderer recovery:
  `crashed_and_hung_renderers_are_replaced_then_the_page_gives_up`.
  GPU process recovery: `a_crashed_gpu_process_is_replaced_for_the_next_frame`
  and `a_lying_or_hung_gpu_process_is_cut_off_and_the_page_is_unchanged`.
  Device and surface loss:
  `the_hardware_compositor_matches_the_reference_and_recovers_a_lost_device`
  and `a_lost_window_surface_is_recreated_and_the_page_keeps_presenting`.
- **Profile separation.**
  `profiles_keep_local_storage_and_persistent_cookies_and_private_writes_nothing`.
  The shell releases its profile lock when a command ends (a8b81be). Dead-owner
  detection is TD-166, now due before the M10 beta.
- **Platform safety.** The M7 sandbox probes still pass. On every desktop
  runner the GPU process runs confined and must reach the same adapter as an
  unconfined one (`the_gpu_adapter_report_with_and_without_the_sandbox` with
  `RUSTIC_REQUIRE_CONFINED_GPU=1`):
  - lavapipe on Linux
  - WARP on Windows x64 and ARM64
  - paravirtual Metal on macOS x64

  `the_sandboxed_gpu_process_cannot_reach_files_network_or_programs` holds.
  DevTools has no remote endpoint
  (`there_is_no_remote_endpoint_and_bad_grants_fail`).
- **Performance budgets.** The lavapipe GPU frame-time gate is a required CI
  job. Hardware budgets are external (see above).
- **Site compatibility.** `the_curated_site_corpus_runs_in_the_desktop_window`
  is a small demo set, not a compatibility claim. M9 measures breakage.

## Native code review

Since M7, `unsafe` code has been added in three places:
- `rustic-native`'s DirectWrite and Core Text adapters (ADR 0078)
- the `Rustic.Gpu` container, which adds no new unsafe call: it reuses ADR
  0060's launcher
- the AccessKit platform adapters, which are third-party and pinned

`rustic-native` keeps every FFI call in its two adapter modules, behind
`#![allow(unsafe_code, reason = …)]`. The broker still reads and parses every
font file itself, within the ADR 0074 budgets, so a wrong answer from the
platform service cannot reach the parser unchecked. `rustic-gpu` has no
`unsafe` code. wgpu's validation sits between the GPU process and the driver.

## Debt

Paid in M8:
- TD-182 (long-lived renderer)
- TD-183 (macOS sandbox before main)
- TD-186 (GPU sandbox profiles)
- TD-196 (per-tab live pages)
- the remote-browsing row: TD-161, TD-165, TD-134, TD-111, TD-102, TD-116 and
  TD-94
- the platform-service part of TD-85

The two M8 rows of the [payoff schedule](../TECH_DEBT.md#payoff-schedule) were
not fully cleared. Each open entry moved to a later row in this change, with
the reason recorded in the entry:

- M9: TD-130, TD-152, TD-173, TD-178 (web APIs and browsing contexts);
  TD-46, TD-52, TD-53, TD-59, TD-177 (paint); TD-42, TD-51, TD-81, TD-85 (text
  and codecs).
- M10 (beta): TD-15, TD-166, TD-175, TD-191, TD-199.
- M12 (performance): TD-181, TD-184, TD-185.

TD-15, TD-130, TD-152 and TD-166 have now moved twice. Each move followed the
work that consumes them: DevTools parse diagnostics in M10, browsing-context
scripting in M9 and real-profile defaults in M10. They must not move again
without a design change.

## Next

M9 compatibility expansion
([ROADMAP §12](../ROADMAP.md#12-m9--compatibility-expansion-program)) and
the M10 external items. See
[ROADMAP §18](../ROADMAP.md#18-current-implementation-queue).
