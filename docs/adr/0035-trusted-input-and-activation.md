# ADR 0035 — Trusted host input and control activation

**Status** Accepted for WEB-04.

## Context

The scripted document already owns one task scheduler and one DOM event
dispatcher. A desktop, mobile, or test host can select a click target by
hit testing, and can supply a node for keyboard delivery. The scheduler
needs to accommodate tree mutations between target selection and delivery.
The [DOM dispatch algorithm](https://dom.spec.whatwg.org/#concept-event-dispatch),
the [HTML input model](https://html.spec.whatwg.org/multipage/input.html#the-input-element),
and [Pointer Events click dispatch](https://www.w3.org/TR/pointerevents3/#the-click-auxclick-and-contextmenu-events)
define the event and activation behavior for the initial controls.

## Decision

The host queues a primary mouse click, keydown, or keyup with a generation-
checked node ID. The same immediate FIFO handles scripts and input. Both
share pending-task and UTF-16 limits with script timers. Input tasks run
under the agent's cancellation and step budget and complete a microtask
checkpoint before the next task. The host supplies the hit-tested pointer
target or its selected keyboard target. The scheduler accepts IDs from the
active light DOM and checks them again at delivery. It drops an input task
whose target was detached or adopted in the meantime.

Native delivery creates PointerEvent or KeyboardEvent state directly.
Script-created events have an unset trust flag. Public dispatch also
clears trust on redispatch. Trust conveys event provenance. Privileged
consent and user-activation gating are separate future host work.

Host clicks on disabled buttons and inputs are suppressed, including the
disabled fieldset rule and its first legend exception. An uncanceled
Enter keydown can click an enabled button. An uncanceled Space keydown and
keyup on the same enabled button or checkbox can click that control.
The binding checks the target again after the key listeners run.

For a mouse-class click that activates a checkbox, preactivation toggles
checkedness and clears indeterminateness before listeners run. Cancellation
restores the previous values. Successful activation emits input and change
for a checkbox connected to its document. The current turn includes those
events and their listener work before its microtask checkpoint. A private
weak map keyed by input wrapper stores mutable checkedness and
indeterminateness, preserving both through adoption and joint collection.

## Verification

Engine tests exercise host pointer and keyboard payloads, queue limits,
target invalidation, lifecycle cancellation, default actions, and
checkpoint ordering. Binding tests exercise script-created mouse clicks,
rollback, attribute reflection, and event payloads under GC stress.
Form submission, focus, text and IME input, other control defaults,
and extended UI event attributes remain covered by WEB-07 and TD-131.
