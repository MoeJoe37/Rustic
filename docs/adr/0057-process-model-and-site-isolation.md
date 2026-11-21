# ADR 0057 — Process model and site isolation

Date: 2026-11-20
Status: Accepted for documents, subresources, frames (ADR 0062) and popups

## Context

ISO-01 asks which process each document runs in, how origins stay apart,
how opener relationships and isolation transitions work. Spectre-class
attacks and renderer compromise mean that anything a renderer process can
address it can read; separation must therefore be by process, and
cross-site data must never be sent to a renderer that does not own it
([SECURITY.md](../SECURITY.md#isolation-rollout)).

## Decision

1. **Site per process.** A renderer holds documents of one *process key*
   (`rustic_net::isolation::process_key`): the HTML site of an `http(s)`
   document; its origin when the response sends `Origin-Agent-Cluster: ?1`
   or the document is cross-origin isolated (COOP `same-origin` + COEP);
   and a unique key for opaque origins, `file:` and `data:` documents,
   which share a process with nothing. `may_reuse` allows a renderer to
   take a navigation only to the same non-unique key.
2. **Authority does not move.** Every renderer gets a fresh grant from the
   broker (ADR 0056); a navigation that changes key gets a new renderer
   and the old grant ends with its process. The CLI already runs one
   renderer per document command, which satisfies this for top-level
   documents.
3. **Browsing context groups.** COOP is parsed for secure contexts
   (`IsolationPolicy`), and `requires_group_switch` implements HTML's COOP
   matching; a switch severs the opener and starts a new group (and so a
   new key) once popups exist.
4. **No cross-site bytes in a renderer.** Opaque Response Blocking runs in
   the broker on every no-cors subresource: cross-origin HTML, XML, JSON,
   plain text and other document and data types, nosniff mismatches and
   responses that sniff as documents become network errors before the
   renderer sees them. COEP `require-corp` requires
   `Cross-Origin-Resource-Policy` on cross-origin no-cors resources, and
   `credentialless` omits their credentials. CORS-mode requests remain
   governed by CORS.
5. **Frames and popups** (frames: [ADR 0062](0062-out-of-process-frames.md),
   every frame in a renderer of its own; popups: 2026-11-21, below): a
   cross-site iframe gets its own renderer keyed like a top-level document;
   same-origin access across documents requires the same agent cluster;
   `window.open` stays in the opener's group unless COOP requires a
   switch.

6. **Popups** (2026-11-21). `window.open()` needs transient user
   activation, which it consumes; the page records the request and the
   broker navigates the popup as a top-level navigation the opener
   initiated (its referrer policy, `no-referrer` for `noreferrer`), in a
   renderer of its own with its own grant. Its initial `about:blank`
   inherits the opener's COOP, so `requires_group_switch(true, opener
   COOP, opener origin, popup COOP, popup origin)` decides whether the
   opener relationship survives; `noopener` (or `noreferrer`) always
   severs it. The broker reports the popup's process key and the
   decision. The opener never gets a WindowProxy for the popup
   (`open()` returns `null`) and the popup has no `window.opener`: they
   live in different processes and the CLI has no cross-process
   WindowProxy (TD-178).

## Consequences

Per-site renderers cost memory per site; one renderer per CLI document is
the strict end of that trade. ORB can block a legitimate cross-origin
`text/plain` image request, as Chromium's ORB does.

## Revisit

When iframes, `window.open` or a long-lived browser shell (M8) arrive, and
when Spectre mitigations need `crossOriginIsolated` exposed to script.
