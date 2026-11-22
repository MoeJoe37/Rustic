# M7 isolation and security completion review — 2026-11-22

## Starting revision

Reviewed `feature` from `807d771` (M6 completion) through `e150a22`, the
commit whose hosted CI run (45026285903) passed every job: native x64 and
ARM64 Linux, Windows and macOS, the emulated 32-bit ARM, 32-bit x86, RISC-V
and big-endian runs, MSRV, cross and mobile compile checks, and the Android
emulator job. This change closes M7 for the scope that
[ADR 0067](../adr/0067-mobile-devices-last.md) leaves in it.

## Scope change at closing

On 2026-11-22 the maintainer put Android and iOS device work aside until the
end of the plan. ADR 0067 moved `security.mobile` (Android on a physical
device, iOS on the simulator and on a device with Apple's browser-engine
entitlement) to the new last milestone M14, the GPU process crash step to
`compositor.recovery` (M8, where the GPU process is built), and the
independent external review to `security.brave` (M10), recorded there as an
external dependency. Nothing in those items was finished and dropped: the
Android design and emulator steps keep their credit under M14, and the review
packet is ready for outside reviewers.

## Delivered behavior

| Task | Result | Records |
| --- | --- | --- |
| ISO-01 process model | Process keys, COOP group switches, ORB and COEP; iframes and popups in renderers of their own | ADR 0057, 0062 |
| ISO-02 IPC and authority | Every document command runs in a `__renderer` child over bounded versioned IPC; the broker answers file, network, `fetch()` and storage requests from its own grant | ADR 0056 |
| ISO-03/04 desktop sandboxes | Linux bubblewrap + seccomp, Windows LPAC AppContainer + job, macOS Seatbelt; on by default, failing closed, green natively on x64 and ARM64 | ADR 0058, 0060, 0063 |
| ISO-05 permissions | User activation, brokered downloads, trusted strip with address display and permission prompt in `rustic view` | ADR 0057 §6 |
| ISO-06 review | Fuzz targets for every untrusted input, unsafe/native inventory, AddressSanitizer run, renderer compromise tests, external review packet | [security review](2026-11-20-m7-security-review.md), [packet](2026-11-22-m7-external-review-packet.md) |
| ISO-07 recovery | Crashed, protocol-breaking and hung renderers end their command; network process in its own confined child, replaced on crash; remote origins reach only public addresses | ADR 0061, 0066 |
| ISO-08 mobile | Design (ADR 0059) and the Android isolated-process renderer on the emulator; device and iOS work is M14 | ADR 0059, 0067 |

## Gate

The roadmap gate is "all applicable security gates pass with recorded
evidence and focused external review where available".

- **Sandboxes.** `the_sandbox_denies_files_network_and_processes_or_fails_closed`
  and the network-process probes (`the_sandboxed_network_process_reaches_only_internet_sockets`,
  `the_windows_network_process_is_confined_for_remote_origins`,
  `the_seatbelt_network_process_reaches_only_internet_sockets`) pass on the
  native jobs of all three desktop systems; a bad sandbox setting is a usage
  error, never a silent unsandboxed launch.
- **Hostile peers.** `a_compromised_renderer_cannot_reach_beyond_its_local_grant`,
  `..._network_grant` and `a_compromised_network_process_is_cut_off_by_the_broker`.
- **Recovery.** `crashed_and_protocol_breaking_renderers_end_the_command_cleanly`,
  `a_hung_renderer_is_stopped_by_the_watchdog`,
  `a_crashed_network_process_fails_its_requests_and_is_replaced`,
  `a_network_process_that_crashes_every_time_cannot_loop`.
- **Site separation.** `opaque_response_blocking_and_coep_keep_other_sites_out_of_the_renderer`.
- **External review.** Not available yet. Under the gate's "where
  available" clause M7 closes without it; remote browsing still needs an
  explicit `--allow-origin` or `--allow-remote` grant, and the review is a
  condition of the M10 beta (`security.brave`).

## Debt

The two M7 rows of the [payoff schedule](../TECH_DEBT.md#payoff-schedule)
were not cleared. They moved to M8: "before remote browsing is on by
default" (TD-111 and the other budget-bounded costs) and "M8 browser shell"
(TD-15, TD-130, TD-152, TD-166). Each entry records the move. TD-12's
Android and iOS runs moved to the M14 row.

## Next

M8 desktop alpha, starting with the GPU process (UI-05, which also carries
the GPU crash step) and the long-lived renderer per page (TD-182). See
[ROADMAP §18](../ROADMAP.md#18-current-implementation-queue).
