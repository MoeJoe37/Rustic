//! A private HTTP cache (RFC 9111) for one profile, partitioned like
//! Chromium's: entries are keyed by the network partition key (the
//! top-level site) as well as the URL, so one site cannot probe what
//! another loaded (Fetch §2.8 "HTTP cache partitions").
//!
//! It stores complete `GET` responses with status 200, 203, 204, 300, 301,
//! 308, 404, 405, 410, 414 or 501 that `Content-Length` declares small
//! enough; `no-store`, `Vary: *` and requests with `Authorization` are not
//! stored. Freshness is `max-age`, else `Expires` − `Date`, else 10% of
//! `Date` − `Last-Modified` (heuristic, capped at a day); `no-cache`
//! always revalidates. Stale entries with an `ETag` or `Last-Modified` are
//! revalidated with `If-None-Match` / `If-Modified-Since`; a 304 updates
//! the stored headers (§4.3.4). A successful unsafe request invalidates
//! the URL in every partition (§4.4). Ages are in whole seconds of the
//! fetcher's clock.

use std::collections::HashMap;

use rustic_url::Url;

use crate::Headers;
use crate::response::ResponseHead;

/// Bounds of one cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheLimits {
    /// Most entries.
    pub max_entries: usize,
    /// Most body bytes of all entries together.
    pub max_total_bytes: usize,
    /// Largest body stored.
    pub max_entry_bytes: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_entries: 512,
            max_total_bytes: 32 * 1024 * 1024,
            max_entry_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Heuristic freshness never exceeds a day (RFC 9111 §4.2.2 suggests a
/// bound; Chromium and Firefox use 10% of the age since modification).
const MAX_HEURISTIC_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    partition: String,
    url: String,
}

#[derive(Clone, Debug)]
struct Entry {
    head: ResponseHead,
    body: Vec<u8>,
    /// Request header values named by `Vary`, lowercase names.
    vary: Vec<(String, Option<Vec<u8>>)>,
    /// When the response arrived (fetcher clock).
    stored_at: i64,
    last_used: u64,
}

/// What a lookup found.
#[derive(Debug)]
pub(crate) enum Lookup {
    /// Nothing usable.
    Miss,
    /// A fresh response to deliver as is.
    Fresh(ResponseHead, Vec<u8>),
    /// A stale response: send these conditional headers, and on 304 use it.
    Revalidate(Vec<(&'static str, Vec<u8>)>),
}

/// Cache-Control directives of a header section (lowercase names).
struct Directives(Vec<(String, Option<String>)>);

impl Directives {
    fn of(headers: &Headers) -> Self {
        let mut list = Vec::new();
        for value in headers.get_all("cache-control") {
            let text = String::from_utf8_lossy(value);
            for part in text.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let (name, argument) = match part.split_once('=') {
                    Some((name, argument)) => (
                        name.trim(),
                        Some(argument.trim().trim_matches('"').to_owned()),
                    ),
                    None => (part, None),
                };
                list.push((name.to_ascii_lowercase(), argument));
            }
        }
        Self(list)
    }

    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(directive, _)| directive == name)
    }

    fn seconds(&self, name: &str) -> Option<i64> {
        self.0
            .iter()
            .find(|(directive, _)| directive == name)
            .and_then(|(_, argument)| argument.as_deref()?.parse::<u64>().ok())
            .map(|seconds| i64::try_from(seconds).unwrap_or(i64::MAX))
    }
}

fn header_text(headers: &Headers, name: &str) -> Option<String> {
    headers
        .get(name)
        .map(|value| String::from_utf8_lossy(value).trim().to_owned())
}

fn date(headers: &Headers, name: &str) -> Option<i64> {
    crate::cookie::parse_cookie_date(&header_text(headers, name)?)
}

/// RFC 9111 §4.2.1: the freshness lifetime in seconds.
fn freshness_lifetime(head: &ResponseHead) -> i64 {
    let headers = head.headers();
    let directives = Directives::of(headers);
    if let Some(max_age) = directives.seconds("max-age") {
        return max_age;
    }
    let response_date = date(headers, "date");
    if headers.contains("expires") {
        // An invalid `Expires` means already expired.
        return match (date(headers, "expires"), response_date) {
            (Some(expires), Some(date)) => expires.saturating_sub(date).max(0),
            _ => 0,
        };
    }
    let heuristic = matches!(
        head.status(),
        200 | 203 | 204 | 206 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501
    );
    match (heuristic, response_date, date(headers, "last-modified")) {
        (true, Some(date), Some(modified)) if date > modified => {
            ((date - modified) / 10).min(MAX_HEURISTIC_SECONDS)
        }
        _ => 0,
    }
}

fn is_storable_status(status: u16) -> bool {
    matches!(
        status,
        200 | 203 | 204 | 300 | 301 | 308 | 404 | 405 | 410 | 414 | 501
    )
}

/// An HTTP cache for one profile.
#[derive(Debug)]
pub struct HttpCache {
    entries: HashMap<Key, Entry>,
    limits: CacheLimits,
    bytes: usize,
    tick: u64,
}

impl Default for HttpCache {
    fn default() -> Self {
        Self::new(CacheLimits::default())
    }
}

impl HttpCache {
    /// An empty cache.
    pub fn new(limits: CacheLimits) -> Self {
        Self {
            entries: HashMap::new(),
            limits,
            bytes: 0,
            tick: 0,
        }
    }

