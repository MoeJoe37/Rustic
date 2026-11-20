//! Cookies (RFC 6265bis-15): `Set-Cookie` parsing, the storage model
//! (§5.7), retrieval for a request (§5.8.3), `SameSite`, `Secure`,
//! `HttpOnly`, the `__Secure-`/`__Host-` prefixes, partitioned cookies
//! (CHIPS) and bounded eviction.
//!
//! Time is Unix seconds supplied by the caller, so the jar is
//! deterministic under test. The jar is in memory; persistence belongs to
//! profiles (NET-06).
//!
//! Rustic's choices where the draft leaves room, each matching Chromium:
//! a missing or unknown `SameSite` is enforced as `Lax`; `Secure` cookies
//! may be set and sent on potentially trustworthy origins such as
//! `http://localhost`; third-party cookies are blocked unless partitioned
//! (Brave's default); limits are 180 cookies per site and 3,000 in all.

mod parse;
mod persist;

use std::fmt;

use rustic_url::{Host, Url};

pub use parse::{
    MAX_AGE_SECONDS, MAX_ATTRIBUTE_VALUE_BYTES, MAX_NAME_VALUE_BYTES, ParsedCookie, SameSite,
    parse_cookie_date, parse_set_cookie,
};

use crate::site::{PublicSuffixList, Site, is_potentially_trustworthy_url};

/// A stored cookie (§5.7 "cookie" fields).
#[derive(Clone, Eq, PartialEq)]
pub struct Cookie {
    name: String,
    value: String,
    /// Unix seconds; `None` for a session cookie.
    expiry: Option<i64>,
    domain: String,
    path: String,
    creation: u64,
    last_access: u64,
    host_only: bool,
    secure_only: bool,
    http_only: bool,
    same_site: SameSite,
    partition: Option<Site>,
}

impl Cookie {
    /// The name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The value.
    pub fn value(&self) -> &str {
        &self.value
    }
    /// The domain (the request host for host-only cookies).
    pub fn domain(&self) -> &str {
        &self.domain
    }
    /// The path.
    pub fn path(&self) -> &str {
        &self.path
    }
    /// Expiry in Unix seconds; `None` for a session cookie.
    pub fn expiry(&self) -> Option<i64> {
        self.expiry
    }
    /// Whether only the exact host receives the cookie.
    pub fn host_only(&self) -> bool {
        self.host_only
    }
    /// `Secure`.
    pub fn secure_only(&self) -> bool {
        self.secure_only
    }
    /// `HttpOnly`.
    pub fn http_only(&self) -> bool {
        self.http_only
    }
    /// The enforced `SameSite` value.
    pub fn same_site(&self) -> SameSite {
        self.same_site
    }
    /// The partition (top-level site) of a partitioned cookie.
    pub fn partition(&self) -> Option<&Site> {
        self.partition.as_ref()
    }

    fn same_identity(&self, other: &Self) -> bool {
        self.name == other.name
            && self.domain == other.domain
            && self.host_only == other.host_only
            && self.path == other.path
            && self.partition == other.partition
    }

    fn is_expired(&self, now: i64) -> bool {
        self.expiry.is_some_and(|expiry| expiry <= now)
    }
}

impl fmt::Debug for Cookie {
    /// Name, domain and path; never the value.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Cookie")
            .field("name", &self.name)
            .field("domain", &self.domain)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The request a cookie is set from or sent with.
#[derive(Clone, Debug)]
pub struct CookieContext<'a> {
    /// The request's current URL.
    pub url: &'a Url,
    /// Whether the request is "same-site" (§5.2): its site for cookies
    /// is same-site with the URL, along the whole redirect chain.
    pub same_site: bool,
    /// Whether the request navigates a top-level traversable.
    pub top_level_navigation: bool,
    /// Whether the method is safe (`GET`, `HEAD`, `OPTIONS`, `TRACE`).
    pub safe_method: bool,
    /// `true` for HTTP headers, `false` for `document.cookie`.
    pub http_api: bool,
    /// The top-level site, for partitioned cookies (`None` when there
    /// is no top-level document, e.g. a CLI fetch).
    pub top_level_site: Option<Site>,
}

impl<'a> CookieContext<'a> {
    /// A same-site, top-level `GET` over HTTP: what a user-initiated
    /// navigation looks like.
    pub fn navigation(url: &'a Url, top_level_site: Option<Site>) -> Self {
        Self {
            url,
            same_site: true,
            top_level_navigation: true,
            safe_method: true,
            http_api: true,
            top_level_site,
        }
    }
}

