# ADR 0063 — macOS renderer sandbox

Date: 2026-11-21
Status: **Proposed** (implemented, unit-tested and type-checked for `aarch64-apple-darwin` and `x86_64-apple-darwin` on Linux; not yet run on a Mac)

## Context

Linux confines renderers with bubblewrap and seccomp (ADR 0058) and
Windows with a capability-less AppContainer (ADR 0060). macOS had no
renderer sandbox: `--sandbox` was an error and renderers ran with the
user's rights (TD-174). ISO-04 asks for a macOS mechanism with an ADR,
verified on native x64 and ARM64.

macOS confines processes with Seatbelt, configured by a profile in the
Sandbox Profile Language (SBPL). Chromium's and Firefox's macOS renderers
use it through `sandbox_init`/`sandbox_init_with_parameters`. The App
Sandbox (entitlements in a signed bundle) is the other option, but it
needs a code-signed application bundle and grants a fixed container
directory; it fits the M8 packaged browser better than a CLI binary run
from a build directory.

## Decision

1. **Seatbelt through `sandbox-exec`.** The broker starts each renderer as
   `/usr/bin/sandbox-exec -p PROFILE EXE __renderer` with a cleared
   environment (the same allowed test variables as other platforms) and
   `/` as its directory. `sandbox-exec` applies the profile and then
   executes the renderer, which inherits it permanently. This keeps the
   project's no-`unsafe` rule outside `rustic-sandbox`'s Windows module:
   calling `sandbox_init` would need FFI. `sandbox-exec` is marked
   deprecated in its manual page but ships with every macOS release
   Rustic targets, and it takes the same profile language.
2. **Deny by default** (`rustic_sandbox::seatbelt::renderer_profile`).
   Allowed: executing the renderer binary (and nothing else, no fork);
   reading that binary, dyld, `/usr/lib`, the system frameworks and the
   dyld shared cache in each location macOS versions use, and
   `/dev/urandom`; metadata of the few parent directories dyld walks; a
   named list of `sysctl` values the Rust runtime reads; signals to
   itself. Everything else is denied: networking, every file write,
   reading user files, other processes, Mach service lookups (no window
   server, pasteboard, keychain or launch services) and IOKit. The
   executable is named by its canonical path (Seatbelt matches resolved
   paths) as a quoted SBPL literal; a path that is not absolute UTF-8
   without control characters is refused.
3. **Opt-in until verified.** `--sandbox` (or `RUSTIC_SANDBOX=1`) uses the
   profile on macOS, and a profile that does not let the renderer start
   fails the command, as everywhere ("fail closed"). The default stays
   unconfined on macOS until a native run passes
   `the_sandbox_denies_files_network_and_processes_or_fails_closed`, which
   now checks, on a Mac with `sandbox-exec`, that every escape probe
   (read a user file, list a directory, write a file, connect to
   loopback, start a shell) works unconfined and fails under
   `--sandbox`, and that a real document still renders. Then
   `SANDBOX_AVAILABLE` includes macOS and this ADR becomes Accepted.
4. **The network process** (ADR 0061, TD-176) gets its own profile,
   `seatbelt::network_profile` (added 2026-11-22): the renderer profile
   plus `network-outbound` to IP addresses (Unix-domain sockets stay
   denied) and Mach lookups of the trust daemons (`com.apple.trustd`,
   `com.apple.trustd.agent`, `com.apple.SecurityServer`,
   `com.apple.ocspd`), which evaluate certificate chains out of process
   for the platform verifier, so no keychain file is opened. It is
   started the same way under `--sandbox` and verified by
   `the_seatbelt_network_process_reaches_only_internet_sockets`
   (read a file, list a directory, start a shell, connect to a Unix
   socket: allowed unconfined, denied confined) on the native macOS
   runners.

## Consequences

The profile is written without a Mac, so its system-library list may be
incomplete for some macOS version: the symptom is a renderer that cannot
start under `--sandbox` (a clean error, never an unconfined run), fixed by
adding the missing read-only path. A renderer has no parent-death signal
on macOS; it ends when its IPC pipe closes, and the watchdog stops a hung
one. `sandbox-exec` may be removed by a future macOS; the fallback is the
same profile through `sandbox_init` in `rustic-sandbox`'s audited
`unsafe` module, or the App Sandbox for the packaged browser (M8).

## Revisit

On the first native macOS x64 and ARM64 runs (accept, adjust the
profile, make it the default), if Apple removes `sandbox-exec`, and when
the M8 packaged application can use the App Sandbox.
