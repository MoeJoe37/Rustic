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

## Current verification status (2026-11-17)

The tiers above describe the configured CI. GitHub has not started hosted
jobs since 2026-11-08 because of an account billing limit. Merges since then
are checked by the sandbox gate of
[DEVELOPMENT.md](DEVELOPMENT.md#branches-and-merging), which runs on one Linux
x64 machine and can therefore cover less:

| Platform | What the sandbox gate can show |
| --- | --- |
| Linux x64 | Full suite, native |
| Linux 32-bit ARM, 32-bit x86, RISC-V, big-endian s390x | Full suite under QEMU |
| Linux aarch64 | Compile check. The JavaScript benchmark corpus was also run once under QEMU for M4 (TD-125) |
| Windows x64 and ARM64 | Compile check at most (GNU-ABI targets); no test run, nothing for the MSVC targets |
| macOS x64 and ARM64 | Nothing: these targets cannot be built or run from a Linux sandbox |
| Android (ARM64, 32-bit ARM, x86-64, x86) | Compile check, as in hosted CI |
| iOS/iPadOS (device and simulator) | Compile check of the shared libraries, as in hosted CI |

Each merge records the scope it actually ran. The shared engine is safe, portable Rust with no OS-specific code yet, which
keeps the risk of an undetected break low, but it is not zero: path handling,
line endings, file system behavior and thread stack sizes differ on Windows
and macOS. When hosted CI returns, the first job is a full native run on
every tier and fixing whatever it finds, before new feature work
([TD-12](TECH_DEBT.md), [ROADMAP §17](ROADMAP.md#17-current-implementation-queue)).
Native runners are a hard prerequisite of M7 (sandboxes) and M8 (hosts).

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
