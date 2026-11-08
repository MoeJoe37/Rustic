# ADR 0013: Support any device that can run a browser

Date: 2026-11-08. Status: accepted following explicit maintainer direction.

## Context

ADR 0006 named five release platforms but CI only exercised x86-64 desktops,
Apple silicon and compile checks for ARM64 mobile targets. The maintainer
requires Rustic to support ARM devices on Windows and Linux too, and more
broadly any device that can run a browser.

## Decision

Rustic's shared engine targets every CPU family and operating system that ships
browsers: x86-64, 32-bit x86, ARM64, 32-bit ARM, RISC-V, LoongArch, POWER and
big-endian systems, on Windows, macOS, Linux, Android, iOS/iPadOS and other
systems with a Rust standard library. The five release platforms from ADR 0006
remain the product hosts; this decision makes every CPU those hosts run on a
first-class target and keeps the core portable beyond them.

CI enforces it in tiers described in [PLATFORMS.md](../PLATFORMS.md): native
tests on x64 and ARM64 for Linux, Windows and macOS; full tests under emulation
for 32-bit ARM, 32-bit x86, RISC-V and big-endian s390x; Android and iOS checks
across their ABIs; and cross-checks for further targets. All are required.

## Alternatives and consequences

Testing only on 64-bit little-endian hosts would hide `usize`, endianness and
memory assumptions until device bring-up, when they are expensive to remove.
The first emulated run already found one: a test that needed a multi-gigabyte
string, impossible in a 32-bit address space (fixed by TD-18's linear dumps). CI is wider and slower, and
emulated jobs take minutes longer than native ones.

Compile and emulation evidence is not device support; native hosts, rendering,
input and performance gates still apply per device class.

## Validation and revisit

Required CI jobs for every tier. Promote targets to native runners as hosted
hardware appears. Revisit if a target loses its Rust standard library or a
platform stops shipping browsers.