/// Bounds on the jar (§6.1 minimums are 50 per domain and 3,000 total).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CookieLimits {
    /// Cookies per site (registrable domain of the cookie's domain).
    pub per_site: usize,
    /// Cookies in the jar.
    pub total: usize,
}

impl Default for CookieLimits {
    fn default() -> Self {
        Self {
            per_site: 180,
            total: 3000,
        }
    }
}

/// Why a received cookie was ignored. Reported for diagnostics; the
/// response is otherwise unaffected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieRejection {
    /// The header failed `Set-Cookie` parsing.
    Malformed,
    /// A nameless cookie's value looks like a prefixed name.
    NamelessPrefix,
    /// `Domain` does not domain-match the request host, or is a public
    /// suffix other than the host.
    DomainMismatch,
    /// `Secure` from a request that is not secure.
    InsecureSecure,
    /// An `HttpOnly` cookie from `document.cookie`, or overwriting one.
    HttpOnly,
    /// A non-secure cookie would shadow a secure one.
    ShadowsSecure,
    /// `SameSite` other than `None` from a cross-site context.
    CrossSite,
    /// `SameSite=None` without `Secure`.
    NoneWithoutSecure,
    /// `__Secure-` or `__Host-` requirements not met.
    Prefix,
    /// `Partitioned` without `Secure` (or without a top-level site).
    Partitioned,
    /// An unpartitioned third-party cookie, blocked by policy.
    ThirdParty,
    /// The request URL has no host.
    NoHost,
}

impl fmt::Display for CookieRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, formatter)
    }
}

impl std::error::Error for CookieRejection {}

/// Whether cross-site requests may use unpartitioned cookies.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThirdPartyCookies {
    /// Only partitioned cookies on cross-site requests (default).
    #[default]
    Block,
    /// Unpartitioned cookies too (subject to `SameSite`).
    Allow,
}

/// An in-memory cookie store.
pub struct CookieJar {
    cookies: Vec<Cookie>,
    suffixes: PublicSuffixList,
    limits: CookieLimits,
    third_party: ThirdPartyCookies,
    clock: u64,
}

