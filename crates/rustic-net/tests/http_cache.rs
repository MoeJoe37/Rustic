//! The partitioned HTTP cache over the fixture server: fresh hits,
//! partitions, revalidation with ETag, Vary, no-store, Set-Cookie and
//! invalidation by unsafe methods.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use rustic_net::cookie::{CookieJar, CookieLimits};
use rustic_net::fetch::{FetchRequest, Fetcher, RequestMode};
use rustic_net::fixture::{FixtureServer, Reply};
use rustic_net::site::PublicSuffixList;
use rustic_net::{NetworkPolicy, Transport, TransportLimits};
use rustic_url::Url;

const NOW: i64 = 1_700_000_000;

fn url(input: &str) -> Url {
    Url::parse(input, None).unwrap()
}

fn fetcher(server: &FixtureServer) -> (Fetcher, Arc<AtomicI64>) {
    let mut policy = NetworkPolicy::deny_all();
    policy
        .allow(&url("http://a.test"), server.address())
        .unwrap();
    let transport = Transport::new(policy, TransportLimits::default()).unwrap();
    let jar = CookieJar::new(
        PublicSuffixList::parse("test\n").unwrap(),
        CookieLimits::default(),
    );
    let mut fetcher = Fetcher::new(transport, jar, Duration::from_secs(5));
    let clock = Arc::new(AtomicI64::new(NOW));
    let shared = Arc::clone(&clock);
    fetcher.set_clock(move || shared.load(Ordering::SeqCst));
    (fetcher, clock)
}

fn get(fetcher: &mut Fetcher, path: &str, top: &str) -> (u16, Vec<u8>) {
    let suffixes = PublicSuffixList::parse("test\n").unwrap();
    let request = FetchRequest {
        mode: RequestMode::NoCors,
        top_level_site: Some(suffixes.site(&url(top).origin())),
        ..FetchRequest::new(
            url(&format!("http://a.test{path}")),
            url("http://a.test").origin(),
        )
    };
    let response = fetcher.fetch(request).unwrap().into_internal();
    let status = response.head.status();
    let body = rustic_net::fetch::read_all(&response.exchange, Duration::from_secs(5)).unwrap();
    (status, body)
}

fn hits(server: &FixtureServer, target: &str) -> usize {
    server
        .requests()
        .iter()
        .filter(|r| r.target == target)
        .count()
}

#[test]
fn fresh_responses_are_reused_within_their_partition_only() {
    let server = FixtureServer::start([(
        "/fresh",
        Reply::response(200, &[("Cache-Control", "max-age=60")], b"one"),
    )])
    .unwrap();
    let (mut fetcher, clock) = fetcher(&server);
    assert_eq!(get(&mut fetcher, "/fresh", "http://a.test").1, b"one");
    assert_eq!(get(&mut fetcher, "/fresh", "http://a.test").1, b"one");
    assert_eq!(hits(&server, "/fresh"), 1);
    // Another top-level site has its own partition.
    get(&mut fetcher, "/fresh", "http://other.test");
    assert_eq!(hits(&server, "/fresh"), 2);
    assert_eq!(fetcher.http_cache().len(), 2);
    // After max-age, without validators, the entry is refetched.
    clock.store(NOW + 61, Ordering::SeqCst);
    get(&mut fetcher, "/fresh", "http://a.test");
    assert_eq!(hits(&server, "/fresh"), 3);
}

