//! Fetch's header classifications (Fetch §2.2.2), the CORS check (§4.9),
//! CORS-preflight requests and their validation (§4.8) and the
//! CORS-preflight cache (§4.8, "CORS-preflight cache").
//!
//! Everything here is pure: callers pass the clock reading, so the cache
//! is deterministic under test.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rustic_url::Url;

use crate::Headers;
use crate::headers::is_token;
use crate::mime::{MimeType, split_header_value};

/// Fetch "forbidden method".
pub fn is_forbidden_method(method: &str) -> bool {
    ["CONNECT", "TRACE", "TRACK"]
        .iter()
        .any(|forbidden| forbidden.eq_ignore_ascii_case(method))
}

/// Fetch "normalize" a method: the six standard methods are uppercased.
pub fn normalize_method(method: &str) -> String {
    for standard in ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"] {
        if standard.eq_ignore_ascii_case(method) {
            return standard.to_owned();
        }
    }
    method.to_owned()
}

/// Fetch "CORS-safelisted method".
pub fn is_cors_safelisted_method(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "POST")
}

const FORBIDDEN_REQUEST_HEADER_NAMES: [&str; 21] = [
    "accept-charset",
    "accept-encoding",
    "access-control-request-headers",
    "access-control-request-method",
    "connection",
    "content-length",
    "cookie",
    "cookie2",
    "date",
    "dnt",
    "expect",
    "host",
    "keep-alive",
    "origin",
    "referer",
    "set-cookie",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "via",
];

fn one_of(name: &str, list: &[&str]) -> bool {
    list.iter().any(|entry| entry.eq_ignore_ascii_case(name))
}

fn has_prefix_ignore_case(name: &str, prefix: &str) -> bool {
    name.len() >= prefix.len()
        && name.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Fetch "forbidden request-header": headers a script may not set.
pub fn is_forbidden_request_header(name: &str, value: &[u8]) -> bool {
    if one_of(name, &FORBIDDEN_REQUEST_HEADER_NAMES)
        || has_prefix_ignore_case(name, "proxy-")
        || has_prefix_ignore_case(name, "sec-")
    {
        return true;
    }
    if one_of(
        name,
        &[
            "x-http-method",
            "x-http-method-override",
            "x-method-override",
        ],
    ) {
        return split_header_value(value)
            .iter()
            .any(|method| is_forbidden_method(method));
    }
    false
}

/// Fetch "forbidden response-header name".
pub fn is_forbidden_response_header_name(name: &str) -> bool {
    one_of(name, &["set-cookie", "set-cookie2"])
}

/// Fetch "CORS-unsafe request-header byte".
fn is_cors_unsafe_byte(byte: u8) -> bool {
    (byte < 0x20 && byte != 0x09) || b"\"():<>?@[\\]{}\x7f".contains(&byte)
}

/// Fetch "CORS-safelisted request-header".
pub fn is_cors_safelisted_request_header(name: &str, value: &[u8]) -> bool {
    if value.len() > 128 {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "accept" => !value.iter().copied().any(is_cors_unsafe_byte),
        "accept-language" | "content-language" => value
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || b" *,-.;=".contains(&byte)),
        "content-type" => {
            if value.iter().copied().any(is_cors_unsafe_byte) {
                return false;
            }
            let text: String = value.iter().map(|&byte| char::from(byte)).collect();
            MimeType::parse(&text).is_some_and(|mime| {
                matches!(
                    mime.essence().as_str(),
                    "application/x-www-form-urlencoded" | "multipart/form-data" | "text/plain"
                )
            })
        }
        // Suffix ranges (`bytes=-N`) are not safelisted.
        "range" => matches!(parse_single_range(value, false), Some((Some(_), _))),
        _ => false,
    }
}

