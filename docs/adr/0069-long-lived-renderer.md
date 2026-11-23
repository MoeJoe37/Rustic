# ADR 0069 — Long-lived renderers: one renderer per page, driven by input and frame requests

Date: 2026-11-23
Status: Accepted (renderer side, broker API and headless `drive`; the desktop window moves onto it with the UI-01 shell)

## Context

`rustic view` ([ADR 0065](0065-desktop-window-host.md)) kept the process
split by running every frame as a one-shot `render` in a fresh sandboxed
renderer at the window's size and scroll position (TD-182). Every frame
re-parsed and re-laid out the page, scripts started from scratch each
time, timers never fired between frames, and no input other than
scrolling reached the page. A browser page is a long-lived thing: its
document, script heap, timers and scroll state persist while the user
interacts with it.

The engine already had what a live page needs inside one process: the
scripted document's task queue with trusted input (ADR 0035), timers on a
host clock (ADR 0022, 0036), rendering opportunities (ADR 0023) and
scenes for scrolled pages (ADR 0064). What was missing was a protocol to
drive it across the process boundary and a broker that keeps a renderer
for the page's lifetime.

## Decision

1. **One renderer per page.** `broker::live` decides the page's grant
   exactly as a one-shot command does (the same `prepare` step), starts
   one confined renderer with `render --live --run-scripts`, and keeps it
   for the page's lifetime. The renderer loads the page once and, instead
   of painting one image, waits for the broker's input. Live pages run
   scripts: they are browsing pages, and the renderer is confined.
2. **Protocol.** `BrokerMessage::Page(PageInput)` carries a click (window
   CSS pixels, modifier bits), a key press or release (DOM `key`/`code`,
   at most 64 bytes each) or a frame request (`FrameAsk`: window size in
   CSS pixels, device scale, page zoom, an optional user scroll position,
   and the time since the previous frame). The renderer answers a click or
   key with `Handled` after the page ran what the input started, and a
   frame with `Painted` (surface, painted scroll position, when the page
   wants its next frame). Exchanges are strictly request/answer, so a
   renderer's own requests (resources, `fetch()`, storage) are served
   while the broker waits and can never interleave with page input.
   Decoding refuses non-finite numbers, out-of-range sizes, scales and
   zooms, and over-long key strings.
3. **Time is the broker's.** The page's manual clock advances by the
   elapsed time the broker sends, at most one second per frame; due timers
   and at most 1,000 turns run per frame, the rest wait for the next one,
   so a busy page cannot stall the window. `next_frame` is 1/60 s while
   animation frame callbacks wait, else the next timer's delay, so the
   host schedules redraws without polling.
4. **Scroll ownership.** The user's scroll goes through the page
   (`ScriptedDocument::scroll_viewport`: clamped like `scrollTo`, with a
   `scroll` event); the host sends a position only when the user scrolled
   since the last painted frame, so a script's scroll is not reverted by
   the next frame.
5. **Keyboard target.** Keys go to the focused element, else the body,
   else the document element (`document.activeElement`), with the
   existing default actions (Tab navigation, text editing, activation).
6. **Recovery.** Every exchange has a watchdog: the renderer timeout for
   the first (the load), at most 10 s afterwards. A renderer that dies,
   hangs or breaks the protocol is killed and replaced; the fresh one
   loads the page again and receives the input that failed. After three
   replacements the page closes with an error. The host gets a note for
   each replacement to show the user.
7. **Headless driver.** `rustic drive --input SCRIPT` runs the same live
   page against a script of host input (frame, wait, click, key, scroll,
   resize, zoom, probe) and prints one line per frame. It is the test
   harness on every platform (no window system needed) and the base the
   automation protocol (UI-06) can build on.

## Alternatives

- **Keep re-rendering per frame** and replay input: cannot keep script
  state, timers or focus, and costs a full load per frame.
- **Asynchronous input (fire and forget).** Simpler for the host, but a
  renderer request issued while handling input could meet the next page
  message on the same channel; request/answer keeps the channel's order
  provable.
- **Renderer-measured time.** Needs nothing from the broker, but makes
  tests timing-dependent and lets a renderer run timers while the window
  is hidden; broker time is deterministic and can be paused later for
  background throttling (`platform.lifecycle`).

## Consequences

- TD-182's renderer side is paid; the desktop window adopts `broker::live`
  in the UI-01 shell (TD-182 stays open for that and for routing text/IME
  input).
- In live pages iframes paint white for now (TD-178 applies as for
  `render --frames`); popups open through the broker as before.
- `render --live` is refused outside `broker::live`, and a renderer gets
  page input only from the broker, which gets it from the trusted host.
- Evidence: `crates/rustic-cli/tests/live.rs` (state, timers, clicks and
  keys between frames; scroll ownership; scale and zoom; crash and hang
  replacement; usage errors) and `crates/rustic-ipc/tests/channel.rs`
  (round trips and refused values).

## Revisit when

- Shared-memory surfaces arrive (TD-184): `Painted` then carries a handle
  instead of pixels.
- Navigation inside a live page (link clicks, form submission, history)
  needs the broker to replace the page's renderer on cross-site
  navigation and keep it on same-site ones.
