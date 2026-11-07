# Security architecture and browsing gates

This is a target threat model. The M0 program has no page execution or network
support and no sandbox implementation. Future security claims require tests and
platform evidence, not this document alone. Reporting instructions are in the
root [security policy](../SECURITY.md).

## Assets, adversaries, and authority

Protect user files, credentials, profile data, other origins' content, browser
chrome, OS resources, and availability. Assume documents, scripts, CSS, fonts,
images, network responses, redirects, persisted site data, and IPC peers can be
malicious. A compromised renderer is an explicit threat. A fully compromised OS
or user-approved replacement of the browser binary is outside this model.

| Boundary | Trusted authority | Required defense |
| --- | --- | --- |
| Content → parser/decoder | Bounded input processor | Checked sizes, depth/token limits, cancellation and fuzzing |
| Script → web APIs | Document agent and host bindings | Origin/capability checks, rooted values, reentry-safe conversions |
| Renderer → network/storage | Broker/service | Caller identity and document epoch; verified request context |
| Origin → other origin | Navigation and response policy | Same-origin rules, CORS/response filtering, opaque-origin identity |
| Site/profile → persistence | Storage service | Partition keys, quotas, validated schemas, private-mode separation |
| Page → native OS | Browser broker and sandbox | No ambient renderer filesystem/network/process access |
| GPU/codec/native library → host | Restricted service where appropriate | Validated buffers, narrow FFI, crash isolation and updates |
| Page → user decision | Trusted chrome | Unspoofable origin display and prompts; activation-gated privileges |
| Repository contribution → CI | GitHub workflow | Read-only tokens, untrusted PR event, no privileged code execution |

## Policy rules

Construct origin records using the URL/navigation rules: tuple origins where
applicable and unique opaque identities otherwise. Never compare displayed URL
strings as an origin check. `file:`, `data:`, `blob:`, `about:blank`, inherited
origins and sandboxed frames require explicit tested rules before exposure.
Do not grant all local files the same trusted origin. Determine a schemeful site
using reviewed public-suffix data for site grouping; a site is not an origin.

The browser assigns document authority at navigation commit. Redirects and
subresource requests are revalidated. A renderer cannot self-assert another
origin, profile, top-level site, permission grant, or policy container. Check
message sender plus object ownership and epoch on the receiving side. Network
DNS/connection handling must avoid turning broker requests into an unrestricted
local-network or privileged-port access mechanism; define relevant web-compatible
restrictions and test address changes/redirects in M6.

Implement CORS and response filtering rather than treating a successful HTTP
request as permission to expose its body. Cookies need domain/path/secure/HTTP-only
and SameSite rules plus the selected partition policy. Key cache and persistence
with the same documented context. Apply CSP, secure-context, referrer, mixed-content
and MIME rules to every implemented execution/loading path, including redirects,
dynamic scripts and workers as those are added. Enforce security features needed
by supported flows; gate flows whose required policy semantics remain absent.

Clipboard, file chooser, downloads, notifications and device APIs require explicit
broker grants and trusted user-facing decisions. Scripts cannot create user
activation through synthetic events. An embedder may remove capabilities; it must
not expose an unrestricted native escape hatch to page script accidentally.

## Memory, resource exhaustion, and native code

Project crates forbid unsafe Rust initially. A future exception requires an ADR
and a narrow separately reviewed adapter with documented invariants. Dependencies
may contain unsafe code; Rust is not a guarantee against their vulnerabilities.
Audit FFI ownership, lengths, alignment, threading, callbacks, panic boundaries,
and native allocations. Never unwind across a foreign boundary.

Before allocation validate byte counts, image dimensions, stride, arithmetic,
compressed/decompressed ratios and queue lengths. Budget DOM nodes, parser depth,
script instructions/heap, timers, decoded fonts/images, requests and profile use.
Watchdogs can terminate a renderer that fails to cooperate. Limits must produce
documented errors without panics or partially authorized operations. Sandboxing
does not prevent memory exhaustion on its own; broker resource accounting is
needed too. Do not log cookies, authorization, entered form values or private-page
content by default.

## Isolation rollout

M1–M5 allow trusted local fixture mode. M6 allows an explicit development origin
allowlist with a disposable profile; it remains unsuitable for arbitrary browsing.
M7 introduces mandatory process boundaries, sandbox launch verification, brokered
services and cross-origin frame/site-assignment policy. The reference browser
must refuse remote mode if the selected platform's required isolation is absent.
An explicitly separate test harness is never advertised as protected browsing.

The OS implementation is platform-specific. Linux namespace/seccomp policy,
Windows token/AppContainer/job policy, and macOS feasibility each need practical
prototypes and adversarial verification. Android needs application/service isolation and broker tests;
iOS needs the applicable engine extension/process/entitlement design and tests.
Do not assume desktop process/JIT/sandbox mechanisms exist on mobile. See the
[mobile gates](MOBILE.md). A compile pass does not prove
that its runtime sandbox exists. Process mapping must respect same-origin access,
agent relationships and cross-origin frames; “one process per tab” is insufficient
as a security specification.

## General remote browsing release gates

- [ ] URL/origin, redirects, inherited/opaque origins and response visibility have negative tests.
- [ ] CSP and loading/execution policy cover all exposed script and resource paths.
- [ ] Cookies, storage, cache, permissions and private profiles cannot cross their isolation keys.
- [ ] Renderer sandbox launches with verified restrictions on each supported remote-browsing OS.
- [ ] Hostile renderer IPC cannot mint capabilities, escape budgets or access other documents.
- [ ] Infinite scripts, decompression bombs and deep parser inputs terminate within budgets.
- [ ] Renderer/service crashes cancel work and invalidate handles without killing chrome.
- [ ] GC/host tracing, native codecs, FFI and shared memory have focused stress/fuzz coverage.
- [ ] Certificate verification is enabled and errors have no silent fallback.
- [ ] Debugging/inspection has no unintended remote listener or private-data exposure.
- [ ] A documented review has no unresolved release-blocking security defects.

Record evidence per platform and committed SHA. Re-run affected gates after
changes to authority, process assignment, serialization, native dependencies or
exposed APIs. External review is desirable; absence of known bugs does not prove
the browser safe.
