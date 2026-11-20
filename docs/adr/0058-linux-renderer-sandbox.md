# ADR 0058 — Linux renderer sandbox (bubblewrap)

Date: 2026-11-20
Status: Proposed (implemented as opt-in `--sandbox`; making it the default and adding a seccomp filter need the maintainer's decision)

## Context

ISO-03 requires the Linux renderer to have no direct filesystem, network
or device access, enforced by the operating system rather than by the
broker protocol alone. The workspace forbids `unsafe` code, so calling
`unshare`, `seccomp`, `landlock` or `prctl` from Rust needs either a
reviewed crate (for example `landlock`, `seccompiler` or `rustix`) or an
external launcher. Chromium and Flatpak use namespaces plus seccomp-bpf;
Flatpak launches through bubblewrap.

## Decision

1. **Launch the renderer through bubblewrap** (`bwrap`, LGPL-2.0+, a
   setuid-free tool shipped by every major distribution and used by
   Flatpak). No Rust dependency is added; the broker runs
   `bwrap … /renderer __renderer`.
2. **Confinement:** `--unshare-all` (user, mount, PID, network, IPC, UTS
   and cgroup namespaces: the renderer has an empty network namespace and
   sees only its own processes), `--cap-drop ALL`, `--new-session`
   (no `TIOCSTI` injection into the user's terminal), `--die-with-parent`,
   `--clearenv` (only test-hook variables are passed), and a filesystem of
   read-only `/usr` with `/lib` and `/lib64` links plus the executable at
   `/renderer`: no `/home`, `/etc`, `/tmp`, `/proc` or `/dev`. Fonts and
   documents arrive over IPC (ADR 0056), so the renderer needs no files.
3. **Fail closed.** `--sandbox` (or `RUSTIC_SANDBOX=1`) without a usable
   bubblewrap, or a sandbox that cannot start, is an error; the renderer
   is never started unconfined instead. `--sandbox` excludes
   `--single-process`.
4. **Watchdog.** The broker kills a renderer that does not finish within
   `--renderer-timeout` seconds (default 120), confined or not.
5. **Verification** is behavioral: a compromised-renderer probe tries to
   read `/etc/passwd`, list the home directory, write `/tmp`, connect to a
   listening loopback socket and run `/bin/sh`; all are denied under
   `--sandbox` and all succeed in the unconfined control run. Every CLI
   suite and the pinned WPT selection pass with `RUSTIC_SANDBOX=1`.

## Not yet (TD-172)

No seccomp-bpf filter restricts system calls (a kernel bug reachable from
the remaining syscalls is not mitigated); `/usr` is readable; the sandbox
is opt-in. Windows and macOS have no sandbox (ISO-04), and no native
aarch64 Linux run exists.

## Revisit

To make `--sandbox` the default, to add seccomp (a BPF program passed to
`bwrap --seccomp` needs file-descriptor passing, so a reviewed crate or
unsafe code), and when ISO-04 starts.
