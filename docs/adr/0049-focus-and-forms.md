# ADR 0049 — Focus, text controls and form submission without navigation

Date: 2026-11-19
Status: Accepted for WEB-07 (`web.input`); implemented

## Context

M5's WEB-07 scope asks for focus (`focus()`, `blur()`,
`document.activeElement`, focus events, Tab navigation), text input and
editing of `value`, form owners including the parser's form element pointer
(TD-31), and `submit` plus constraint validation without navigation.
Navigation, the network and native text input do not exist yet, and the
renderer paints no control contents. The design must keep values as page
state that survives collection and adoption, keep the DOM free of
JavaScript, and give hosts a deterministic result for a submission.

## Decision

1. **Focus.** The page keeps one focused element identity for the main
   document. Focusable areas are HTML elements with a valid `tabindex`,
   `a`/`area` with `href`, `button`, `select`, `textarea`, `summary` and
   non-hidden inputs, minus disabled controls. The focus update steps fire
   `change` (for an edited text control), `blur` and `focusout` at the old
   area, then `focus` and `focusin` at the new one, with `relatedTarget`.
   A focused element that leaves the document stops being reported by
   `activeElement`, which falls back to the body. Trusted clicks focus the
   nearest focusable inclusive ancestor (or the viewport) before `click`.
   An uncanceled Tab or Shift+Tab moves through positive `tabindex` values
   in order, then `tabindex=0` areas in tree order, and wraps.
2. **Values.** Text control values, the user-edit flag, the value at focus
   and custom validity messages are private records keyed by the control
   wrapper, like checkbox state. The `value` getter applies HTML value
   sanitization for the implemented input types. `defaultValue` reflects
   the content attribute (or a textarea's text).
3. **Editing.** A trusted key press on an editable text control appends
   printable text, removes the last code point on Backspace, or inserts a
   line break in a textarea, between a cancelable `beforeinput` and an
   `input` InputEvent. There is no caret: edits apply at the end.
   `maxlength` limits insertion. Enter in a single-line control runs
   implicit submission through the form's default button.
4. **Form owners.** The owner is computed on demand: the `form` attribute's
   form, else the nearest form ancestor, else the parser's association.
   `Document::associate_parser_form` records the HTML parser's form element
   pointer for listed controls that are not the form's descendants, and a
   control that leaves its parent loses it (pays TD-31).
5. **Validation and submission.** `ValidityState` computes `valueMissing`
   (including radio groups), `typeMismatch` (email, url), `tooLong`/
   `tooShort` after user edits, `rangeUnderflow`/`rangeOverflow` and
   `customError`. `checkValidity`, `reportValidity` and form validation fire
   `invalid`. `requestSubmit`, submit buttons and implicit submission
   validate (unless `novalidate` or `formnovalidate`), fire a cancelable
   `SubmitEvent`, construct the entry list and hand a `FormSubmission` to
   the host; `submit()` skips both steps. `reset()` fires `reset` and
   restores values and checkedness. Hosts drain submissions with
   `ScriptedDocument::take_form_submissions`; nothing navigates.

## Alternatives considered

- Storing values in the DOM would let the renderer paint them, but values
  are scripting state with collection and adoption rules the bindings
  already enforce for checkboxes; the renderer can read them when control
  painting lands.
- A per-element form owner field kept in sync by every insertion and
  removal follows HTML's "reset the form owner" more literally but costs a
  hook in every DOM mutation for a case the computed owner handles.

## Consequences

- `:focus`, `:valid`/`:invalid` and dirty checkedness are not visible to
  style, control values are not painted, and focus changes do not
  invalidate frames. `select`/`option`, labels, selection APIs, caret
  movement, `pattern`, `step` and `badInput` are not implemented
  ([TD-154](../TECH_DEBT.md#td-154--form-and-focus-gaps-outside-the-m5-scope)).
- A control whose ancestor moves keeps the parser association until the
  control itself moves.

## Validation

`crates/rustic-engine/tests/forms.rs` drives Tab and Shift+Tab, click
focusing, typing with `beforeinput` cancellation, `change` on blur,
textarea line breaks, readonly controls, implicit submission blocked by
invalid fields and then accepted with its exact entry list, and the script
APIs: parser association, the `form` attribute, live `elements`, value
sanitization, range, url and radio-group validity, custom messages,
`novalidate`, `submit()`, `requestSubmit` errors, reset and the event
constructors. The `form-validation` scripted reftest is the M5 demo.

## Revisit

When control painting, `select`, labels or a caret model land (M9), or when
navigation makes submissions load documents (M6 NET-07).
