# ADR 0059 — Android and iOS renderer isolation (design)

Date: 2026-11-20
Status: Proposed (design only; nothing here has run on a device, emulator or simulator)

## Context

ISO-08 / MOB-07 ask for renderer and service isolation on Android and
iOS/iPadOS using mechanisms an ordinary app may use, not desktop
namespaces, seccomp or `posix_spawn` of the app's own executable
([MOBILE.md](../MOBILE.md)). The broker/renderer split, its protocol
(ADR 0056) and the grant checks (ADR 0057) are platform-independent; only
how the renderer process is created, confined and connected differs.

## Decision

### Android

1. The renderer is a bound `Service` declared with
   `android:isolatedProcess="true"` (and `android:useAppZygote` once
   startup cost matters): Android gives each instance its own UID with
   no permissions, no network and no access to the app's files, and the
   SELinux `isolated_app` domain restricts it further. This is the
   mechanism Chromium's renderers use.
2. The broker (the app's main process) binds one renderer per process key
   (ADR 0057) and passes one end of a `socketpair` to it as a
   `ParcelFileDescriptor` over Binder; `rustic-ipc` runs over that stream
   unchanged. The service starts no other IPC.
3. Fonts, documents and resources arrive over the channel; the isolated
   process opens nothing. Binder death notifications and channel
   `Closed` both mean the renderer died: the broker reports it and starts
   a fresh renderer with a fresh grant (security.recovery).
4. JIT is not required: the interpreter runs everywhere.

### iOS and iPadOS

1. Where Apple's alternative-browser-engine program applies (entitlement
   `com.apple.developer.web-browser-engine.host`, region rules in
   MOBILE.md), the renderer runs in a BrowserEngineKit *web content*
   extension, networking in a *networking* extension, and GPU work later
   in a *rendering* extension. Each is a separate sandboxed process the
   system launches; the web content extension has no network access.
2. The broker connects to each extension over XPC and transfers a
   socket descriptor (`xpc_fd_create`) for `rustic-ipc`; XPC carries only
   that handshake.
3. Without the entitlement or outside eligible regions, Rustic does not
   ship a browsing mode on iOS: it never substitutes WKWebView and never
   runs untrusted content in the app process.

## Verification required before acceptance

- Android emulator (ARM64 and x86-64) and a device: the compromised-
  renderer probes of `rustic-cli/tests/isolation.rs` (files, network,
  processes) denied inside the isolated service; crash and hang recovery.
- iOS simulator and a device with the entitlement: the same probes inside
  the web content extension; extension crash recovery.
- Native runners for both (TD-12); none exist in this repository's
  sandbox, so this ADR stays Proposed.

## Revisit

When MOB-03/MOB-04 hosts exist (M8), and at each review of Apple's
browser-engine program.
