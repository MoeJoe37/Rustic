# ADR 0073 — Accessibility: semantics tree, AccessKit bridges, editing and IME

Date: 2026-11-23
Status: Accepted (desktop bridges; real screen-reader acceptance external, mobile bridges in M14)

## Context

M8 (desktop alpha) needs the four `a11y` capabilities: a semantics tree
built from the DOM, layout and live script state (`a11y.semantics`), the
platform bridges that hand it to assistive technology (`a11y.bridges`:
UI Automation on Windows, NSAccessibility on macOS, AT-SPI on Linux),
keyboard editing, focus, selection and IME composition
(`a11y.editing`), and checks through the platform APIs
(`a11y.acceptance`).

Three constraints shape the design. The renderer is sandboxed and owns the
DOM, so the tree is built there and must cross IPC to the broker, which
treats every byte from a renderer as hostile. Each platform API is native
COM, Objective-C or D-Bus code that the workspace's `unsafe_code = forbid`
rules out writing ourselves. And the tree is what assistive users read, so
its roles and names must follow the specifications (HTML-AAM, WAI-ARIA
1.2, AccName 1.2), not an ad hoc mapping.

## Decision

1. **`rustic-a11y` builds the tree in the renderer.** It depends only on
   `rustic-dom` and `rustic-ipc`. Roles follow HTML-AAM's implicit roles
   and valid explicit ARIA roles (first recognised token; abstract roles
   ignored); names and descriptions follow AccName 1.2 (`aria-labelledby`,
   `aria-label`, native labels, `alt`, `title`, `<legend>`, `<caption>`,
   name from content for the roles that allow it, with cycle guards).
   States come from attributes, ARIA and the live form state the page's
   scripts left (`LiveState`: focus, value, checkedness, validity,
   selection). Layout supplies bounds and the rendered/invisible split;
   `display: none`, `hidden`, `aria-hidden` and `visibility: hidden` prune.
   Generic and presentational wrappers collapse. Hard limits (50 000
   nodes, depth 256, 16 KiB of text per field) truncate instead of failing
   and set `truncated`.
2. **The tree crosses IPC as one bounded, versioned message.**
   `PageInput::Accessibility` asks a live renderer for its tree and
   `RendererMessage::Accessibility` answers with `rustic_a11y::wire`
   bytes (at most 32 MiB). The broker decodes with the same limits, in
   preorder, rejecting unknown roles, duplicate ids, parents that do not
   precede children, overlong text and invalid UTF-8; decode never trusts
   a count it has not bounded. Node ids are DOM node ids with the node's
   generation, so a stale id the AT sends back cannot reach a reused slot.
3. **AccessKit 0.25.0 and accesskit_winit 0.34.0, pinned exactly, are the
   platform bridges** (`crates/rustic-shell`, desktop targets only,
   default features off; `accesskit_unix` with `async-io`, `rwh_06`).
   AccessKit is the bridge egui, Slint and Bevy ship; one schema maps to
   UI Automation, NSAccessibility and AT-SPI, and its unsafe platform code
   stays in the reviewed crates. 0.25.0 is the newest release that builds
   with the workspace MSRV (1.85). The 68-package graph (zbus for AT-SPI,
   `windows` 0.62 for UI Automation, objc2 for AppKit) is pinned with
   checksums, source revisions and retained notices in
   `third_party/rust-dependencies.json`; five AccessKit crates contain
   Chromium-derived code, so `LICENSE.chromium` (BSD-3-Clause) is kept
   with them. The adapter is created before the window is shown, as
   AccessKit requires, and the tree is only built once an AT asks for it.
4. **The shell maps the page tree, the AT's actions map back.** The
   window is the root node; the document sits under it with a transform
   from CSS pixels to window pixels. Editable fields get a text-run child
   with character lengths, so carets and selections are reported in
   characters (UTF-16 offsets from the page are converted). AT actions
   (`Focus`, `Click`, `SetValue`, `SetTextSelection`, `ScrollIntoView`)
   become `PageInput::Assistive` requests; the renderer delivers them as
   trusted input through the same paths a user's input takes (focus
   steps, activation behaviour, `beforeinput`/`input`).
5. **Editing and IME live in the engine, not the shell.** Text controls
   keep a selection (start, end, direction) with caret movement, Shift
   extension, select-all, deletion and insertion honouring `maxlength`,
   and the selection API (`selectionStart`, `setSelectionRange`,
   `setRangeText`, `select`). `PageInput::Composition` (start, update,
   end) fires `compositionstart`/`update`/`end` and `beforeinput`/`input`
   with `insertCompositionText` and `isComposing`. The host only turns
   platform IME events into these messages.

## Alternatives considered

- **Bridge code of our own per platform.** Three native APIs, each
  needing `unsafe` FFI and years of AT compatibility work; AccessKit
  already carries that and is reviewed once.
- **Newer AccessKit (0.25.2+ / accesskit_winit 0.35+).** Needs Rust 1.87;
  revisit when the MSRV moves.
- **Building the tree in the broker from a serialized DOM.** Would move
  DOM and style knowledge into the trusted process and duplicate layout;
  the renderer already has both.
- **Incremental tree updates.** Full updates are simpler to bound and
  validate; pages in the alpha are small. Incremental updates are a
  performance item for M12.

## Consequences

- `rustic dump-a11y` prints the tree of a page; `rustic drive` scripts
  can ask for it (`a11y`), compose (`compose start|update|end`) and act
  as an AT (`assist focus|click|value|select`).
- `rustic view` exposes each frame's tree to the platform AT. The window
  still renders per frame rather than over a live page (UI-01), so AT
  actions from the window are accepted only once the view adopts the
  live renderer (TD-191).
- New duplicate versions (quick-xml, winnow, `windows` 0.58/0.62) are
  recorded as `cargo deny` skips.

## Validation

`crates/rustic-a11y/tests/semantics.rs` (roles, names, states, pruning,
limits), `wire` round-trip and hostile-input tests,
`crates/rustic-engine/tests/editing.rs` (caret, selection API,
composition events, assistive actions), `crates/rustic-cli/tests/a11y.rs`
(dump and live drive over IPC), `crates/rustic-shell` mapping tests, and
the CI acceptance probes that read the tree of a running `rustic view`
through AT-SPI, UI Automation and NSAccessibility. Acceptance by real
screen-reader users is recorded as external in the progress catalog.

## Revisit when

The MSRV allows a newer AccessKit, mobile bridges start (M14), or tree
size makes full updates measurable in the M12 budgets.
