# Desktop and mobile engine support

## Scope and current status

Windows, macOS, Linux, Android and iOS/iPadOS are target platforms for the same
original Rustic engine. Android/iOS are part of the architecture now, not an
optional rewrite after desktop completion. Parsers, DOM, JavaScript, layout,
paint semantics and browser policy must stay independent of desktop shells.
Mozilla/Firefox/Gecko, SpiderMonkey, Servo, Chromium/Blink and WebKit are excluded
as implementation foundations on every target. A system WebView must not silently
replace Rustic's engine on mobile.

Current mobile support consists of shared-library cross-compilation checks
(green in hosted CI on 2026-11-22 for all seven Android and iOS targets) and
the mobile isolation of [ADR 0059](adr/0059-mobile-renderer-isolation.md)
(Proposed, MOB-07): on Android a test app (`android/`) runs the renderer in an
isolated-process service through `crates/rustic-android`, with an instrumented
isolation and crash-recovery test on an API 34 x86-64 emulator in CI (first
run pending); iOS is a design only. There is no user-facing Android app, iOS
application/framework, device renderer surface, touch/IME bridge or
real-device test yet. `rustic-shell` builds its desktop window only off Android,
where the host will be an activity. Passing `cargo check`
does not prove those work. The desktop development CLI is not a mobile app.

## Shared engine and native adapters

Core crates keep document semantics and state ownership in Rust. Put JNI/NDK/
Java/Kotlin and Objective-C/Swift/Apple SDK integration in narrow platform adapters.
Create adapter crates when an executable surface prototype needs them. Avoid
empty platform crates and duplicated parser/rendering implementations.

Host contracts must include:

- Surface creation, resize, density/scale, safe-area updates and surface recreation.
- Touch/pointer events, scrolling/pinch gestures, hit testing, focus and selection.
- Keyboard/IME composition, UTF-16 positions, caret rectangles and text replacement.
- Foreground/background, suspension/resume, close and memory-pressure notifications.
- Accessibility roles/names/states/focus and geometry mapped to native services.
- Permission prompts, file picking, clipboard, downloads and profile directories.
- Frame scheduling without assuming a permanently active desktop event loop.
- Platform-supported renderer/service isolation, brokering and crash recovery.

The document agent still serializes DOM/JS mutation. Native callbacks post work
to it; never call script while OS or engine mutable borrows/locks are held. Async
completions carry navigation/document epochs so backgrounding or closing a view
cannot revive stale work. Memory pressure may evict caches, not script-reachable
DOM objects outside the tracing contract. Define timer/background scheduling with
the HTML event loop rather than resetting monotonic clocks on resume.

Prototype native bridges are narrow and unstable: define opaque handles, thread
affinity, byte/UTF-16 lengths, cancellation, shutdown and error delivery. Rust must
not unwind across a foreign boundary. Stabilize the long-term ABI at M10 after
real hosts exercise it. Keep the original interpreter usable without JIT or
executable-memory permission. Select graphics backends using actual Android/iOS
feasibility and reference-renderer parity evidence.

## Implementation tracks

Required full-product mobile acceptance also includes Wasm interpreter/Web APIs,
GPU acceleration, native Rustic MV2 extension lifecycle and the uBO port. See
[PRODUCT.md](PRODUCT.md) and [EXTENSIONS.md](EXTENSIONS.md). Budget background
callbacks, preserve broker filtering while UI pages are suspended, and test
permission revocation/update recovery on devices. Recheck current platform
entitlements, distribution eligibility and executable-memory constraints before
release; missing approvals cannot be represented as successful runtime support.

| Task | Milestone | Deliverable and acceptance evidence |
| --- | --- | --- |
| MOB-01 | M1 | Shared library cross-checks for Android ARM64/ARMv7/x86-64/x86 and iOS device plus ARM64/x86-64 simulator, required by CI |
| MOB-02 | M1–M3 | Audit native dependencies for both mobile systems, licenses, SDK/NDK and thread/resource requirements |
| MOB-03 | M3, carried forward through M5; now the first task of M8 mobile hosts ([M5 review](reviews/2026-11-19-m5-completion.md)); needs NDK emulator runners | Android local-fixture CPU surface through an unstable JNI/native bridge; compare pixels with headless output |
| MOB-04 | M3, carried forward through M5; now the first task of M8 mobile hosts; needs Xcode simulator runners | iOS local-fixture CPU surface and C/Swift bridge; link/run in Simulator and on a signed development device where allowed |
| MOB-05 | After MOB-03/04 (was M3–M5; not started) | Touch, viewport/scale, keyboard/IME, caret/selection and accessibility fixtures on both mobile systems |
| MOB-06 | M5–M6 | Background/resume, memory pressure, profile isolation, permission UI and cancellation integration tests |
| MOB-07 | M7 | OS-supported isolated renderer/service design (ADR 0059, Proposed; Android isolated renderer service implemented 2026-11-22), adversarial authority tests and iOS entitlement/process feasibility (open; `security.mobile`) |
| MOB-08 | M8 | Native mobile reference hosts using Rustic's renderer/runtime; emulator/simulator and real-device suites |
| MOB-09 | M9 | Shared site/conformance corpus plus mobile editing/fonts/graphics/accessibility and measured energy/memory behavior |
| MOB-10 | M10 | Reproducible native artifacts, supported OS/ABI policy, signing/distribution gates and security updates |

