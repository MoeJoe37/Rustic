# Platform and device support

**Rustic must run on any device that can run a browser.** That means every
mainstream operating system on every CPU family that ships browsers today:
phones, tablets, laptops, desktops, single-board computers, Chromebooks,
TVs/kiosks and servers, on x86-64, x86 (32-bit), ARM64, ARM (32-bit) and newer
architectures such as RISC-V and LoongArch. Windows, macOS, Linux, Android and
iOS/iPadOS remain the five release platforms (ADR 0006); this policy extends
that commitment to every CPU those systems run on and to other systems with a
Rust standard library. See [ADR 0013](adr/0013-any-capable-device.md).

## Engineering rules

These apply to every change, in every crate:

- **Portable by default.** Shared engine code must compile and behave the same
  on 32- and 64-bit pointers, little- and big-endian byte order and every
  instruction set listed below. Do not assume `usize` is 64 bits, that memory
  is plentiful, or that the host is x86.
- **Explicit widths.** Use fixed-width integers for persisted, hashed or
  serialized data and checked conversions (`try_from`) at `usize` boundaries.
  Use `to_le_bytes`/`from_be_bytes`-style APIs; never reinterpret memory.
- **Bounded memory.** Budgets must fail cleanly before an allocation can exceed
  a 32-bit address space. Tests must not need gigabytes of memory; prove limits
  with budgets instead.
- **Arch-specific code is optional and isolated.** SIMD or other CPU-specific
  fast paths need a portable fallback with the same results, tests for both,
  and runtime or `cfg` selection in one narrow module.
- **OS code lives in adapters.** Windows, macOS, Linux, Android and iOS
  integration stays in platform adapters (see [MOBILE.md](MOBILE.md)); the
  shared core uses only `core`/`alloc`/`std` interfaces available everywhere.
- **No silent platform loss.** A change that drops a target from CI needs an
  ADR update explaining why and how it returns.

## Android and iOS: deferred, not dropped (2026-11-22)

[ADR 0067](adr/0067-mobile-devices-last.md) moves Android and iOS device
work (physical devices, the iOS simulator and Apple's browser-engine
entitlement, native mobile hosts and mobile shipping) to M14, the last
milestone, after the desktop engine 1.0. They remain release platforms and
overall progress cannot reach 100% without them. What stays in force now:

- the **Mobile check** tier below and the Android emulator isolation job
  remain required by the aggregate `CI` job;
- the engineering rules above apply to every change, so the shared core keeps
  building and behaving the same on the mobile targets;
- mobile-specific code (`crates/rustic-android`, `android/`) is kept working
  but not extended until M14.

## Support tiers and CI coverage

Every row is a required job in the [CI workflow](../.github/workflows/ci.yml);
the aggregate `CI` gate fails if any of them fails.

| Tier | Meaning | Targets |
| --- | --- | --- |
| Native test | Full test suite, doctests and CLI smoke run on real hardware | Linux x64 and ARM64, Windows x64 and ARM64, macOS ARM64 (Apple silicon) and x64 (Intel) |
| Emulated test | Full test suite and CLI smoke run under QEMU user mode (32-bit x86 runs natively) | `armv7-unknown-linux-gnueabihf` (32-bit ARM: Raspberry Pi OS, older Chromebooks/boards), `i686-unknown-linux-gnu` (32-bit x86), `riscv64gc-unknown-linux-gnu` (RISC-V), `s390x-unknown-linux-gnu` (big-endian) |
| Mobile check | Shared libraries compiled with the target standard library and SDK | Android `aarch64`, `armv7`, `x86_64`, `i686`; iOS device `aarch64`, Simulator `aarch64` and `x86_64` |
| Cross-check | Every crate, test and tool compiled for the target; no linking | `arm-unknown-linux-gnueabihf` (ARMv6, Pi Zero/1), `aarch64-unknown-linux-musl` and `x86_64-unknown-linux-musl` (Alpine/embedded), `aarch64-pc-windows-gnullvm`, `i686-pc-windows-msvc` (32-bit Windows), `powerpc64le-unknown-linux-gnu`, `loongarch64-unknown-linux-gnu`, `x86_64-unknown-freebsd` |

