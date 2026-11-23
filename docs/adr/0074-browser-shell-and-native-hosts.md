# ADR 0074 — Browser shell, desktop host lifecycle and native fonts

Date: 2026-11-23
Status: Accepted (Linux X11 and Wayland verified locally and in CI; native Windows and macOS x64/ARM64 through the CI window tests)

## Context

M8 (UI-01, UI-02) needs a usable desktop browser on Windows, macOS and
Linux: address entry, tabs, history, reload and stop, loading and error
states, zoom, find, download and permission UI and profiles, all kept
apart from page content; a host that handles resizes, device-scale changes
(including fractional scales), minimize and background, keyboard and IME
input; and text drawn with the platform's own fonts instead of the bundled
test fonts. [ADR 0065](0065-desktop-window-host.md) gave a window that
re-rendered the page per frame and knew only scrolling;
[ADR 0069](0069-long-lived-renderer.md) gave a long-lived renderer per
page.

## Decision

1. **Host API (`rustic-shell`).** The window turns platform events into
   library-independent `Input` (keys with modifiers, typed text, IME
   preedit/commit, strip clicks, page clicks, scrolls) and `Lifecycle`
   (resize with device scale, visible/hidden, focus) for a `Content`.
   Unhandled keys fall back to scrolling and Escape-to-close. A window that
   is minimized, fully occluded or zero-sized presents nothing. The
   content says when text entry is active, and the host enables the input
   method and places its candidate window there. Frames are `settled`
   when they show what was asked; tests count only settled frames.
2. **Browser model (`rustic-cli/src/browser.rs`).** Tabs with session
   history (50 entries), load state (loading, stopped, failed),
   per-tab zoom steps, per-entry scroll, find queries, the address and
   find fields (caret, selection, IME composition) and the browser
   shortcuts (platform command key). It holds no pixels and runs no page
   code. The address field accepts absolute `http(s):`/`file:` URLs,
   existing local paths and bare host names (as `https://`); there is no
   search, and refused entries say why.
3. **Trusted strip (`chrome.rs`).** Tab row, toolbar (back, forward,
   reload/stop, address, zoom), find bar, permission prompt and download
   notice, drawn by the broker from escaped printable ASCII with scripts
   off, from one layout that both drawing and hit testing use. Error,
   stopped and new-tab pages are trusted documents drawn the same way.
4. **Pages (`view.rs`).** Each tab has a worker thread holding its live
   page (`broker::live`); frames, clicks and keys go to the active tab's,
   so the window keeps answering while a page loads. Stop and navigation
   drop stale results by generation. Background tabs stay live (scripts
   and state survive a tab switch) and get at most one frame a second;
   beyond six live pages the least recently shown is discarded and loads
   again when shown. All page threads share one opened profile, so one
   profile lock serves every tab. Each frame reports the focused editable
   field's box (`Painted.editing`); while a page field has focus the
   window enables the input method at that box and preedit and commit
   reach the page as composition (`compositionstart`/`update`/`end` and
   `input`), keys go to the page instead of the browser's shortcuts, and
   turning the input method off ends a composition. Find-in-page is part of the frame request
   (`FrameAsk.find`, `Painted.found`): the renderer marks matches and
   reports the active one, which the browser scrolls into view (TD-195).
   Load failures and downloads come from the broker (`broker::Failure`),
   never from the renderer.
   While an assistive technology listens (ADR 0073), each frame also
   fetches the page's accessibility tree for the platform bridge, and its
   actions (focus, click, set value, set selection) go to the live page.
   A renderer keeps the broker's first page input when it arrives while
   the renderer is still loading and waiting for a reply.
5. **Native fonts (platform.native).** `view` (and `--system-fonts` on
   any command) grants the platform's fonts: fontconfig's answers on Linux
   and the BSDs, `rustic_text::native` tables for Windows and macOS (and
   Unix without fontconfig), each CSS generic family mapped to the
   platform's family, plus fallback faces for other scripts, at most
   64 MiB and 64 faces. OpenType collections are split into standalone
   faces (`rustic_text::collection_face`). The broker reads the files and
   sends bytes and `generic=family` mappings (`Start.families`); a renderer
   opens no files.
6. **Native evidence.** `RUSTIC_TEST_WINDOW_SCRIPT` drives a real window
   through resizes, minimize and restore (the window system's own
   operations), keys, text, IME events and clicks through the same handlers
   as platform events, and logs each settled frame and the browser's
   state; `RUSTIC_TEST_WINDOW_CAPTURE` saves the presented frames. CI runs
   these tests on native Windows and macOS (x64 and ARM64) desktops and on
   Linux under Xvfb (X11, including a 1.5 scale) and headless Weston
   (Wayland). A curated site corpus (`tests/corpus`: an article, a
   floated blog, a flex landing page, sticky and fixed documentation, an
   image gallery in every decoded format, a clicked tab app, timers and
   animation frames, international and right-to-left text, keyboard
   events) checks itself in each window and turns a corner square green;
   the test reads it from the presented frame.
7. **Windowless children (TD-183).** `rustic-cli`'s window stack is the
   `window` feature, on by default. `rustic-renderer` is the same commands
   without it; on macOS the broker starts renderers and the network
   process from it when it sits beside `rustic` and is not older, so
   AppKit's initializers never run before a child's Seatbelt profile
   applies. The profiles also hide the machine's names and identifiers
   and the `kern.proc`, `kern.procargs` and `net` parameter families.

## Alternatives considered

- A browser UI rendered by a page (HTML chrome in a renderer): rejected; the
  strip must stay outside anything a page can script or paint.
- Platform font APIs (DirectWrite, Core Text) through FFI: they need
  `unsafe` outside `rustic-sandbox`; file tables plus fontconfig cover the
  generic families now, and a reviewed adapter can replace them (TD-85).
- Only the active tab live, others reloading when shown: lost script
  state on every tab switch; replaced by per-tab live pages (TD-196).
- An allowlist of `sysctl` names for the Seatbelt profiles: the system
  libraries' reads are open-ended and undocumented (the first native run
  aborted in dyld), so the profiles hide what identifies the machine or
  reveals other processes instead.

## Consequences

`rustic view` is a small browser: real fonts, tabs, history, find and
zoom, with page input and composition through the long-lived renderer.
Real system input methods are not exercised in CI (synthetic IME events
use the same path, TD-197). Find marks are estimated within a text
fragment (TD-195). Six live pages bound memory; a seventh tab discards
the least recently shown. Packaging must build `rustic-renderer` on its
own (built with the workspace, Cargo unifies features and links the
window stack in).

## Validation

`crates/rustic-cli/tests/view.rs` (window lifecycle, browser shell,
fractional scale, page composition, background tabs, the site corpus),
the macOS escape probes and the CI step that builds `rustic-renderer`
alone, checks it links no AppKit and runs the isolation tests from it, unit tests in `rustic-shell` (scripts, fallbacks, strip
rows), `browser.rs`, `chrome.rs`, `find.rs`, `fonts`/`native`/collection
tests in `rustic-text`, IPC round trips for `FrameAsk.find`,
`Painted.found` and `Start.families`.

## Revisit

When a GPU backend presents the window (TD-185), when platform input
methods run in CI (TD-197), when macOS packaging bundles the executables,
or when a reviewed native font
adapter is proposed.
