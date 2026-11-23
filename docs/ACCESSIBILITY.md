# Accessibility

Rustic's accessibility support (M8 `a11y.*`, [ADR 0073](adr/0073-accessibility.md)):
how a page becomes a tree that screen readers read, how assistive
technology acts on the page, and how text editing and IME composition
work. Open gaps are TD-191 to TD-194 in [TECH_DEBT.md](TECH_DEBT.md).

## The tree

`crates/rustic-a11y` builds an `AxTree` in the renderer from the DOM, the
layout and the state the page's scripts left:

- **Roles** follow [HTML-AAM](https://www.w3.org/TR/html-aam-1.0/)'s
  implicit roles and the first valid explicit
  [WAI-ARIA 1.2](https://www.w3.org/TR/wai-aria-1.2/) role token.
  Abstract roles are ignored; `presentation`/`none` and generic wrappers
  collapse into their parents.
- **Names and descriptions** follow
  [AccName 1.2](https://www.w3.org/TR/accname-1.2/): `aria-labelledby`,
  `aria-label`, native labels (`<label>`, `alt`, `<legend>`, `<caption>`,
  button values), name from content for the roles that allow it, then
  `title` and `placeholder`.
- **States**: focusable, focused, disabled, checked/mixed, pressed,
  expanded, selected, required, invalid, read-only, multi-line,
  multi-selectable, editable, modal, busy, protected; heading levels,
  range values, live regions, URLs and text selection.
- **Hidden content** (`display: none`, `hidden`, `aria-hidden=true`,
  `visibility: hidden`) is left out. Laid-out nodes carry bounds in CSS
  pixels.
- **Limits**: 50 000 nodes, depth 256 and 16 KiB of text per field. A
  page past them gets a truncated tree, never an error.

`rustic dump-a11y PAGE` prints the tree:

```text
document "Accessibility probe" @0,0 1280x178.88
  main @8,21.44 1264x149.44
    heading "Accessibility probe" level=1 @8,21.44 1264x32
    navigation "Sections" @8,106.88 1264x16
      link "First section" url="#one" focusable @8,106.88 100x16
    checkbox "Remember me" focusable checked=true @8,138.88 1264x16
```

## Across the process boundary

The renderer is untrusted. Its tree reaches the broker as one
`RendererMessage::Accessibility` message of at most 32 MiB (or the
`a11y` report line of `render --report-a11y`, which `rustic view` uses
each frame). `rustic_a11y::wire::decode` applies the same limits as the
builder and rejects unknown roles, duplicate ids, parents that do not
precede their children, invalid UTF-8 and counts it has not bounded.
Node ids carry the DOM node's generation, so an id an assistive
technology sends back after the node is gone matches nothing.

## Platform bridges

`crates/rustic-shell` hands the tree to the platform through AccessKit
0.25 (UI Automation on Windows, NSAccessibility on macOS, AT-SPI over
D-Bus on Linux). The window is the root; the document sits under it,
scaled to device pixels and placed below the browser's address strip.
Editable fields get a text-run child so carets and selections are
reported in characters. The tree is built only after an assistive
technology asks for it.

Assistive actions map back to `PageInput::Assistive` requests: focus,
click (activation behaviour, as a user's click), set value (with
`beforeinput`/`input`) and set selection; scroll-into-view has no page
request yet.
`rustic view` does not deliver actions yet (TD-191); `rustic drive`
scripts do:

```text
a11y
assist focus Email
assist value Email=me@example.com
assist select Email=0,2
assist click Send
```

## Editing, focus and IME

Text inputs and text areas keep a selection (start, end, direction).
Arrow keys, Home and End move the caret (Shift extends), Up and Down move
between text-area lines, Ctrl/Cmd+A selects all, Backspace, Delete, Enter
and printable keys edit at the selection within `maxlength`, through
cancelable `beforeinput` and then `input`. The HTML selection API is
implemented: `selectionStart`, `selectionEnd`, `selectionDirection`,
`setSelectionRange`, `select` and `setRangeText` with all four modes.

IME composition arrives as `PageInput::Composition` (`start`, `update`,
`end`) and fires `compositionstart`, `compositionupdate` and
`compositionend` with `beforeinput`/`input` of type
`insertCompositionText` and `isComposing`. In a drive script:

```text
compose start
compose update にほ
compose end 日本
```

## Acceptance

CI runs `rustic view` on every desktop runner and reads its tree through
the platform API a screen reader uses:
[`tools/a11y_probe_linux.py`](../tools/a11y_probe_linux.py) (AT-SPI under
Xvfb), [`tools/a11y_probe_windows.ps1`](../tools/a11y_probe_windows.ps1)
(UI Automation) and [`tools/a11y_probe_macos.py`](../tools/a11y_probe_macos.py)
(NSAccessibility through System Events), all on
[`tests/a11y/probe.html`](../tests/a11y/probe.html). Acceptance by
screen-reader users (NVDA, Narrator, VoiceOver, Orca) needs people outside
the project and is recorded as an external step of `a11y.acceptance`.
