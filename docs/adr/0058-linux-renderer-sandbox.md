# ADR 0058 — Linux renderer sandbox (bubblewrap)

Date: 2026-11-20
Status: Accepted 2026-11-21 (on by default with a seccomp-bpf allowlist; verified natively on Linux x86-64, and on the native Linux ARM64 CI runner with bubblewrap on 2026-11-22 at `9961c1c`)

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
3. **Fail closed, on by default** (amended 2026-11-21). Every split
   command confines its renderer unless `--no-sandbox` or
   `RUSTIC_SANDBOX=0` opts out explicitly; `--single-process` has no
   renderer to confine, and `--sandbox` with it is a usage error. A
   missing bubblewrap, an architecture without a system-call table, or
   a sandbox that cannot start is an error; the renderer is never started
   unconfined instead.
6. **System-call allowlist** (amended 2026-11-21, pays TD-172).
   `rustic_sandbox::seccomp` builds a classic-BPF program that bubblewrap
   installs with `--seccomp`: about 75 calls the renderer was measured to
   need (descriptor I/O, memory, threads, signals, clocks, randomness,
   one `execve`), `clone` without `CLONE_NEW*` flags and `ioctl` without
   `TIOCSTI`/`TIOCLINUX`; everything else fails with `ENOSYS` (sockets
   included), and other ABIs (i386, x32) kill the process. The workspace
   still forbids `unsafe`: `/bin/sh` opens the program as descriptor 3
   (`exec 3<file; rm file; exec bwrap --seccomp 3 …`), so std never needs
   to pass a descriptor. x86-64 and aarch64 have tables; other
   architectures need `--no-sandbox`.
4. **Watchdog.** The broker kills a renderer that does not finish within
   `--renderer-timeout` seconds (default 120), confined or not.
5. **Verification** is behavioral: a compromised-renderer probe tries to
   read `/etc/passwd`, list the home directory, write `/tmp`, connect to a
   listening loopback socket and run `/bin/sh`; all are denied under
   `--sandbox` and by default (the socket call by the filter, with `ENOSYS`,
   before the empty network namespace is reached) and all succeed in the
   unconfined `--no-sandbox` control run. Every CLI suite and the pinned
   WPT selection pass with the default sandbox. The allowlist was
   measured with `strace -f` over the CLI suites and is checked by an
   interpreter of the generated program in unit tests.

## Not yet

`/usr` is readable (TD-175); Windows has its own sandbox (ADR 0060) and
macOS none (TD-174); no native aarch64 Linux run exists (TD-12), so the
aarch64 table is checked only by unit tests.

## Revisit

When the renderer gains threads, GPU or font services that need new
system calls (extend the allowlist with a trace, never fall back to a
deny list), and when a native aarch64 runner exists.