impl fmt::Debug for CookieJar {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CookieJar")
            .field("cookies", &self.cookies.len())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Default for CookieJar {
    fn default() -> Self {
        Self::new(PublicSuffixList::bundled(), CookieLimits::default())
    }
}

/// RFC 6265bis §5.1.3 "domain-match".
pub fn domain_match(host: &str, domain: &str, host_is_ip: bool) -> bool {
    host == domain
        || (!host_is_ip
            && host.len() > domain.len()
            && host.ends_with(domain)
            && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
}

/// RFC 6265bis §5.1.4 "default-path".
pub fn default_path(url: &Url) -> String {
    let path = url.pathname();
    if !path.starts_with('/') {
        return "/".to_owned();
    }
    match path.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => path[..index].to_owned(),
    }
}

/// RFC 6265bis §5.1.4 "path-match".
pub fn path_match(request_path: &str, cookie_path: &str) -> bool {
    request_path == cookie_path
        || (request_path.starts_with(cookie_path)
            && (cookie_path.ends_with('/')
                || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')))
}

fn has_prefix_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

impl CookieJar {
    /// An empty jar.
    pub fn new(suffixes: PublicSuffixList, limits: CookieLimits) -> Self {
        Self {
            cookies: Vec::new(),
            suffixes,
            limits,
            third_party: ThirdPartyCookies::default(),
            clock: 0,
        }
    }

    /// Change the third-party cookie policy.
    pub fn set_third_party(&mut self, policy: ThirdPartyCookies) {
        self.third_party = policy;
    }

    /// The public suffix list in use.
    pub fn suffixes(&self) -> &PublicSuffixList {
        &self.suffixes
    }

    /// Stored cookies, including expired ones not yet evicted.
    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Number of stored cookies.
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Whether the jar is empty.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Remove every cookie.
    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Run the storage model for one `Set-Cookie` value received at `now`.
    pub fn set_cookie(
        &mut self,
        header: &str,
        context: &CookieContext<'_>,
        now: i64,
    ) -> Result<(), CookieRejection> {
        let parsed = parse_set_cookie(header).ok_or(CookieRejection::Malformed)?;
        self.store(parsed, context, now)
    }

    fn store(
        &mut self,
        parsed: ParsedCookie,
        context: &CookieContext<'_>,
        now: i64,
    ) -> Result<(), CookieRejection> {
        let url = context.url;
        let host = url.host().ok_or(CookieRejection::NoHost)?;
        let host_is_ip = host.domain().is_none();
        let request_host = host.serialize();
        if parsed.name.is_empty()
            && (has_prefix_ignore_case(&parsed.value, "__Secure-")
                || has_prefix_ignore_case(&parsed.value, "__Host-"))
        {
            return Err(CookieRejection::NamelessPrefix);
        }
        // §5.7 step 6 (Max-Age wins over Expires), capped at 400 days.
        let cap = now.saturating_add(MAX_AGE_SECONDS);
        let expiry = match (parsed.max_age, parsed.expires) {
            (Some(age), _) if age <= 0 => Some(i64::MIN),
            (Some(age), _) => Some(now.saturating_add(age).min(cap)),
            (None, Some(time)) => Some(time.min(cap)),
            (None, None) => None,
        };
        // Steps 7–9: Domain, public suffixes and host-only.
        let mut domain_attribute = parsed.domain.clone().unwrap_or_default();
        if !domain_attribute.is_empty()
            && !host_is_ip
            && self
                .suffixes
                .registrable_domain(&domain_attribute)
                .is_none()
        {
            if domain_attribute == request_host {
                domain_attribute.clear();
            } else {
                return Err(CookieRejection::DomainMismatch);
            }
        }
        let (domain, host_only) = if domain_attribute.is_empty() {
            (request_host.clone(), true)
        } else {
            if !domain_match(&request_host, &domain_attribute, host_is_ip) {
                return Err(CookieRejection::DomainMismatch);
            }
            (domain_attribute, false)
        };
        let path = parsed.path.clone().unwrap_or_else(|| default_path(url));
        let secure_request = is_potentially_trustworthy_url(url);
        if parsed.secure && !secure_request {
            return Err(CookieRejection::InsecureSecure);
        }
        if parsed.http_only && !context.http_api {
            return Err(CookieRejection::HttpOnly);
        }
        // Step 12: a non-secure cookie cannot shadow a secure one.
        if !parsed.secure && !secure_request {
            let shadows = self.cookies.iter().any(|existing| {
                existing.name == parsed.name
                    && existing.secure_only
                    && (domain_match(&domain, &existing.domain, host_is_ip)
                        || domain_match(&existing.domain, &domain, host_is_ip))
                    && path_match(&path, &existing.path)
            });
            if shadows {
                return Err(CookieRejection::ShadowsSecure);
            }
        }
        // Steps 13–15: SameSite.
        if parsed.same_site != SameSite::None
            && !context.same_site
            && !(context.http_api && context.top_level_navigation)
        {
            return Err(CookieRejection::CrossSite);
        }
        if parsed.same_site == SameSite::None && !parsed.secure {
            return Err(CookieRejection::NoneWithoutSecure);
        }
        // Steps 16–17: prefixes (case-insensitive, as the draft requires).
        if has_prefix_ignore_case(&parsed.name, "__Secure-") && !parsed.secure {
            return Err(CookieRejection::Prefix);
        }
        if has_prefix_ignore_case(&parsed.name, "__Host-")
            && !(parsed.secure && host_only && path == "/")
        {
            return Err(CookieRejection::Prefix);
        }
        // CHIPS: partitioned cookies need Secure and a top-level site.
        let partition = if parsed.partitioned {
            if !parsed.secure {
                return Err(CookieRejection::Partitioned);
            }
            Some(
                context
                    .top_level_site
                    .clone()
                    .ok_or(CookieRejection::Partitioned)?,
            )
        } else {
            None
        };
        if partition.is_none() && !self.allows_unpartitioned(context) {
            return Err(CookieRejection::ThirdParty);
        }
        let creation = self.tick();
        let mut cookie = Cookie {
            name: parsed.name,
            value: parsed.value,
            expiry,
            domain,
            path,
            creation,
            last_access: creation,
            host_only,
            secure_only: parsed.secure,
            http_only: parsed.http_only,
            same_site: parsed.same_site,
            partition,
        };
        // Step 18: replace an existing cookie with the same identity.
        if let Some(index) = self
            .cookies
            .iter()
            .position(|old| old.same_identity(&cookie))
        {
            if self.cookies[index].http_only && !context.http_api {
                return Err(CookieRejection::HttpOnly);
            }
            cookie.creation = self.cookies[index].creation;
            self.cookies.remove(index);
        }
        if !cookie.is_expired(now) {
            self.cookies.push(cookie);
            self.evict(now);
        }
        Ok(())
    }

    /// Third-party blocking: a cross-site subresource request (not a
    /// top-level navigation) gets only partitioned cookies.
    fn allows_unpartitioned(&self, context: &CookieContext<'_>) -> bool {
        context.same_site
            || context.top_level_navigation
            || self.third_party == ThirdPartyCookies::Allow
    }

    fn site_of(&self, domain: &str) -> String {
        self.suffixes
            .registrable_domain(domain)
            .unwrap_or(domain)
            .to_owned()
    }

    /// §5.7 "remove excess cookies": expired first, then per site
    /// non-secure before secure, then the whole jar; least recently
    /// accessed first within each class.
    fn evict(&mut self, now: i64) {
        self.cookies.retain(|cookie| !cookie.is_expired(now));
        let mut sites: Vec<String> = self
            .cookies
            .iter()
            .map(|cookie| self.site_of(&cookie.domain))
            .collect();
        let mut unique = sites.clone();
        unique.sort_unstable();
        unique.dedup();
        for site in unique {
            loop {
                let members: Vec<usize> = (0..self.cookies.len())
                    .filter(|&index| sites[index] == site)
                    .collect();
                if members.len() <= self.limits.per_site {
                    break;
                }
                let victim = members
                    .iter()
                    .copied()
                    .min_by_key(|&index| {
                        let cookie = &self.cookies[index];
                        (cookie.secure_only, cookie.last_access)
                    })
                    .expect("over the limit means non-empty");
                self.cookies.remove(victim);
                sites.remove(victim);
            }
        }
        while self.cookies.len() > self.limits.total {
            let victim = (0..self.cookies.len())
                .min_by_key(|&index| self.cookies[index].last_access)
                .expect("over the limit means non-empty");
            self.cookies.remove(victim);
        }
    }

    /// §5.8.3: the cookies to send for `context`, longest path first, then
    /// oldest; their last-access times are updated.
    pub fn cookies_for(&mut self, context: &CookieContext<'_>, now: i64) -> Vec<&Cookie> {
        let Some(host) = context.url.host() else {
            return Vec::new();
        };
        let host_is_ip = host.domain().is_none();
        let request_host = host.serialize();
        let path = context.url.pathname();
        let secure = is_potentially_trustworthy_url(context.url);
        let allow_unpartitioned = self.allows_unpartitioned(context);
        let mut selected: Vec<usize> = (0..self.cookies.len())
            .filter(|&index| {
                let cookie = &self.cookies[index];
                if cookie.is_expired(now) {
                    return false;
                }
                let host_ok = if cookie.host_only {
                    request_host == cookie.domain
                } else {
                    domain_match(&request_host, &cookie.domain, host_is_ip)
                };
                let partition_ok = match &cookie.partition {
                    Some(partition) => context.top_level_site.as_ref() == Some(partition),
                    None => allow_unpartitioned,
                };
                let same_site_ok = context.same_site
                    || match cookie.same_site {
                        SameSite::None => true,
                        SameSite::Strict => false,
                        SameSite::Lax | SameSite::Default => {
                            context.top_level_navigation && context.safe_method
                        }
                    };
                host_ok
                    && partition_ok
                    && path_match(&path, &cookie.path)
                    && (!cookie.secure_only || secure)
                    && (!cookie.http_only || context.http_api)
                    && same_site_ok
            })
            .collect();
        selected.sort_by(|&a, &b| {
            let (a, b) = (&self.cookies[a], &self.cookies[b]);
            b.path
                .len()
                .cmp(&a.path.len())
                .then(a.creation.cmp(&b.creation))
        });
        for &index in &selected {
            let tick = self.tick();
            self.cookies[index].last_access = tick;
        }
        selected.iter().map(|&index| &self.cookies[index]).collect()
    }

    /// §5.8.3 step 4: the `Cookie` header value, or `None` when no cookie
    /// applies.
    pub fn cookie_header(&mut self, context: &CookieContext<'_>, now: i64) -> Option<String> {
        let cookies = self.cookies_for(context, now);
        if cookies.is_empty() {
            return None;
        }
        let parts: Vec<String> = cookies
            .iter()
            .map(|cookie| {
                if cookie.name.is_empty() {
                    cookie.value.clone()
                } else {
                    format!("{}={}", cookie.name, cookie.value)
                }
            })
            .collect();
        Some(parts.join("; "))
    }

    /// Remove expired cookies now rather than at the next store.
    pub fn purge_expired(&mut self, now: i64) {
        self.cookies.retain(|cookie| !cookie.is_expired(now));
    }

    /// Remove every cookie whose domain belongs to `host`'s site (a
    /// "clear site data" primitive for profiles).
    pub fn clear_site(&mut self, host: &Host) {
        let target = self.suffixes.site_host(host);
        let suffixes = &self.suffixes;
        self.cookies.retain(|cookie| {
            suffixes
                .registrable_domain(&cookie.domain)
                .unwrap_or(&cookie.domain)
                != target
        });
    }
}

#[cfg(test)]
mod tests;