/// Fetch "parse a single range header value": `bytes=start-end` with
/// either side optional (not both). With `allow_whitespace` false, no
/// HTTP whitespace is accepted anywhere.
pub fn parse_single_range(
    value: &[u8],
    allow_whitespace: bool,
) -> Option<(Option<u64>, Option<u64>)> {
    let text = std::str::from_utf8(value).ok()?;
    let skip = |input: &str| -> String {
        if allow_whitespace {
            input.trim_matches([' ', '\t']).to_owned()
        } else {
            input.to_owned()
        }
    };
    let (unit, rest) = text.split_once('=')?;
    if !skip(unit).eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (start, end) = rest.split_once('-')?;
    let number = |part: &str| -> Option<Option<u64>> {
        let part = skip(part);
        if part.is_empty() {
            Some(None)
        } else if part.bytes().all(|byte| byte.is_ascii_digit()) {
            part.parse().ok().map(Some)
        } else {
            None
        }
    };
    let (start, end) = (number(start)?, number(end)?);
    match (start, end) {
        (None, None) => None,
        (Some(start), Some(end)) if start > end => None,
        range => Some(range),
    }
}

/// Fetch "no-CORS-safelisted request-header".
pub fn is_no_cors_safelisted_request_header(name: &str, value: &[u8]) -> bool {
    one_of(
        name,
        &[
            "accept",
            "accept-language",
            "content-language",
            "content-type",
        ],
    ) && is_cors_safelisted_request_header(name, value)
}

/// Fetch "CORS non-wildcard request-header name".
pub fn is_cors_non_wildcard_request_header_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
}

/// Fetch "CORS-safelisted response-header name" given the exposed list.
pub fn is_cors_safelisted_response_header_name(name: &str, exposed: &[String]) -> bool {
    one_of(
        name,
        &[
            "cache-control",
            "content-language",
            "content-length",
            "content-type",
            "expires",
            "last-modified",
            "pragma",
        ],
    ) || (!is_forbidden_response_header_name(name)
        && exposed.iter().any(|entry| entry.eq_ignore_ascii_case(name)))
}

/// Fetch "CORS-unsafe request-header names": lowercase, sorted, unique.
/// Safelisted headers count as unsafe too once their values total more
/// than 1,024 bytes.
pub fn cors_unsafe_request_header_names(headers: &Headers) -> Vec<String> {
    let mut unsafe_names = Vec::new();
    let mut safe_names = Vec::new();
    let mut safe_bytes = 0usize;
    for (name, value) in headers.iter() {
        if is_cors_safelisted_request_header(name, value) {
            safe_names.push(name);
            safe_bytes = safe_bytes.saturating_add(value.len());
        } else {
            unsafe_names.push(name);
        }
    }
    if safe_bytes > 1024 {
        unsafe_names.extend(safe_names);
    }
    let mut names: Vec<String> = unsafe_names
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// A list header had an element that is not a token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTokenList;

/// Fetch "extract header list values" for a `#token` list: `None` when
/// the header is absent, `Err` when an element is not a token.
pub fn extract_token_list(
    headers: &Headers,
    name: &str,
) -> Result<Option<Vec<String>>, InvalidTokenList> {
    let Some(combined) = headers.combined(name) else {
        return Ok(None);
    };
    let mut values = Vec::new();
    for value in split_header_value(&combined) {
        if value.is_empty() {
            continue;
        }
        if !is_token(value.as_bytes()) {
            return Err(InvalidTokenList);
        }
        values.push(value);
    }
    Ok(Some(values))
}

/// Request credentials mode (Fetch "credentials mode").
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum CredentialsMode {
    /// Never send or store credentials.
    Omit,
    /// Credentials for same-origin requests only.
    #[default]
    SameOrigin,
    /// Always send and store credentials.
    Include,
}

/// Why a CORS check or preflight failed. Messages never include header
/// values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorsError {
    /// `Access-Control-Allow-Origin` is missing.
    MissingAllowOrigin,
    /// `Access-Control-Allow-Origin` does not match the request origin, or
    /// is `*` for a credentialed request.
    AllowOriginMismatch,
    /// A credentialed request's response lacks
    /// `Access-Control-Allow-Credentials: true`.
    CredentialsNotAllowed,
    /// The preflight response status was not 2xx.
    PreflightStatus,
    /// An `Access-Control-Allow-Methods` or `-Headers` element is not a
    /// token.
    InvalidPreflightList,
    /// The preflight did not allow the method.
    MethodNotAllowed,
    /// The preflight did not allow a request header.
    HeaderNotAllowed,
}

