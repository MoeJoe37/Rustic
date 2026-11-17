# ADR 0042 — Scripted document rendering state

Date: 2026-11-17
Status: Accepted for WEB-06

The scripted document owns the validity of style, layout and frame output.
The host chooses the viewport and delivers bounded stylesheet and image
data. The headless CPU pipeline remains the reference for rebuilding a
snapshot from the authoritative DOM and the same resource inputs.

Use a document revision for successful changes to links, attributes and
character data. Its scope may conservatively include detached nodes in the
same document. The engine associates a separate revision with viewport and
resource updates when those enter the pump. Each snapshot is document-local
and can be reused only for its recorded revisions. An exhausted revision
disables reuse.

Sample the document revision after animation callbacks and their checkpoints
and before calling the renderer. Validate that revision when the host
accepts the ticket. A read-only script turn can leave a ticket valid. As in
[ADR 0023](0023-document-rendering-turns.md), a newer rendering turn or
document cancellation invalidates an older ticket.

Layout reads and rendering must use the same bounded style and layout
construction. A layout read converts its JavaScript arguments before
borrowing the document and exposes a snapshot value after releasing the
borrow. The engine may recompute once for each dirty snapshot, within a
per-turn budget, and records each recomputation for the host. Resource
delivery and drawing remain separate from the synchronous read.

The first implementation verifies the DOM component of frame validity.
Resource delivery, viewport revisions, cached layout reads and differential
pixel evidence are subsequent WEB-06 work.