Mobile CPU surface prototypes arrive alongside M3's renderer. GPU optimization
follows a correct CPU/reference path, with surface-loss recovery and device tests.
Do not assume desktop window, graphics or process interfaces work unchanged.

## Android

Start with `aarch64-linux-android` (`arm64-v8a`) for modern devices and
`x86_64-linux-android` for emulator coverage. Metadata checks do not link apps.
The native surface prototype must select/pin the NDK, SDK/minimum API, linker,
Gradle setup and supported ABIs. Minimum OS versions remain undecided until
graphics, fonts, accessibility and isolation requirements are evaluated.

Tests must cover rotation, surface recreation, insets/density, touch, IME,
backgrounding, low memory and app/service death. Renderer separation must use
mechanisms available to an ordinary app on supported Android versions, without
assuming root or desktop namespace/seccomp/process privileges.

Certificate verification uses the Android system verifier through
`rustls-platform-verifier` (ADR 0051). The host app must ship that crate's
Kotlin component (`rustls-platform-verifier-android`) and call its
`init_hosted` JNI initializer before the first `https` request; until then
`TlsConfig::platform()` fails and `https` requests end with
`TransportError::TlsUnavailable`, never without verification.

Reference: [Android NDK ABI guide](https://developer.android.com/ndk/guides/abis).

## iOS and iPadOS

Start with `aarch64-apple-ios` for devices and `aarch64-apple-ios-sim` for ARM64
Simulator. Cross-check on macOS with the matching Xcode SDK. Later tests must
link a real host/framework, sign/install the development app and execute rendering,
input and lifecycle fixtures in Simulator and on physical devices. Choose a
deployment baseline with the native adapter's actual capabilities, not Rust's
historical minimum. Target availability does not establish App Store eligibility.

Apple currently provides alternative-engine mechanisms with region- and
entitlement-dependent availability. Evaluate current BrowserEngineKit/Core,
extension/process, engine eligibility and distribution requirements before
shipping. Official documentation describes routes including the EU and Japan;
it does not establish unconditional global availability. Recheck at implementation
and release time. Gate unsupported configurations explicitly; do not claim success
by substituting WKWebView/WebKit for Rustic.

References: [Rust iOS targets](https://doc.rust-lang.org/rustc/platform-support/apple-ios.html),
[BrowserEngineKit](https://developer.apple.com/documentation/browserenginekit),
[engine entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.web-browser-engine.host),
and [Apple's EU program](https://developer.apple.com/support/alternative-browser-engines/).
Reviewed 2026-11-07. No entitlement approval, signing identity, device access or
distribution permission is asserted by this repository.

## CI and runtime gates

Seven `Mobile core` jobs run `cargo check --locked --workspace --lib --target`:
Android `aarch64`, `armv7`, `x86_64` and `i686`, and iOS `aarch64` device,
`aarch64` Simulator and `x86_64` Simulator. Rustic must run on any device that
can run a browser, so older 32-bit Android phones and Intel Macs' simulators
are covered alongside current hardware; see [PLATFORMS.md](PLATFORMS.md).
Android checks run on Ubuntu and iOS checks on macOS with Xcode SDK selection.
They compile the shared libraries without linking an app or running target tests;
the desktop CLI/subprocess tests are excluded. The aggregate `CI` gate requires
all seven checks to succeed.

For example, check Android's shared library code locally with the pinned toolchain:

```sh
rustup target add aarch64-linux-android
cargo check --locked --workspace --lib --target aarch64-linux-android
```

On macOS, use `aarch64-apple-ios` or `aarch64-apple-ios-sim` with the appropriate
Xcode SDK; use `x86_64-linux-android` for the Android emulator compile check.

Add NDK/Xcode link/package checks when bridges exist, then emulator/simulator
runtime tests. Releases additionally need real-device rendering, touch/IME,
accessibility, lifecycle, memory/energy, permission, profile and isolation evidence.
Keep shared conformance fixtures identical across targets with explicit reasons
for platform-specific expectations. Cross-compilation alone is not device support.
