# M6 controlled networking completion review — 2026-11-20

## Starting revision

Reviewed `feature` from `d50d034` (M5 completion) through the M6 commits
of 2026-11-20: transport, Fetch, CORS and cookies (`0ce3c93`), the
vendored Public Suffix List (`8bcec0d`), TLS (`5871256`), content codings
(`3f9c728`), navigation, CSP and session history (`1068dbe`), HTTP loading
in the CLI (`38f6406`), encoding restart (`efad860`), profiles and Web
Storage (`58c215c`), inline-style CSP (`0aca466`), the HTTP cache
(`dea7858`), `fetch()` (`bc7af5f`) and this change (conformance gate,
demo, ADR 0055, debt rescheduling).

## Delivered behavior

| Task | Result | Records |
| --- | --- | --- |
| NET-01 transport and TLS | Own HTTP/1.1 client, loopback origin allowlist (no DNS), timeouts, cancellation, backpressure, fixture server; rustls 0.23 + ring + platform verifier; gzip, deflate and brotli | ADR 0051–0053 |
| NET-02 navigation | Hop-by-hop navigation with redirects, downloads and 204/205, origin and policy-container inheritance, Referrer Policy, `Sec-Fetch-*`, `data:` URLs, MIME sniffing, response charsets, encoding confidence and reload; the CLI loads documents and subresources over HTTP(S) from granted origins | [IMPLEMENTATION.md](../IMPLEMENTATION.md#net-02--documents-and-subresources-over-http-in-the-cli) |
| NET-03 Fetch | Main fetch with modes, tainting, CORS and preflights, credentials, CORP, filtered responses; `fetch()`, `Headers`, `Request`, `Response` | ADR 0054 |
| NET-04 cookies and cache | RFC 6265bis jar (Lax by default, blocked third-party cookies, CHIPS); partitioned RFC 9111 private cache with revalidation | ADR 0054 |
| NET-05 policy | CSP3 for scripts, `eval`, subresource URLs, inline styles and `connect-src`; mixed content; secure contexts; certificate errors without bypass | — |
| NET-06 profiles and storage | `rustic-storage`: ephemeral, private and locked persistent profiles, versioned checksummed files with quarantine, storage keys partitioned by top-level site, quotas, `localStorage` and `sessionStorage`; persistent cookies and partitioned preflight cache | — |
| NET-07 history, forms, downloads, WebSocket | Session history and form submissions; downloads broker contract and WebSocket plan | ADR 0055 |

## Gate

**Certificate verification is never silently bypassed.** `tls.rs` covers
expired, wrong-host and untrusted chains; `https_documents_verify_certificates_against_the_given_roots`
and `m6_gate_refuses_unlisted_origins_and_bad_certificates_without_bypass`
check the CLI fails closed with nothing on standard output.

**Redirect policies are rechecked.** `RedirectChain` and Fetch's redirect
steps run per hop (`redirects.rs`, `cross_origin_redirects_taint_the_origin`);
`Fetcher::fetch_checked` asks CSP and mixed content before every hop
(`client_policy_is_asked_before_every_hop`), and the CLI demo follows a
redirect into a CSP-governed page.

**Origin, storage and cache separation.** Storage keys and the HTTP and
preflight caches are partitioned by top-level site
(`storage_is_partitioned_by_top_level_site_and_cleared_per_origin`,
`fresh_responses_are_reused_within_their_partition_only`,
`preflight_grants_do_not_cross_network_partitions`); third-party cookies
are blocked (`cookie` tests); opaque origins get no Web Storage
(`web_storage_reflects_the_host_and_throws_without_one`).

**Response filtering.** Basic, CORS, opaque and opaque-redirect filtering
(`fetch.rs` tests); a CORS-less cross-origin `fetch()` is a `TypeError`
and its body never reaches script (`script_fetch_applies_cors_and_connect_src`,
the demo).

**Limits cover streaming and decompression.** Head, header, chunk and body
limits split-invariantly (`transport.rs`), a decompression bomb stops at
the body limit (`a_decompression_bomb_stops_at_the_limit`), profile files
are bounded before allocation (`readers_check_counts_before_allocating`),
and parser budgets degrade as in M5.

**Conformance.** Pinned WPT at `cb97c571`: 975 MIME-type records, 72
`data:` URL and 80 forgiving-base64 records all pass (`wpt_mime.rs`); the
Window selection adds 11 Fetch API files: 114 of 119 subtests pass, the
other five need `arrayBuffer()`, `blob()`, `formData()` and `body` streams
(TD-169). The whole selection is 70 files, 462 subtests, 387 passes, 75
expected failures with reasons.

**Demo.** `m6_demo_loads_an_allowlisted_site_and_denies_cross_origin_reads`:
two allowlisted origins on a loopback fixture server; the page arrives
through a redirect, sets a cookie, and its stylesheet, image, script,
`fetch()` and `localStorage` all work under a private profile, while a
cross-origin read is denied.

## Exit checklist

- [x] Every M6 capability is complete in the catalog (`progress/catalog.json`).
- [x] Native x86-64 passes the full gate: formatting, Clippy, all-target
      tests, doctests, rustdoc, repository and tool tests, progress check,
      `cargo deny`, pinned WPT.
- [x] MSRV 1.85.0 builds the workspace with all targets and runs its tests
      (separate rustup home, pinned toolchain untouched).
- [ ] **Tiers.** No QEMU or cross toolchains exist in this sandbox, so the
      emulated 32-bit ARM, 32-bit x86, RISC-V and s390x runs and the
      mobile cross-checks were **not run**; hosted CI is still blocked
      (TD-12). The new dependency graph is unchanged since `5871256`, whose
      review recorded its tier support.
- [x] M6 debt: TD-156..TD-169 recorded; TD-158, TD-159, TD-160, TD-164
      paid; TD-161 partly paid; the M6 entry row rescheduled with reasons.

## Scope decisions to confirm

- The "M6 entry" debt row was **rescheduled, not paid**: TD-138, TD-139
  and TD-140 are bad debt (JS evaluation-order and TDZ bugs) and now sit
  at the front of the M9 `js.conformance` gate; the HTML, diagnostics and
  Window items moved to M7 and M9 with their reasons in TECH_DEBT.md.
- `fetch()` blocks the script thread until its response is complete
  (TD-169); bodies are strings.
- Responses that set cookies are never stored in the HTTP cache.

## Next

M7 isolation: the broker and network process (TD-161, TD-165), and the
download broker of ADR 0055.