impl std::fmt::Display for CorsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingAllowOrigin => "CORS: no Access-Control-Allow-Origin",
            Self::AllowOriginMismatch => "CORS: Access-Control-Allow-Origin does not match",
            Self::CredentialsNotAllowed => "CORS: credentials not allowed",
            Self::PreflightStatus => "CORS: preflight response status is not ok",
            Self::InvalidPreflightList => "CORS: malformed preflight allow list",
            Self::MethodNotAllowed => "CORS: method not allowed by preflight",
            Self::HeaderNotAllowed => "CORS: request header not allowed by preflight",
        })
    }
}

impl std::error::Error for CorsError {}

/// Fetch §4.9 "CORS check". `request_origin` is the byte-serialized
/// request origin (`null` when tainted or opaque).
pub fn cors_check(
    request_origin: &str,
    credentials: CredentialsMode,
    response: &Headers,
) -> Result<(), CorsError> {
    let origin = response
        .combined("access-control-allow-origin")
        .ok_or(CorsError::MissingAllowOrigin)?;
    if credentials != CredentialsMode::Include && origin == b"*" {
        return Ok(());
    }
    if origin != request_origin.as_bytes() {
        return Err(CorsError::AllowOriginMismatch);
    }
    if credentials != CredentialsMode::Include {
        return Ok(());
    }
    match response.combined("access-control-allow-credentials") {
        Some(value) if value == b"true" => Ok(()),
        _ => Err(CorsError::CredentialsNotAllowed),
    }
}

/// The exposed header names of a CORS response (Fetch main fetch,
/// "CORS-exposed header-name list"). `*` exposes every name unless the
/// request was credentialed. A malformed list exposes nothing extra.
pub fn exposed_header_names(response: &Headers, credentials: CredentialsMode) -> Vec<String> {
    let Ok(Some(names)) = extract_token_list(response, "access-control-expose-headers") else {
        return Vec::new();
    };
    if credentials != CredentialsMode::Include && names.iter().any(|name| name == "*") {
        let mut all: Vec<String> = Vec::new();
        for (name, _) in response.iter() {
            if !all.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
                all.push(name.to_owned());
            }
        }
        return all;
    }
    names
}

/// The headers of a CORS-preflight request (Fetch §4.8 steps 1–3):
/// `Accept: */*`, `Access-Control-Request-Method` and, when there are
/// unsafe names, `Access-Control-Request-Headers` joined with `,`.
pub fn preflight_headers(method: &str, request_headers: &Headers) -> Headers {
    let mut headers = Headers::new();
    headers.append("Accept", b"*/*").expect("constant header");
    headers
        .append("Access-Control-Request-Method", method.as_bytes())
        .expect("methods are tokens");
    let names = cors_unsafe_request_header_names(request_headers);
    if !names.is_empty() {
        headers
            .append("Access-Control-Request-Headers", names.join(",").as_bytes())
            .expect("header names are tokens");
    }
    headers
}

/// A successful preflight's grant, ready for [`PreflightCache::insert`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightGrant {
    /// Allowed methods as listed (`*` included when present).
    pub methods: Vec<String>,
    /// Allowed header names as listed (`*` included when present).
    pub header_names: Vec<String>,
    /// How long the grant may be cached.
    pub max_age: Duration,
}

/// Chromium's upper bound on `Access-Control-Max-Age` (two hours).
pub const MAX_PREFLIGHT_AGE: Duration = Duration::from_secs(7200);
/// Fetch's default when `Access-Control-Max-Age` is absent.
pub const DEFAULT_PREFLIGHT_AGE: Duration = Duration::from_secs(5);

