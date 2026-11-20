# ADR 0054 — Sites, the Public Suffix List and cookie policy

Date: 2026-11-20
Status: Accepted (bundled list accepted by the maintainer 2026-11-20)

## Context

Cookies (RFC 6265bis), `SameSite`, Cross-Origin-Resource-Policy,
partitioned storage (NET-06) and the HTML "site" concept all need the
registrable domain of a host, which comes from the Public Suffix List
(PSL). The PSL is data under MPL-2.0, about 340 KiB of text, updated
several times a month. Bundling it is a data dependency that
[DEPENDENCIES.md](../DEPENDENCIES.md) asks to review; it also goes stale
in a shipped binary unless the browser updates it.

RFC 6265bis also leaves policy room that decides privacy behaviour: how a
missing `SameSite` is enforced, whether third-party cookies are allowed,
and whether `Secure` cookies work on `http://localhost`.

## Decision

1. `rustic_net::site::PublicSuffixList` implements the PSL's published
   algorithm (rules, `*.` wildcards, `!` exceptions, the implicit `*`
   rule) over **text the host supplies** (`PublicSuffixList::parse`), like
   host-granted font discovery, and **vendors the list** at a pinned
   publicsuffix/list revision (`PublicSuffixList::bundled()`, digests in
   `crates/rustic-net/data/psl_manifest.json`, MPL-2.0 notice in
   `third_party/licenses`). Unicode rules go through domain-to-ASCII.
   `fallback()` (implicit rule only) remains for tests.
2. Sites, same-site and schemelessly-same-site follow HTML; potentially
   trustworthy origins follow Secure Contexts (`https`, `wss`,
   `127.0.0.0/8`, `::1`, `localhost` and `*.localhost`).
3. Cookie policy, each matching Chromium and, for third parties, Brave:
   - a missing or unknown `SameSite` is enforced as `Lax`;
   - `SameSite=None` requires `Secure`;
   - `Secure` cookies may be set and sent on potentially trustworthy
     `http` origins;
   - unpartitioned cookies are neither sent nor stored on cross-site
     subresource requests (`ThirdPartyCookies::Block`, the default);
     `Partitioned` (CHIPS) cookies, which require `Secure`, are keyed by
     the top-level site;
   - expiry is capped at 400 days, and the jar holds at most 180 cookies
     per site and 3,000 in all, evicting expired, then non-secure, then
     least recently used cookies.
4. The jar is in memory and clock-free (callers pass Unix seconds).
   Persistence and per-profile separation are NET-06.

## Alternatives considered

- **Vendor the PSL now.** Simplest at runtime, but adds an MPL-2.0 data
  file and an update process without review. Kept as the proposed option
  for the maintainer: vendor a pinned revision with a digest-checked
  import script, as the WPT and Unicode data are.
- **Read the system copy** (`/usr/share/publicsuffix`). Not present on
  Windows, macOS, Android or iOS, and Fedora ships it as a DAFSA, not
  text.
- **An existing crate** (`publicsuffix`, `psl`). A dependency for a small
  algorithm; the data question stays.
- **Firefox's `SameSite` default (`None`).** Weaker against CSRF and
  cross-site tracking; Rustic's privacy target is Brave-comparable.

## Consequences

- The vendored list goes stale; `tools/import_public_suffix_list.py`
  re-pins it, and a profile may supply a newer list at runtime.
- Lax-by-default breaks some cross-site `POST` login flows that Chromium
  works around with a two-minute "Lax-allowing-unsafe" window; Rustic
  does not have that window.

## Validation

`crates/rustic-net/tests/public_suffix.rs` (all 78 upstream vectors),
`site.rs`, `cookie/` and `crates/rustic-net/tests/fetch.rs` tests;
`random_sequences_keep_the_jar_invariants` checks jar invariants over
3,000 random stores.

## Revisit when

The list needs updating for a release, or profiles persist cookies
(NET-06).