    /// Number of stored responses.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Body bytes stored.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Remove everything (`Clear-Site-Data: "cache"` for all sites).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn key(partition: &str, url: &Url) -> Key {
        Key {
            partition: partition.to_owned(),
            url: url.serialize_without_fragment(),
        }
    }

    /// Whether a response to this request may be stored, judged from the
    /// response head: `Content-Length` must fit the entry limit.
    pub(crate) fn may_store(&self, request: &Headers, head: &ResponseHead) -> Option<usize> {
        let response = Directives::of(head.headers());
        let asked = Directives::of(request);
        if !is_storable_status(head.status())
            || response.has("no-store")
            || asked.has("no-store")
            || request.contains("authorization")
            || head.headers().contains("set-cookie")
            || header_text(head.headers(), "vary").is_some_and(|vary| vary.contains('*'))
        {
            return None;
        }
        let length: usize = header_text(head.headers(), "content-length")?
            .parse()
            .ok()?;
        (length <= self.limits.max_entry_bytes).then_some(length)
    }

    /// Store a complete response received at `now`.
    pub(crate) fn store(
        &mut self,
        partition: &str,
        url: &Url,
        request: &Headers,
        head: ResponseHead,
        body: Vec<u8>,
        now: i64,
    ) {
        if body.len() > self.limits.max_entry_bytes || self.limits.max_entries == 0 {
            return;
        }
        let vary = header_text(head.headers(), "vary")
            .map(|vary| {
                vary.split(',')
                    .map(|name| name.trim().to_ascii_lowercase())
                    .filter(|name| !name.is_empty())
                    .map(|name| {
                        let value = request.get(&name).map(<[u8]>::to_vec);
                        (name, value)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let key = Self::key(partition, url);
        self.remove(&key);
        while !self.entries.is_empty()
            && (self.entries.len() >= self.limits.max_entries
                || self.bytes + body.len() > self.limits.max_total_bytes)
        {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
                .expect("non-empty");
            self.remove(&oldest);
        }
        if self.bytes + body.len() > self.limits.max_total_bytes {
            return;
        }
        self.tick += 1;
        self.bytes += body.len();
        self.entries.insert(
            key,
            Entry {
                head,
                body,
                vary,
                stored_at: now,
                last_used: self.tick,
            },
        );
    }

    fn remove(&mut self, key: &Key) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes -= entry.body.len();
        }
    }

    /// Look up a `GET` for `url` in `partition` with `request` headers.
    pub(crate) fn lookup(
        &mut self,
        partition: &str,
        url: &Url,
        request: &Headers,
        now: i64,
    ) -> Lookup {
        let key = Self::key(partition, url);
        let Some(entry) = self.entries.get_mut(&key) else {
            return Lookup::Miss;
        };
        // §4.1: every header named by `Vary` must match.
        if entry
            .vary
            .iter()
            .any(|(name, value)| request.get(name) != value.as_deref())
        {
            return Lookup::Miss;
        }
        self.tick += 1;
        entry.last_used = self.tick;
        let headers = entry.head.headers();
        let directives = Directives::of(headers);
        let asked = Directives::of(request);
        let age_header = header_text(headers, "age")
            .and_then(|age| age.parse::<i64>().ok())
            .unwrap_or(0)
            .max(0);
        let age = now
            .saturating_sub(entry.stored_at)
            .max(0)
            .saturating_add(age_header);
        let pragma = header_text(request, "pragma").is_some_and(|value| value == "no-cache");
        let fresh = age < freshness_lifetime(&entry.head)
            && !directives.has("no-cache")
            && !asked.has("no-cache")
            && !pragma
            && asked.seconds("max-age").is_none_or(|max| age <= max);
        if fresh {
            return Lookup::Fresh(entry.head.clone(), entry.body.clone());
        }
        let mut conditional = Vec::new();
        if let Some(etag) = headers.get("etag") {
            conditional.push(("If-None-Match", etag.to_vec()));
        }
        if let Some(modified) = headers.get("last-modified") {
            conditional.push(("If-Modified-Since", modified.to_vec()));
        }
        if conditional.is_empty() {
            self.remove(&key);
            return Lookup::Miss;
        }
        Lookup::Revalidate(conditional)
    }

    /// §4.3.4: freshen the stored response with a 304's headers (all but
    /// the framing fields) and return it.
    pub(crate) fn freshen(
        &mut self,
        partition: &str,
        url: &Url,
        not_modified: &ResponseHead,
        now: i64,
    ) -> Option<(ResponseHead, Vec<u8>)> {
        let key = Self::key(partition, url);
        let entry = self.entries.get_mut(&key)?;
        let mut headers = Headers::new();
        let replaced: Vec<&str> = not_modified
            .headers()
            .iter()
            .map(|(name, _)| name)
            .filter(|name| {
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "content-length" | "content-encoding" | "transfer-encoding" | "content-range"
                )
            })
            .collect();
        for (name, value) in entry.head.headers().iter() {
            if !replaced
                .iter()
                .any(|other| other.eq_ignore_ascii_case(name))
            {
                headers.append(name, value).ok()?;
            }
        }
        for (name, value) in not_modified.headers().iter() {
            if replaced
                .iter()
                .any(|other| other.eq_ignore_ascii_case(name))
            {
                headers.append(name, value).ok()?;
            }
        }
        entry.head = entry.head.with_headers(headers);
        entry.stored_at = now;
        Some((entry.head.clone(), entry.body.clone()))
    }

    /// §4.4: a non-error response to an unsafe method invalidates the URL
    /// in every partition.
    pub(crate) fn invalidate(&mut self, url: &Url) {
        let target = url.serialize_without_fragment();
        let keys: Vec<Key> = self
            .entries
            .keys()
            .filter(|key| key.url == target)
            .cloned()
            .collect();
        for key in keys {
            self.remove(&key);
        }
    }
}
