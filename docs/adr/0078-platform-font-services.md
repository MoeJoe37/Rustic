# ADR 0078 — Platform font services and the scope of native OS adapters

Date: 2026-11-23
Status: Accepted (Linux verified locally; the Windows and macOS adapters are type-checked for both targets locally and run in the CI test jobs on Windows and macOS, x64 and ARM64)

## Context

`platform.native` (M8) asks for a reviewed platform font adapter on
Windows and macOS and for "native codec and OS service adapters"
(TD-85). [ADR 0074](0074-browser-shell-and-native-hosts.md) gave the
browser the platform's fonts by asking fontconfig on Linux and the BSDs
and by looking for known file names (`rustic_text::native`) on Windows
and macOS. File tables miss fonts that are installed elsewhere, renamed
by an OS release, installed per user, or chosen by the system (the
Windows message font, the macOS system font and its fallback cascade).
Chromium, Gecko and WebKit all ask the OS font service instead:
DirectWrite on Windows and Core Text on macOS.

Both services are C or COM APIs. Rustic forbids `unsafe` code outside
the crates [ADR 0059](0059-mobile-renderer-isolation.md) and
[ADR 0060](0060-windows-renderer-sandbox.md) allow, and every external
dependency needs a review (docs/DEPENDENCIES.md).

## Decision

1. **A new crate, `rustic-native`, holds the OS service adapters.** Its
   API is safe and answers only with paths: `font_file(family, style)`,
   `system_ui_file(style)`, `system_ui_family()` and
   `fallback_files(limit)`. Only absolute paths to existing regular files
   leave the crate. Family names longer than 256 bytes or containing NUL
   never reach a service. On other platforms every answer is `None` or
   empty.
2. **DirectWrite on Windows.** The system font collection finds the
   family; `GetFirstMatchingFont` picks the face for weight 400 or 700
   and upright or italic. A face that DirectWrite would only simulate
   (synthetic bold or oblique) is refused, so a family without a bold
   face is not granted twice. The file path comes from the local font
   file loader; memory and downloadable fonts are refused. `system-ui` is
   the message font from `SPI_GETNONCLIENTMETRICS`, as in Chromium.
   Bindings: `windows` =0.62.2 with only `Win32_Graphics_DirectWrite`,
   `Win32_Graphics_Gdi` and `Win32_UI_WindowsAndMessaging`. The package
   is already in the reviewed graph for the UI Automation bridge
   ([ADR 0073](0073-accessibility.md)), so no new package is added.
3. **Core Text on macOS.** `CTFontCreateWithName` makes the font. Core
   Text substitutes another family for an unknown name, so the answer
   must report the family asked for. Styles come from
   `CTFontCreateCopyWithSymbolicTraits` and must carry the asked traits.
   The file is the font's `kCTFontURLAttribute`. `system-ui` is
   `CTFontCreateUIFontForLanguage(kCTFontUIFontSystem)`, and the fallback
   list is that font's default cascade list for the user's languages
   (at most 8 files). Bindings: `objc2-core-text` =0.3.2 with only
   `std`, `CTFont`, `CTFontDescriptor` and `CTFontTraits`, over the
   already reviewed `objc2-core-foundation` 0.3.2. The new package's
   license (Zlib OR Apache-2.0 OR MIT) is retained from the objc2
   repository at the published revision, as for the other objc2 crates.
4. **`unsafe` is confined as in ADR 0060.** `rustic-native` denies
   (does not forbid) `unsafe_code`. Only the `direct_write` and
   `core_text` modules allow it, with a reason. Clippy's
   `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` keep
   one call per block with a `SAFETY` comment.
5. **The broker stays the reader.** `rustic-cli`'s `system_fonts` asks
   fontconfig, then the platform service (`families` per generic in
   `rustic_text::native`, `system-ui` from the service), then the file
   tables. It reads, size-checks, parses and extracts the files itself,
   within the ADR 0074 budgets (64 MiB, 64 faces). A font service never
   parses web content and never runs in a renderer or the GPU process.
6. **Scope of "native codec and OS service adapters".**
   - **Image codecs stay Rustic's own** bounded decoders (`rustic-codecs`,
     chosen at M3). OS image decoders (WIC, ImageIO) are not used: they
     would parse attacker bytes with the OS's code and limits, and change
     output by OS version.
   - **Platform audio and video decoders** (Media Foundation,
     VideoToolbox, VA-API, MediaCodec) are adapters of the media pipeline.
     They are owned by `media.codecs` (M11), where the ROADMAP platform
     plan already names them, and are not part of M8.
   - **Other OS services** (audio output, notifications, passkeys,
     clipboard) are owned by the capability that needs each one, as the
     ROADMAP platform plan lists. Each one adds its adapter to
     `rustic-native` under this ADR's rules.

## Alternatives considered

- **Keep the file tables only.** These miss per-user and relocated fonts
  and the system UI font, and differ from every engine's behavior.
- **Hand-written FFI declarations.** This means fewer packages but the
  same `unsafe`, and unreviewed struct layouts and COM vtables. The
  `windows` package is already reviewed. `objc2-core-text` is generated
  from Apple's headers by the same project as the reviewed
  `objc2-core-foundation`.
- **`font-kit` or `fontdb`.** These are larger and bring their own
  parsing and matching, and Rustic already parses fonts with its reviewed
  adapters (ADR 0014). Only the service's file answer is needed.
- **Let renderers ask the service.** Renderers open no files and are
  sandboxed away from font services (ADR 0060 and the Seatbelt profile).
  The broker already grants font bytes.

## Consequences

- Windows and macOS hosts grant the fonts the OS uses for each family,
  including per-user installs, the message or system UI font, and Core
  Text's fallback cascade. The file tables remain the fallback.
- Project code gains a second `unsafe` module set, confined to one crate.
  Review it whenever those modules change.
- TD-85's platform-service part is paid. Global cache accounting,
  handle-based traversal and per-user configuration beyond the services
  remain (M9).
- `rustic-native/tests/fonts.rs` asserts real answers on the Windows and
  macOS CI runners: Arial and Helvetica regular and bold, the system UI
  font, and the macOS cascade. It also asserts that unknown families are
  never substituted.
