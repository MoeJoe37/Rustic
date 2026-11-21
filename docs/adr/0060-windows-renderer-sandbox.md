# ADR 0060 — Windows renderer sandbox and the first project `unsafe` code

Date: 2026-11-21
Status: Accepted for the M7 prototype (verified on native Windows 11 x64; ARM64 waits for a native runner, TD-12)

## Context

ISO-04 requires the Windows renderer to have no direct file, registry,
network or process access, enforced by the operating system
([SECURITY.md](../SECURITY.md#isolation-rollout)). The threat is code
execution in a renderer that parses hostile content: it must not read the
user's files, reach local services (SMB, development servers, other
browsers' debugging ports) or the internet, or start other programs.

Windows offers restricted tokens with integrity levels, AppContainers, Less
Privileged AppContainers (LPAC), job objects and process mitigation
policies. All of them are reached only through Win32 calls that take raw
pointers. Rustic's crates forbid `unsafe` (SECURITY.md: an exception needs
an ADR and a narrow, separately reviewed adapter), and there is no
external launcher comparable to bubblewrap on Windows.

## Decision

1. **A new crate, `rustic-sandbox`,** holds operating-system confinement.
   It is the only project crate whose lints *deny* rather than *forbid*
   `unsafe_code`, and only its Windows module allows it (with a reason).
   Clippy's `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block`
   are on, so every block performs one operation and states why it is
   sound. Its Linux part (the seccomp program generator of ADR 0058) is
   safe Rust and has no dependency.
2. **Binding:** `windows-sys` 0.61.2, already in the locked graph through
   `rustls-platform-verifier` and `schannel` (MIT OR Apache-2.0, Microsoft,
   generated declarations only), built for Windows targets only, with the
   eight feature modules the launcher calls. No new package enters
   `Cargo.lock`.
3. **Confinement:** the renderer starts in the `Rustic.Renderer`
   AppContainer **with no capabilities and as an LPAC**
   (`PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT`), so objects that
   admit every AppContainer do not admit it. It is created suspended with
   only its three standard pipe ends in an explicit inherited-handle list,
   no console (`DETACHED_PROCESS`), child-process creation restricted, the
   system directory as working directory and an environment of
   `SystemRoot`, `LOCALAPPDATA` (needed by `CreateProcessW` for container
   processes) and the test hooks only. It is then assigned to a job that
   kills it when the broker's handle closes, admits one active process and
   ends it on an unhandled exception, and only then resumed.
4. **Executable access:** the broker grants the container SID read and
   execute on the renderer executable with `icacls` from the system
   directory (not `PATH`) once per run. A broker that may not change the
   executable's ACL fails.
5. **On by default** on Windows and Linux; `--no-sandbox` or
   `RUSTIC_SANDBOX=0` is the explicit opt-out, `--single-process` has no
   renderer to confine. Every failure is an error; nothing falls back to
   an unconfined renderer.

## Verification

`crates/rustic-cli/tests/isolation.rs` runs a compromised-renderer probe
that reads a file next to the document, lists the user's directory,
writes a file there, connects to a loopback listener and starts
`cmd.exe`. In the unconfined control run all five succeed; with
`--sandbox` and by default all five fail. With a plain AppContainer the
loopback connection *succeeded* on Windows 11 23H2 with the firewall on
(the internet was blocked): loopback isolation is not reliable for
unpackaged containers. Under LPAC, Winsock cannot initialize at all, so
the renderer has no sockets. Every workspace test that runs the CLI runs
its renderer in the container.

## Alternatives considered

- **Restricted token plus low integrity (Chromium's renderer token).**
  Stronger against some kernel objects but much more code (token
  construction, desktop and window-station handling) and still needs
  AppContainer or firewall rules for networking. Revisit with win32k
  lockdown in M8 hardening.
- **Plain AppContainer.** Rejected: loopback stayed reachable (above).
- **A reviewed sandbox crate.** None is maintained for this use with an
  acceptable dependency graph; the launcher is under 600 lines.
- **An external launcher program.** Windows has none in the base system.

## Consequences

- Project code now contains `unsafe`, confined to one module that only
  calls Win32. A memory-safety bug there is in the broker, the most
  trusted process; reviews of `rustic-sandbox/src/windows.rs` are
  security reviews.
- The renderer cannot use the network, the registry outside its
  container, or files other than the system libraries and its own
  executable. Fonts and documents already arrive over IPC (ADR 0056).
- Not yet: win32k system-call lockdown (needs no GDI/user32 in the
  renderer, which is true today), a dynamic-code prohibition mitigation
  (blocks a future JIT, PERF-03), and a native ARM64 run (TD-12).

## Revisit

When ISO-04 adds macOS (TD-174), when the GPU process needs a
different policy, before a JIT (dynamic code policy), and when a native
Windows ARM64 runner exists.