/// Fetch §4.8 steps 7.1–7.9: validate a preflight response for a request
/// with `method`, `request_headers` and `credentials`.
pub fn validate_preflight(
    status: u16,
    response: &Headers,
    request_origin: &str,
    credentials: CredentialsMode,
    method: &str,
    request_headers: &Headers,
    use_cors_preflight: bool,
) -> Result<PreflightGrant, CorsError> {
    cors_check(request_origin, credentials, response)?;
    if !(200..300).contains(&status) {
        return Err(CorsError::PreflightStatus);
    }
    let methods = extract_token_list(response, "access-control-allow-methods")
        .map_err(|InvalidTokenList| CorsError::InvalidPreflightList)?;
    let header_names = extract_token_list(response, "access-control-allow-headers")
        .map_err(|InvalidTokenList| CorsError::InvalidPreflightList)?
        .unwrap_or_default();
    let methods = match methods {
        None if use_cors_preflight => vec![method.to_owned()],
        other => other.unwrap_or_default(),
    };
    let credentialed = credentials == CredentialsMode::Include;
    let wildcard_methods = !credentialed && methods.iter().any(|entry| entry == "*");
    if !methods.iter().any(|entry| entry == method)
        && !is_cors_safelisted_method(method)
        && !wildcard_methods
    {
        return Err(CorsError::MethodNotAllowed);
    }
    let listed = |name: &str| {
        header_names
            .iter()
            .any(|entry| entry.eq_ignore_ascii_case(name))
    };
    for (name, _) in request_headers.iter() {
        if is_cors_non_wildcard_request_header_name(name) && !listed(name) {
            return Err(CorsError::HeaderNotAllowed);
        }
    }
    let wildcard_headers = !credentialed && header_names.iter().any(|entry| entry == "*");
    for name in cors_unsafe_request_header_names(request_headers) {
        if !listed(&name) && !wildcard_headers {
            return Err(CorsError::HeaderNotAllowed);
        }
    }
    // RFC 9111 delta-seconds; values too large for u64 saturate.
    let max_age = match response.get("access-control-max-age") {
        Some(value) if !value.is_empty() && value.iter().all(u8::is_ascii_digit) => {
            let seconds = std::str::from_utf8(value)
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
                .unwrap_or(u64::MAX);
            Duration::from_secs(seconds).min(MAX_PREFLIGHT_AGE)
        }
        _ => DEFAULT_PREFLIGHT_AGE,
    };
    Ok(PreflightGrant {
        methods,
        header_names,
        max_age,
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct CacheKey {
    partition: String,
    origin: String,
    url: String,
    credentials: bool,
}

#[derive(Clone, Debug, Default)]
struct CacheEntry {
    methods: Vec<(String, Instant)>,
    headers: Vec<(String, Instant)>,
}

/// Fetch's CORS-preflight cache, bounded in keys.
///
/// Keyed by the network partition key (the serialized top-level site),
/// byte-serialized origin, URL (without fragment) and whether credentials
/// were included, so one top-level site's preflights never answer for
/// another's (Fetch §4.8, "network partition key"). The methods without
/// `_in` use the empty partition.
#[derive(Debug)]
pub struct PreflightCache {
    entries: HashMap<CacheKey, CacheEntry>,
    max_keys: usize,
}

impl Default for PreflightCache {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl PreflightCache {
    /// A cache holding at most `max_keys` (origin, URL, credentials)
    /// keys; when full, expired keys are dropped first and then the
    /// cache is cleared rather than grown.
    pub fn new(max_keys: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_keys,
        }
    }

    fn key(partition: &str, origin: &str, url: &Url, credentials: bool) -> CacheKey {
        CacheKey {
            partition: partition.to_owned(),
            origin: origin.to_owned(),
            url: url.serialize_without_fragment(),
            credentials,
        }
    }

    /// Record a grant obtained at `now`.
    pub fn insert(
        &mut self,
        origin: &str,
        url: &Url,
        credentials: bool,
        grant: &PreflightGrant,
        now: Instant,
    ) {
        self.insert_in("", origin, url, credentials, grant, now);
    }

    /// [`Self::insert`] in network partition `partition`.
    pub fn insert_in(
        &mut self,
        partition: &str,
        origin: &str,
        url: &Url,
        credentials: bool,
        grant: &PreflightGrant,
        now: Instant,
    ) {
        let key = Self::key(partition, origin, url, credentials);
        if !self.entries.contains_key(&key) && self.entries.len() >= self.max_keys {
            self.entries.retain(|_, entry| {
                entry
                    .methods
                    .iter()
                    .chain(&entry.headers)
                    .any(|(_, expiry)| *expiry > now)
            });
            if self.entries.len() >= self.max_keys {
                self.entries.clear();
            }
        }
        let expiry = now + grant.max_age;
        let entry = self.entries.entry(key).or_default();
        for method in &grant.methods {
            upsert(&mut entry.methods, method, expiry, false);
        }
        for name in &grant.header_names {
            upsert(&mut entry.headers, name, expiry, true);
        }
    }

    /// Fetch "method cache entry match" (credentials-aware wildcard).
    pub fn allows_method(
        &self,
        origin: &str,
        url: &Url,
        credentials: bool,
        method: &str,
        now: Instant,
    ) -> bool {
        self.allows_method_in("", origin, url, credentials, method, now)
    }

    /// [`Self::allows_method`] in network partition `partition`.
    pub fn allows_method_in(
        &self,
        partition: &str,
        origin: &str,
        url: &Url,
        credentials: bool,
        method: &str,
        now: Instant,
    ) -> bool {
        self.lookup(partition, origin, url, credentials, |entry| {
            entry.methods.iter().any(|(name, expiry)| {
                *expiry > now && (name == method || (!credentials && name == "*"))
            })
        })
    }

    /// Fetch "header-name cache entry match".
    pub fn allows_header(
        &self,
        origin: &str,
        url: &Url,
        credentials: bool,
        header: &str,
        now: Instant,
    ) -> bool {
        self.allows_header_in("", origin, url, credentials, header, now)
    }

    /// [`Self::allows_header`] in network partition `partition`.
    pub fn allows_header_in(
        &self,
        partition: &str,
        origin: &str,
        url: &Url,
        credentials: bool,
        header: &str,
        now: Instant,
    ) -> bool {
        self.lookup(partition, origin, url, credentials, |entry| {
            entry.headers.iter().any(|(name, expiry)| {
                *expiry > now
                    && (name.eq_ignore_ascii_case(header)
                        || (!credentials
                            && name == "*"
                            && !is_cors_non_wildcard_request_header_name(header)))
            })
        })
    }

    fn lookup(
        &self,
        partition: &str,
        origin: &str,
        url: &Url,
        credentials: bool,
        found: impl Fn(&CacheEntry) -> bool,
    ) -> bool {
        self.entries
            .get(&Self::key(partition, origin, url, credentials))
            .is_some_and(found)
    }

    /// Fetch: on a failed CORS check for a request, clear entries for its
    /// (origin, URL) pair, in every partition.
    pub fn clear_for(&mut self, origin: &str, url: &Url) {
        let target = url.serialize_without_fragment();
        self.entries
            .retain(|key, _| !(key.origin == origin && key.url == target));
    }

    /// Number of (origin, URL, credentials) keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn upsert(list: &mut Vec<(String, Instant)>, name: &str, expiry: Instant, ignore_case: bool) {
    let same = |entry: &str| {
        if ignore_case {
            entry.eq_ignore_ascii_case(name)
        } else {
            entry == name
        }
    };
    if let Some(existing) = list.iter_mut().find(|(entry, _)| same(entry)) {
        existing.1 = expiry;
    } else {
        list.push((name.to_owned(), expiry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(fields: &[(&str, &str)]) -> Headers {
        let mut headers = Headers::new();
        for (name, value) in fields {
            headers.append(name, value.as_bytes()).unwrap();
        }
        headers
    }

    #[test]
    fn methods_are_classified_and_normalized() {
        assert!(is_forbidden_method("connect"));
        assert!(is_forbidden_method("TrAcK"));
        assert!(!is_forbidden_method("PATCH"));
        assert_eq!(normalize_method("post"), "POST");
        assert_eq!(normalize_method("patch"), "patch");
        assert!(is_cors_safelisted_method("POST"));
        assert!(!is_cors_safelisted_method("post"));
        assert!(!is_cors_safelisted_method("PUT"));
    }

    #[test]
    fn forbidden_request_headers() {
        for name in [
            "Cookie",
            "HOST",
            "Sec-Fetch-Mode",
            "proxy-authorization",
            "Origin",
            "DNT",
        ] {
            assert!(is_forbidden_request_header(name, b"x"), "{name}");
        }
        for name in ["Accept", "X-Custom", "Authorization", "secret", "proxy"] {
            assert!(!is_forbidden_request_header(name, b"x"), "{name}");
        }
        assert!(is_forbidden_request_header(
            "X-HTTP-Method-Override",
            b"GET, trace"
        ));
        assert!(!is_forbidden_request_header(
            "X-HTTP-Method-Override",
            b"PATCH"
        ));
        assert!(is_forbidden_response_header_name("Set-Cookie"));
        assert!(!is_forbidden_response_header_name("Set-Cookie3"));
    }

    #[test]
    fn safelisted_request_headers() {
        for (name, value, expected) in [
            ("Accept", "text/html, */*;q=0.8", true),
            ("Accept", "a\"b", false),
            ("Accept-Language", "en-US,en;q=0.5", true),
            ("Content-Language", "de/at", false),
            ("Content-Type", "text/plain;charset=utf-8", true),
            ("Content-Type", "Multipart/Form-Data; boundary=x", true),
            ("Content-Type", "application/json", false),
            ("Content-Type", "text/plain; a=\"(\"", false),
            ("Range", "bytes=0-", true),
            ("Range", "bytes=5-10", true),
            ("Range", "bytes=-5", false),
            ("Range", "bytes=10-5", false),
            ("Range", "bytes= 0-", false),
            ("Range", "bytes=0-1,2-3", false),
            ("X-Custom", "a", false),
        ] {
            assert_eq!(
                is_cors_safelisted_request_header(name, value.as_bytes()),
                expected,
                "{name}: {value}"
            );
        }
        let long = "a".repeat(129);
        assert!(!is_cors_safelisted_request_header(
            "Accept",
            long.as_bytes()
        ));
        assert!(is_cors_safelisted_request_header(
            "Accept",
            &long.as_bytes()[1..]
        ));
        assert!(is_no_cors_safelisted_request_header("Accept", b"*/*"));
        assert!(!is_no_cors_safelisted_request_header("Range", b"bytes=0-"));
        assert_eq!(
            parse_single_range(b"bytes = 1 - 2", true),
            Some((Some(1), Some(2)))
        );
        assert_eq!(
            parse_single_range(b"bytes=-5", false),
            Some((None, Some(5)))
        );
        assert_eq!(parse_single_range(b"bytes=-", false), None);
    }

    #[test]
    fn unsafe_names_are_sorted_lowercase_and_count_safelisted_bytes() {
        let list = headers(&[
            ("X-B", "1"),
            ("Accept", "*/*"),
            ("x-a", "2"),
            ("X-A", "3"),
            ("Content-Type", "application/json"),
        ]);
        assert_eq!(
            cors_unsafe_request_header_names(&list),
            ["content-type", "x-a", "x-b"]
        );
        let value = "a".repeat(128);
        let mut big = Headers::new();
        for _ in 0..9 {
            big.append("Accept-Language", value.as_bytes()).unwrap();
        }
        assert_eq!(cors_unsafe_request_header_names(&big), ["accept-language"]);
        let preflight = preflight_headers("PUT", &list);
        assert_eq!(preflight.get("accept"), Some(&b"*/*"[..]));
        assert_eq!(
            preflight.get("access-control-request-method"),
            Some(&b"PUT"[..])
        );
        assert_eq!(
            preflight.get("access-control-request-headers"),
            Some(&b"content-type,x-a,x-b"[..])
        );
        assert!(
            !preflight_headers("GET", &Headers::new()).contains("access-control-request-headers")
        );
    }

    #[test]
    fn cors_check_follows_the_algorithm() {
        let origin = "http://a.test";
        let omit = CredentialsMode::Omit;
        let include = CredentialsMode::Include;
        assert_eq!(
            cors_check(origin, omit, &headers(&[])),
            Err(CorsError::MissingAllowOrigin)
        );
        assert_eq!(
            cors_check(
                origin,
                omit,
                &headers(&[("Access-Control-Allow-Origin", "*")])
            ),
            Ok(())
        );
        assert_eq!(
            cors_check(
                origin,
                include,
                &headers(&[("Access-Control-Allow-Origin", "*")])
            ),
            Err(CorsError::AllowOriginMismatch)
        );
        assert_eq!(
            cors_check(
                origin,
                omit,
                &headers(&[("Access-Control-Allow-Origin", "http://a.test/")])
            ),
            Err(CorsError::AllowOriginMismatch)
        );
        // Two values combine to "a, b", which never matches.
        assert_eq!(
            cors_check(
                origin,
                omit,
                &headers(&[
                    ("Access-Control-Allow-Origin", "http://a.test"),
                    ("Access-Control-Allow-Origin", "http://a.test"),
                ])
            ),
            Err(CorsError::AllowOriginMismatch)
        );
        let exact = [("Access-Control-Allow-Origin", "http://a.test")];
        assert_eq!(cors_check(origin, omit, &headers(&exact)), Ok(()));
        assert_eq!(
            cors_check(origin, include, &headers(&exact)),
            Err(CorsError::CredentialsNotAllowed)
        );
        let credentialed = [
            ("Access-Control-Allow-Origin", "http://a.test"),
            ("Access-Control-Allow-Credentials", "True"),
        ];
        assert_eq!(
            cors_check(origin, include, &headers(&credentialed)),
            Err(CorsError::CredentialsNotAllowed)
        );
        let credentialed = [
            ("Access-Control-Allow-Origin", "http://a.test"),
            ("Access-Control-Allow-Credentials", "true"),
        ];
        assert_eq!(cors_check(origin, include, &headers(&credentialed)), Ok(()));
        assert_eq!(
            cors_check(
                "null",
                omit,
                &headers(&[("Access-Control-Allow-Origin", "null")])
            ),
            Ok(())
        );
    }

    #[test]
    fn exposed_names_and_safelisted_response_headers() {
        let response = headers(&[
            ("Access-Control-Expose-Headers", "X-A, x-b"),
            ("X-A", "1"),
            ("Set-Cookie", "a=b"),
        ]);
        let exposed = exposed_header_names(&response, CredentialsMode::Omit);
        assert_eq!(exposed, ["X-A", "x-b"]);
        assert!(is_cors_safelisted_response_header_name("x-a", &exposed));
        assert!(is_cors_safelisted_response_header_name("Content-Type", &[]));
        assert!(!is_cors_safelisted_response_header_name("X-C", &exposed));
        let star = headers(&[
            ("Access-Control-Expose-Headers", "*"),
            ("X-Z", "1"),
            ("Set-Cookie", "a=b"),
        ]);
        let all = exposed_header_names(&star, CredentialsMode::Omit);
        assert_eq!(all, ["Access-Control-Expose-Headers", "X-Z", "Set-Cookie"]);
        assert!(!is_cors_safelisted_response_header_name("set-cookie", &all));
        assert_eq!(exposed_header_names(&star, CredentialsMode::Include), ["*"]);
        let bad = headers(&[("Access-Control-Expose-Headers", "a b")]);
        assert!(exposed_header_names(&bad, CredentialsMode::Omit).is_empty());
    }

    fn validate(
        response: &[(&str, &str)],
        credentials: CredentialsMode,
        method: &str,
        request: &[(&str, &str)],
    ) -> Result<PreflightGrant, CorsError> {
        validate_preflight(
            204,
            &headers(response),
            "http://a.test",
            credentials,
            method,
            &headers(request),
            false,
        )
    }

    #[test]
    fn preflight_validation() {
        const ORIGIN: (&str, &str) = ("Access-Control-Allow-Origin", "http://a.test");
        let omit = CredentialsMode::Omit;
        let grant = validate(
            &[
                ORIGIN,
                ("Access-Control-Allow-Methods", "PUT, DELETE"),
                ("Access-Control-Allow-Headers", "X-A"),
                ("Access-Control-Max-Age", "60"),
            ],
            omit,
            "PUT",
            &[("x-a", "1")],
        )
        .unwrap();
        assert_eq!(grant.methods, ["PUT", "DELETE"]);
        assert_eq!(grant.max_age, Duration::from_secs(60));
        // Methods compare case-sensitively.
        assert_eq!(
            validate(
                &[ORIGIN, ("Access-Control-Allow-Methods", "put")],
                omit,
                "PUT",
                &[]
            ),
            Err(CorsError::MethodNotAllowed)
        );
        assert!(
            validate(
                &[ORIGIN, ("Access-Control-Allow-Methods", "*")],
                omit,
                "PUT",
                &[]
            )
            .is_ok()
        );
        assert_eq!(
            validate(
                &[
                    ORIGIN,
                    ("Access-Control-Allow-Methods", "*"),
                    ("Access-Control-Allow-Credentials", "true")
                ],
                CredentialsMode::Include,
                "PUT",
                &[]
            ),
            Err(CorsError::MethodNotAllowed)
        );
        assert_eq!(
            validate(
                &[ORIGIN, ("Access-Control-Allow-Headers", "*")],
                omit,
                "GET",
                &[("Authorization", "x")]
            ),
            Err(CorsError::HeaderNotAllowed)
        );
        assert!(
            validate(
                &[ORIGIN, ("Access-Control-Allow-Headers", "*, authorization")],
                omit,
                "GET",
                &[("Authorization", "x")]
            )
            .is_ok()
        );
        assert!(
            validate(
                &[ORIGIN, ("Access-Control-Allow-Headers", "*")],
                omit,
                "GET",
                &[("X-Q", "1")]
            )
            .is_ok()
        );
        assert_eq!(
            validate(
                &[ORIGIN, ("Access-Control-Allow-Headers", "x-a")],
                omit,
                "GET",
                &[("X-Q", "1")]
            ),
            Err(CorsError::HeaderNotAllowed)
        );
        assert_eq!(
            validate(
                &[ORIGIN, ("Access-Control-Allow-Methods", "PUT;")],
                omit,
                "PUT",
                &[]
            ),
            Err(CorsError::InvalidPreflightList)
        );
        assert_eq!(
            validate(&[], omit, "PUT", &[]),
            Err(CorsError::MissingAllowOrigin)
        );
        assert_eq!(
            validate_preflight(
                500,
                &headers(&[ORIGIN]),
                "http://a.test",
                omit,
                "GET",
                &Headers::new(),
                false
            ),
            Err(CorsError::PreflightStatus)
        );
        // With the use-CORS-preflight flag, a missing method list means
        // the request's own method.
        assert!(
            validate_preflight(
                200,
                &headers(&[ORIGIN]),
                "http://a.test",
                omit,
                "PUT",
                &Headers::new(),
                true
            )
            .is_ok()
        );
        let age = |value: &str| {
            validate(
                &[ORIGIN, ("Access-Control-Max-Age", value)],
                omit,
                "GET",
                &[],
            )
            .unwrap()
            .max_age
        };
        assert_eq!(age("abc"), DEFAULT_PREFLIGHT_AGE);
        assert_eq!(age("-1"), DEFAULT_PREFLIGHT_AGE);
        assert_eq!(age("99999999999999999999999"), MAX_PREFLIGHT_AGE);
        assert_eq!(age("0"), Duration::ZERO);
    }

    #[test]
    fn preflight_cache_matches_expires_and_is_bounded() {
        let url = Url::parse("http://b.test/x#frag", None).unwrap();
        let same = Url::parse("http://b.test/x", None).unwrap();
        let start = Instant::now();
        let mut cache = PreflightCache::new(2);
        let grant = PreflightGrant {
            methods: vec!["PUT".into(), "*".into()],
            header_names: vec!["X-A".into(), "*".into()],
            max_age: Duration::from_secs(10),
        };
        cache.insert("http://a.test", &url, false, &grant, start);
        let soon = start + Duration::from_secs(9);
        assert!(cache.allows_method("http://a.test", &same, false, "PUT", soon));
        assert!(cache.allows_method("http://a.test", &same, false, "PATCH", soon));
        assert!(cache.allows_header("http://a.test", &same, false, "x-a", soon));
        assert!(cache.allows_header("http://a.test", &same, false, "x-other", soon));
        assert!(!cache.allows_header("http://a.test", &same, false, "authorization", soon));
        assert!(!cache.allows_method("http://a.test", &same, true, "PUT", soon));
        assert!(!cache.allows_method("http://c.test", &same, false, "PUT", soon));
        let later = start + Duration::from_secs(10);
        assert!(!cache.allows_method("http://a.test", &same, false, "PUT", later));
        cache.insert("http://a.test", &url, true, &grant, start);
        assert!(!cache.allows_method("http://a.test", &same, true, "PATCH", soon));
        assert!(cache.allows_method("http://a.test", &same, true, "PUT", soon));
        assert_eq!(cache.len(), 2);
        // Full: expired keys go first, then everything.
        cache.insert("http://z.test", &url, false, &grant, later);
        assert_eq!(cache.len(), 1);
        cache.insert("http://y.test", &url, false, &grant, later);
        cache.insert("http://x.test", &url, false, &grant, later);
        assert_eq!(cache.len(), 1);
        cache.clear_for("http://x.test", &same);
        assert!(cache.is_empty());
    }
}