Big-endian s390x stands in for big-endian systems generally, and 32-bit ARM and
x86 for every 32-bit device. Emulated and cross-checked targets are compile and
behavior evidence, not device support: device support still requires the
native host, rendering, input, lifecycle and performance gates in the
[roadmap](ROADMAP.md) and [mobile plan](MOBILE.md).

## Current verification status (2026-11-22)

The tiers above describe the configured CI, and since 2026-11-22 they run
again: the repository moved to `moejoe873/Rustic`, where GitHub Actions starts
jobs, and the full matrix passed on `feature` at `9961c1c` (native x64 and
ARM64 Linux, Windows and macOS; the emulated 32-bit ARM, 32-bit x86, RISC-V and
big-endian s390x runs; the cross and mobile compile checks). Later that day
GitHub stopped starting jobs on the new account too (failed payments or a
spending limit), so later commits have no hosted run until it resumes; the
[Actions page](https://github.com/moejoe873/Rustic/actions) shows each push.

From 2026-11-08 to 2026-11-22 GitHub did not start hosted jobs (an account
billing limit), and merges were checked by the sandbox gate of
[DEVELOPMENT.md](DEVELOPMENT.md#branches-and-merging) on one Linux x64 machine.
The first native runs after that window found portability bugs in that work
(Windows AppContainer profile races, macOS `dyld` under Seatbelt, missing C
cross compilers for `ring`), all fixed and recorded in [TD-12](TECH_DEBT.md).
If Actions stops again, this is what the sandbox gate can and cannot show:

| Platform | What the sandbox gate can show |
| --- | --- |
| Linux x64 | Full suite, native |
| Linux 32-bit ARM, 32-bit x86, RISC-V, big-endian s390x | Full suite under QEMU |
| Linux aarch64 | Compile check |
| Windows x64 and ARM64 | Compile check at most (GNU-ABI targets); no test run, nothing for the MSVC targets |
| macOS x64 and ARM64 | Nothing: these targets cannot be built or run from a Linux sandbox |
| Android (ARM64, 32-bit ARM, x86-64, x86) | Compile check, as in hosted CI |
| iOS/iPadOS (device and simulator) | Compile check of the shared libraries, as in hosted CI |

What hosted CI still does not cover:

- **Android and iOS barely run.** Both are compile-checked; the only
  execution is the Android isolated-renderer test on an API 34 x86-64
  emulator (ADR 0059), added to CI on 2026-11-22 and passing since its first run that day. iOS runs
  nothing. Device work (MOB-03 onward, iOS with Apple's browser-engine
  entitlement, mobile shipping) is deliberately the last milestone, M14
  ([ADR 0067](adr/0067-mobile-devices-last.md)).
- **Two cross-checks are partial.** `aarch64-pc-windows-gnullvm` and
  `loongarch64-unknown-linux-gnu` have no packaged C cross compiler for
  `ring`, so the crates that use it are left out there (TD-12).
- **The JavaScript benchmark record for ARM64 is emulated** (TD-125, due in M8).

The engine now has OS-specific code (the renderer and network-process sandboxes
in `rustic-sandbox`, the desktop window in `rustic-shell`), so a change to
those crates is only verified once the native job for that OS has passed.

## Reproducing locally

Native: `cargo test --locked --workspace --all-targets` on the device.

Emulated on Ubuntu/Debian (example for 32-bit ARM):

```sh
sudo apt-get install gcc-arm-linux-gnueabihf libc6-dev-armhf-cross qemu-user-binfmt
rustup target add armv7-unknown-linux-gnueabihf
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc
export QEMU_LD_PREFIX=/usr/arm-linux-gnueabihf
cargo test --locked --workspace --all-targets --target armv7-unknown-linux-gnueabihf
```

For 32-bit x86 install `gcc-multilib` and set the target linker to `gcc`.
Cross-check: `rustup target add <target>` then
`cargo check --locked --workspace --all-targets --target <target>`.

## Adding a target

Add the target to the matching CI job, run it locally, fix portability issues in
the shared code rather than excluding tests, and update the table above. Prefer
promoting a target to a stronger tier (cross-check, then emulated, then native)
once runners or emulators exist.
