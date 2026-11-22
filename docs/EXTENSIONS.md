# Rustic extensions and the uBlock Origin port

Accepted product scope, 2026-11-07; **not implemented yet**. Build a Rustic-native
extension ecosystem using Manifest V2 semantics and a maintained Rustic-specific
uBlock Origin port. This applies to desktop, Android and iOS/iPadOS hosts.

## Package and API contract

Packages are authored and released for Rustic. A versioned Rustic manifest
section declares identity, API version and minimum host version alongside
`manifest_version: 2`. Specify a Rustic package/signature format, extension origin
scheme, key/ID binding and update protocol before exposing a stable API. No
Chrome/Firefox store or unmodified CRX/XPI installation is part of the contract.
Porting useful extension code to Rustic is allowed; engine/runtime replacement
is still prohibited. The native API namespace is `rustic.*`.

Manifest V2 is an ongoing Rustic commitment: background pages/event pages,
content scripts, popup/options pages, browser/page actions, messaging, storage,
host permissions and blocking request hooks are required. Do not silently replace
blocking callbacks with a restricted MV3 rule-only API. Validate supported fields,
permissions and versions; reject unsupported required fields explicitly. A native
declarative filter accelerator may coexist with MV2 callbacks without reducing
their documented behavior. MV2 semantics do not require unrestricted remote code,
`eval`, unsigned production packages or ambient native privileges.

Create separate extension principals/realms and an explicit isolated content-script
world. Isolated worlds share controlled DOM access, not page JS globals or
extension secrets. Page/extension/chrome messaging must validate sender, origin,
frame, extension ID, navigation epoch and structured-clone payload limits. Check
permissions at every broker/API boundary; never trust manifest claims from a
renderer. Browser-internal pages, other extensions and private sessions require
separate grants. Revocation takes effect on live workers, frames and requests.

Background pages need documented desktop/mobile suspension and resume behavior,
bounded tasks/storage/messaging and teardown. Android/iOS background restrictions
must not make blocking depend on an OS-suspended UI page: trusted compiled filters
remain available in the network broker, and other callback deadlines have an
explicit failure policy. Package signing alone does not establish trustworthy
code. Enforce extension CSP, review executable updates and prevent remote filter
data from becoming arbitrary extension code.

## Network and content hooks

Support MV2-style `webRequest` and `webRequestBlocking`, with scoped URL/initiator
permissions, redirect/header changes, authentication callbacks and completion/
error events. Define deterministic precedence across the built-in blocker and
multiple extensions; cancellation wins, redirect cycles are bounded, restricted
headers stay protected, and timeouts cannot silently bypass security policy.
Specify response-body filtering needed by the uBO feature matrix through a
separately reviewed streaming capability.

The broker evaluates policy before connection/body delivery and rechecks redirects,
workers, frames, cache/service-worker responses, WebSockets and extension fetches.
Filtering decisions never grant SOP/CORS/CSP authority. Requests retain attributed
document/site/profile/extension context; stale callbacks cannot modify a new
navigation. Page scripts cannot disable protection or mint extension privileges.
Cosmetic filtering and scriptlets need bounded selectors, explicit frame/run-at
semantics, isolated injection and allowlisted packaged resources.

## Rebuild/port uBlock Origin for Rustic

Use the full uBlock Origin feature model, not merely a small ad-domain denylist or
uBO Lite replacement. Maintain a versioned Rustic port with upstream source/revision
tracking, attributed changes and a published supported-feature matrix. A working
name is **Rustic Block**; do not imply an official upstream endorsement. The core
engine remains original. Porting upstream extension logic is a separate licensed
application layer; reviewed Rust acceleration must preserve its filter semantics.

Required parity includes static network filters/exceptions, request types and
party/domain rules, cosmetic and procedural filters, packaged scriptlets and
redirect resources, dynamic URL/host/firewall rules for advanced users, strict
blocking, per-site switches, element picker/zapper, logger, dashboard, custom
filters/rules, list management, backup/restore and diagnostics. Inventory upstream
capabilities such as response/HTML filtering, query/header/CSP transformations,
large-media/remote-font and popup controls explicitly rather than dropping them
when a host API is missing. Track DNS/CNAME protections with the broker's resolver
and privacy model. Update that matrix when upstream changes.

Treat filter lists as hostile data: authenticated list metadata/update sources,
size/time/regex/selector budgets, transactional parsing/index publication,
last-known-good rollback, list-specific notices and observable failures. Rules
cannot execute downloaded code. Package scriptlets/resources separately, review
their capabilities and retain a tested reference matcher before optimizing Rust
indexes. Evaluate stored/cache responses against current policy and define filter
versioning across navigation. Keep logs local, redact credentials and make private
profile state ephemeral.

The upstream project identifies its code as GPLv3. Pin and review the exact source
license and each imported resource/list notice before copying code or distributing
the port; preserve notices, corresponding source and upstream attribution.
Rustic's existing AGPL license is unchanged. No upstream code or filter lists are
vendored by this planning change. References: [uBO source/license](https://github.com/gorhill/uBlock),
[uBO documentation](https://github.com/gorhill/uBlock/wiki) and
[MV2 API reference](https://developer.chrome.com/docs/extensions/mv2/reference/webRequest).
These describe behavior to port; Rustic's host/runtime is implemented separately.

## Task order and gates

| Tasks | Deliverable | Dependencies and acceptance |
| --- | --- | --- |
| EXT-01/02 | Manifest/package/identity and versioned API inventory | M5 host model; malformed manifests, path traversal/zip bombs, signature/key mismatch and rollback tests |
| EXT-03/04 | Isolated worlds, background lifecycle and messaging | M4/M5 realms/tracing; spoofing, cross-world leaks, suspension and revoked-grant tests |
| EXT-05/06 | Actions/options/storage and MV2 network/response hooks | M6 broker; deterministic multi-extension requests and private-profile tests |
| EXT-07/08 | Signed updates, trusted permission UI and developer tools | M7 isolation; hostile package/update, revocation and debuggability tests |
| EXT-09/10 | Desktop/mobile integration and release suite | M8/M9 device lifecycle, input/UI, isolation and supported-API corpus |
| BLK-01/02 | Upstream feature/license inventory and reference network matcher | Pinned sources/lists and independent decision corpus; no remote authority |
| BLK-03/04 | Cosmetic/procedural filters, scriptlets and broker integration | DOM/CSS/M6 APIs; hostile filters, cache/worker/redirect coverage and page-world separation |
| BLK-05/06 | Rustic uBO port UI, advanced controls and transactional list updates | EXT runtime; feature-matrix regression, backup/restore and update rollback tests |
| BLK-07/08 | Rust optimization, full feature/mobile parity and release | Reference/optimized decision parity, memory/latency/energy budgets and extension/security review |

M13 desktop and M14 five-platform acceptance require both a safe Rustic MV2 platform and the working uBO port;
an API skeleton or a filter parser alone is insufficient. Privacy defaults,
user exceptions and broken-site reporting belong in trusted browser UI.