#[test]
fn stale_responses_revalidate_with_etags_and_304_freshens_them() {
    let server = FixtureServer::start([
        (
            "/etag",
            Reply::response(
                200,
                &[("Cache-Control", "max-age=0"), ("ETag", "\"v1\"")],
                b"body",
            ),
        ),
        (
            "/etag if-none-match=\"v1\"",
            Reply::response(
                304,
                &[("Cache-Control", "max-age=30"), ("ETag", "\"v1\"")],
                b"",
            ),
        ),
    ])
    .unwrap();
    let (mut fetcher, _) = fetcher(&server);
    assert_eq!(
        get(&mut fetcher, "/etag", "http://a.test"),
        (200, b"body".to_vec())
    );
    // Stale: a conditional request, answered 304, delivers the stored body.
    assert_eq!(
        get(&mut fetcher, "/etag", "http://a.test"),
        (200, b"body".to_vec())
    );
    let log = server.requests();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].header("if-none-match"), None);
    assert_eq!(log[1].header("if-none-match"), Some("\"v1\""));
    // The 304's max-age=30 made the entry fresh: no third request.
    get(&mut fetcher, "/etag", "http://a.test");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn uncacheable_responses_and_unsafe_methods() {
    let server = FixtureServer::start([
        (
            "/no-store",
            Reply::response(200, &[("Cache-Control", "no-store, max-age=60")], b"x"),
        ),
        (
            "/cookie",
            Reply::response(
                200,
                &[("Cache-Control", "max-age=60"), ("Set-Cookie", "a=1")],
                b"x",
            ),
        ),
        (
            "/star",
            Reply::response(200, &[("Cache-Control", "max-age=60"), ("Vary", "*")], b"x"),
        ),
        (
            "/heuristic",
            Reply::response(
                200,
                &[
                    ("Date", "Tue, 14 Nov 2023 22:13:20 GMT"),
                    ("Last-Modified", "Sat, 04 Nov 2023 22:13:20 GMT"),
                ],
                b"h",
            ),
        ),
    ])
    .unwrap();
    let (mut fetcher, _) = fetcher(&server);
    for path in ["/no-store", "/cookie", "/star"] {
        get(&mut fetcher, path, "http://a.test");
        get(&mut fetcher, path, "http://a.test");
        assert_eq!(hits(&server, path), 2, "{path}");
    }
    // Ten days since modification: a one-day heuristic lifetime (capped).
    get(&mut fetcher, "/heuristic", "http://a.test");
    get(&mut fetcher, "/heuristic", "http://a.test");
    assert_eq!(hits(&server, "/heuristic"), 1);
    // A POST to the URL invalidates it in every partition.
    let post = FetchRequest {
        method: "POST".to_owned(),
        body: b"x".to_vec(),
        ..FetchRequest::new(
            url("http://a.test/heuristic"),
            url("http://a.test").origin(),
        )
    };
    fetcher.fetch(post).unwrap();
    get(&mut fetcher, "/heuristic", "http://a.test");
    assert_eq!(hits(&server, "/heuristic"), 3);
}

#[test]
fn vary_selects_on_request_headers() {
    let server = FixtureServer::start([(
        "/vary",
        Reply::response(
            200,
            &[("Cache-Control", "max-age=60"), ("Vary", "Accept-Language")],
            b"v",
        ),
    )])
    .unwrap();
    let (mut fetcher, _) = fetcher(&server);
    let mut request = |language: &str| {
        let mut request =
            FetchRequest::new(url("http://a.test/vary"), url("http://a.test").origin());
        request
            .headers
            .append("Accept-Language", language.as_bytes())
            .unwrap();
        let response = fetcher.fetch(request).unwrap().into_internal();
        rustic_net::fetch::read_all(&response.exchange, Duration::from_secs(5)).unwrap();
    };
    request("fr");
    request("fr");
    assert_eq!(hits(&server, "/vary"), 1);
    request("de");
    assert_eq!(hits(&server, "/vary"), 2);
    fetcher.http_cache_mut().clear();
    assert!(fetcher.http_cache().is_empty());
}

#[test]
fn the_cache_evicts_least_recently_used_entries_at_its_limits() {
    use rustic_net::http_cache::{CacheLimits, HttpCache};
    let cache = HttpCache::new(CacheLimits {
        max_entries: 1,
        ..CacheLimits::default()
    });
    assert!(cache.is_empty());
    let server = FixtureServer::start([
        (
            "/a",
            Reply::response(200, &[("Cache-Control", "max-age=60")], b"a"),
        ),
        (
            "/b",
            Reply::response(200, &[("Cache-Control", "max-age=60")], b"bb"),
        ),
    ])
    .unwrap();
    let (mut fetcher, _) = fetcher(&server);
    *fetcher.http_cache_mut() = cache;
    get(&mut fetcher, "/a", "http://a.test");
    get(&mut fetcher, "/b", "http://a.test");
    assert_eq!(fetcher.http_cache().len(), 1);
    assert_eq!(fetcher.http_cache().bytes(), 2);
    get(&mut fetcher, "/a", "http://a.test");
    assert_eq!(hits(&server, "/a"), 2);
}
