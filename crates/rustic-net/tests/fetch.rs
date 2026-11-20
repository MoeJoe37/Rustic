//! Fetch modes, CORS, preflights, redirects, cookies and filtered
//! responses over the loopback fixture server.

use std::time::Duration;

use rustic_net::cookie::{CookieJar, CookieLimits};
use rustic_net::cors::{CorsError, CredentialsMode};
use rustic_net::fetch::{
    FetchRequest, FetchResponse, Fetcher, NetworkError, ReferrerPolicy, RequestMode, ResponseType,
};
use rustic_net::fixture::{FixtureServer, Reply};
use rustic_net::redirect::RedirectMode;
use rustic_net::site::PublicSuffixList;
use rustic_net::{NetworkPolicy, Transport, TransportError, TransportLimits};
use rustic_url::{Origin, Url};

const TIMEOUT: Duration = Duration::from_secs(5);
const NOW: i64 = 1_700_000_000;

fn url(input: &str) -> Url {
    Url::parse(input, None).unwrap()
}

/// A fetcher whose policy maps each `(origin, server)` pair.
fn fetcher(routes: &[(&str, &FixtureServer)]) -> Fetcher {
    let mut policy = NetworkPolicy::deny_all();
    for (origin, server) in routes {
        policy.allow(&url(origin), server.address()).unwrap();
    }
    let transport = Transport::new(policy, TransportLimits::default()).unwrap();
    let suffixes = PublicSuffixList::parse("test\n").unwrap();
    let mut fetcher = Fetcher::new(
        transport,
        CookieJar::new(suffixes, CookieLimits::default()),
        TIMEOUT,
    );
    fetcher.set_clock(|| NOW);
    fetcher
}

fn origin(input: &str) -> Origin {
    url(input).origin()
}

fn request(target: &str, from: &str, mode: RequestMode) -> FetchRequest {
    FetchRequest {
        mode,
        ..FetchRequest::new(url(target), origin(from))
    }
}

fn header<'a>(response: &'a FetchResponse, name: &str) -> Option<&'a [u8]> {
    response.headers().get(name)
}

#[test]
fn same_origin_responses_are_basic_and_hide_set_cookie() {
    let server = FixtureServer::start([
        (
            "/set",
            Reply::response(
                200,
                &[("Set-Cookie", "id=7; Path=/"), ("X-Any", "1")],
                b"hello",
            ),
        ),
        ("/echo", Reply::response(200, &[], b"")),
    ])
    .unwrap();
    let mut fetcher = fetcher(&[("http://a.test", &server)]);
    let response = fetcher
        .fetch(request(
            "http://a.test/set",
            "http://a.test",
            RequestMode::Cors,
        ))
        .unwrap();
    assert_eq!(response.response_type(), ResponseType::Basic);
    assert_eq!(response.status(), 200);
    assert_eq!(header(&response, "x-any"), Some(&b"1"[..]));
    assert_eq!(header(&response, "set-cookie"), None);
    assert!(!response.redirected());
    assert_eq!(response.read_body(TIMEOUT).unwrap(), b"hello");
    assert_eq!(fetcher.cookies().len(), 1);
    fetcher
        .fetch(request(
            "http://a.test/echo",
            "http://a.test",
            RequestMode::Cors,
        ))
        .unwrap();
    let log = server.requests();
    assert_eq!(log[1].header("cookie"), Some("id=7"));
    assert_eq!(
        log[1].header("origin"),
        None,
        "same-origin GET sends no Origin"
    );
    assert_eq!(log[1].header("accept"), Some("*/*"));
    // credentials: omit sends no cookie.
    let mut omit = request("http://a.test/echo", "http://a.test", RequestMode::Cors);
    omit.credentials = CredentialsMode::Omit;
    fetcher.fetch(omit).unwrap();
    assert_eq!(server.requests()[2].header("cookie"), None);
}

#[test]
fn same_origin_mode_refuses_other_origins_before_sending() {
    let server = FixtureServer::start([("/", Reply::response(200, &[], b""))]).unwrap();
    let mut fetcher = fetcher(&[("http://a.test", &server), ("http://b.test", &server)]);
    assert_eq!(
        fetcher
            .fetch(request(
                "http://b.test/",
                "http://a.test",
                RequestMode::SameOrigin
            ))
            .unwrap_err(),
        NetworkError::CrossOriginSameOriginMode
    );
    assert_eq!(server.connections(), 0);
    let mut forbidden = request("http://a.test/", "http://a.test", RequestMode::Cors);
    forbidden.method = "trace".to_owned();
    assert_eq!(
        fetcher.fetch(forbidden).unwrap_err(),
        NetworkError::ForbiddenMethod
    );
    let mut manual = request("http://a.test/", "http://a.test", RequestMode::NoCors);
    manual.redirect = RedirectMode::Manual;
    assert_eq!(
        fetcher.fetch(manual).unwrap_err(),
        NetworkError::NoCorsRedirectMode
    );
    assert_eq!(
        fetcher
            .fetch(request(
                "http://c.test/",
                "http://a.test",
                RequestMode::Cors
            ))
            .unwrap_err(),
        NetworkError::Transport(TransportError::Blocked)
    );
    assert_eq!(server.connections(), 0);
}

#[test]
fn no_cors_cross_origin_responses_are_opaque() {
    let server = FixtureServer::start([(
        "/img",
        Reply::response(
            200,
            &[("Content-Type", "image/png"), ("Set-Cookie", "t=1")],
            b"PNGDATA",
        ),
    )])
    .unwrap();
    let mut fetcher = fetcher(&[("http://b.test", &server)]);
    let response = fetcher
        .fetch(request(
            "http://b.test/img",
            "http://a.test",
            RequestMode::NoCors,
        ))
        .unwrap();
    assert_eq!(response.response_type(), ResponseType::Opaque);
    assert_eq!(response.status(), 0);
    assert_eq!(response.status_message(), b"");
    assert!(response.headers().is_empty());
    assert!(response.url().is_none());
    let internal = response.into_internal();
    assert_eq!(internal.head.status(), 200);
    assert_eq!(
        rustic_net::fetch::read_all(&internal.exchange, TIMEOUT).unwrap(),
        b"PNGDATA"
    );
    // Same-origin credentials mode: no cookie stored from an opaque response.
    assert!(fetcher.cookies().is_empty());
    assert_eq!(server.requests()[0].header("origin"), None);
    let opaque = fetcher
        .fetch(request(
            "http://b.test/img",
            "http://a.test",
            RequestMode::NoCors,
        ))
        .unwrap();
    assert!(opaque.read_body(TIMEOUT).unwrap().is_empty());
}

#[test]
fn cors_responses_need_allow_origin_and_filter_headers() {
    let server = FixtureServer::start([
        ("/none", Reply::response(200, &[], b"secret")),
        (
            "/star",
            Reply::response(
                200,
                &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Access-Control-Expose-Headers", "X-Exposed"),
                    ("Content-Type", "text/plain"),
                    ("X-Exposed", "yes"),
                    ("X-Hidden", "no"),
                ],
                b"shared",
            ),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&[("http://b.test", &server)]);
    assert_eq!(
        fetcher
            .fetch(request(
                "http://b.test/none",
                "http://a.test",
                RequestMode::Cors
            ))
            .unwrap_err(),
        NetworkError::Cors(CorsError::MissingAllowOrigin)
    );
    let response = fetcher
        .fetch(request(
            "http://b.test/star",
            "http://a.test",
            RequestMode::Cors,
        ))
        .unwrap();
    assert_eq!(response.response_type(), ResponseType::Cors);
    assert_eq!(header(&response, "content-type"), Some(&b"text/plain"[..]));
    assert_eq!(header(&response, "x-exposed"), Some(&b"yes"[..]));
    assert_eq!(header(&response, "x-hidden"), None);
    assert_eq!(header(&response, "access-control-allow-origin"), None);
    assert_eq!(response.read_body(TIMEOUT).unwrap(), b"shared");
    for recorded in server.requests() {
        assert_eq!(recorded.header("origin"), Some("http://a.test"));
    }
    // A credentialed request cannot use the wildcard.
    let mut credentialed = request("http://b.test/star", "http://a.test", RequestMode::Cors);
    credentialed.credentials = CredentialsMode::Include;
    assert_eq!(
        fetcher.fetch(credentialed).unwrap_err(),
        NetworkError::Cors(CorsError::AllowOriginMismatch)
    );
}

#[test]
fn credentialed_cors_sends_and_stores_cookies_when_allowed() {
    let other = FixtureServer::start([(
        "/api",
        Reply::response(
            200,
            &[
                ("Access-Control-Allow-Origin", "http://127.0.0.1"),
                ("Access-Control-Allow-Credentials", "true"),
                ("Set-Cookie", "api=1"),
            ],
            b"",
        ),
    )])
    .unwrap();
    // 127.0.0.1 on two ports: cross-origin but same-site.
    let target = other.url("/api").serialize();
    let mut fetcher = fetcher(&[(target.as_str(), &other)]);
    let mut first = request(&target, "http://127.0.0.1", RequestMode::Cors);
    first.credentials = CredentialsMode::Include;
    fetcher.fetch(first.clone()).unwrap();
    assert_eq!(fetcher.cookies().len(), 1);
    fetcher.fetch(first).unwrap();
    assert_eq!(other.requests()[1].header("cookie"), Some("api=1"));
    // Without `include`, a cross-origin CORS request carries no cookies.
    let mut same_origin_credentials = request(&target, "http://127.0.0.1", RequestMode::Cors);
    same_origin_credentials.credentials = CredentialsMode::SameOrigin;
    fetcher.fetch(same_origin_credentials).unwrap();
    assert_eq!(other.requests()[2].header("cookie"), None);
}

#[test]
fn preflights_run_validate_and_are_cached() {
    let server = FixtureServer::start([
        (
            "/ok",
            Reply::response(
                204,
                &[
                    ("Access-Control-Allow-Origin", "http://a.test"),
                    ("Access-Control-Allow-Methods", "PUT"),
                    ("Access-Control-Allow-Headers", "x-custom"),
                    ("Access-Control-Max-Age", "600"),
                ],
                b"",
            ),
        ),
        (
            "/denied",
            Reply::response(
                204,
                &[("Access-Control-Allow-Origin", "http://a.test")],
                b"",
            ),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&[("http://b.test", &server)]);
    let put = |path: &str| {
        let mut put = request(
            &format!("http://b.test{path}"),
            "http://a.test",
            RequestMode::Cors,
        );
        put.method = "put".to_owned();
        put.headers.append("X-Custom", b"1").unwrap();
        put.body = b"data".to_vec();
        put
    };
    let response = fetcher.fetch(put("/ok")).unwrap();
    assert_eq!(response.status(), 204);
    let log = server.requests();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].method, "OPTIONS");
    assert_eq!(log[0].header("access-control-request-method"), Some("PUT"));
    assert_eq!(
        log[0].header("access-control-request-headers"),
        Some("x-custom")
    );
    assert_eq!(log[0].header("origin"), Some("http://a.test"));
    assert_eq!(log[0].header("x-custom"), None);
    assert!(log[0].body.is_empty());
    assert_eq!(
        (log[1].method.as_str(), log[1].body.as_slice()),
        ("PUT", &b"data"[..])
    );
    assert_eq!(fetcher.preflight_cache().len(), 1);
    fetcher.fetch(put("/ok")).unwrap();
    assert_eq!(
        server.requests().len(),
        3,
        "the cached grant skips the preflight"
    );
    assert_eq!(
        fetcher.fetch(put("/denied")).unwrap_err(),
        NetworkError::Preflight(CorsError::MethodNotAllowed)
    );
    let log = server.requests();
    assert_eq!(log.len(), 4, "the actual request is never sent");
    assert_eq!(log[3].method, "OPTIONS");
    // A safelisted request needs no preflight.
    let mut post = request("http://b.test/denied", "http://a.test", RequestMode::Cors);
    post.method = "POST".to_owned();
    post.headers.append("Content-Type", b"text/plain").unwrap();
    fetcher.fetch(post).unwrap();
    assert_eq!(server.requests()[4].method, "POST");
}

#[test]
fn cross_origin_redirects_taint_the_origin() {
    let a = FixtureServer::start([(
        "/start",
        Reply::response(302, &[("Location", "http://b.test/hop")], b""),
    )])
    .unwrap();
    let b = FixtureServer::start([
        (
            "/hop",
            Reply::response(
                302,
                &[
                    ("Location", "http://c.test/end"),
                    ("Access-Control-Allow-Origin", "http://a.test"),
                ],
                b"",
            ),
        ),
        (
            "/creds",
            Reply::response(
                302,
                &[
                    ("Location", "http://user:pw@c.test/end"),
                    ("Access-Control-Allow-Origin", "*"),
                ],
                b"",
            ),
        ),
    ])
    .unwrap();
    let c = FixtureServer::start([(
        "/end",
        Reply::response(200, &[("Access-Control-Allow-Origin", "null")], b"done"),
    )])
    .unwrap();
    let mut fetcher = fetcher(&[
        ("http://a.test", &a),
        ("http://b.test", &b),
        ("http://c.test", &c),
    ]);
    let response = fetcher
        .fetch(request(
            "http://a.test/start",
            "http://a.test",
            RequestMode::Cors,
        ))
        .unwrap();
    assert_eq!(response.response_type(), ResponseType::Cors);
    assert!(response.redirected());
    assert_eq!(response.url().unwrap().serialize(), "http://c.test/end");
    assert_eq!(a.requests()[0].header("origin"), None);
    assert_eq!(b.requests()[0].header("origin"), Some("http://a.test"));
    assert_eq!(c.requests()[0].header("origin"), Some("null"));
    assert_eq!(
        fetcher
            .fetch(request(
                "http://b.test/creds",
                "http://a.test",
                RequestMode::Cors
            ))
            .unwrap_err(),
        NetworkError::RedirectWithCredentials
    );
    // A same-origin-mode request may not be redirected elsewhere.
    assert_eq!(
        fetcher
            .fetch(request(
                "http://a.test/start",
                "http://a.test",
                RequestMode::SameOrigin
            ))
            .unwrap_err(),
        NetworkError::CrossOriginSameOriginMode
    );
    // no-cors through the same chain ends opaque.
    let opaque = fetcher
        .fetch(request(
            "http://a.test/start",
            "http://a.test",
            RequestMode::NoCors,
        ))
        .unwrap();
    assert_eq!(opaque.response_type(), ResponseType::Opaque);
    assert!(!opaque.redirected());
}

#[test]
fn manual_redirects_are_opaque_redirects() {
    let server =
        FixtureServer::start([("/r", Reply::response(301, &[("Location", "/next")], b""))])
            .unwrap();
    let mut fetcher = fetcher(&[("http://a.test", &server)]);
    let mut manual = request("http://a.test/r", "http://a.test", RequestMode::Cors);
    manual.redirect = RedirectMode::Manual;
    let response = fetcher.fetch(manual).unwrap();
    assert_eq!(response.response_type(), ResponseType::OpaqueRedirect);
    assert_eq!(response.status(), 0);
    assert!(response.headers().is_empty());
    let internal = response.into_internal();
    assert_eq!(internal.head.headers().get("location"), Some(&b"/next"[..]));
    let mut error = request("http://a.test/r", "http://a.test", RequestMode::Cors);
    error.redirect = RedirectMode::Error;
    assert!(matches!(
        fetcher.fetch(error),
        Err(NetworkError::Redirect(_))
    ));
}

#[test]
fn resource_policy_blocks_no_cors_responses() {
    let server = FixtureServer::start([
        (
            "/same-origin",
            Reply::response(200, &[("Cross-Origin-Resource-Policy", "same-origin")], b""),
        ),
        (
            "/same-site",
            Reply::response(200, &[("Cross-Origin-Resource-Policy", "same-site")], b""),
        ),
        (
            "/cross",
            Reply::response(
                200,
                &[("Cross-Origin-Resource-Policy", "cross-origin")],
                b"",
            ),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&[("http://img.a.test", &server), ("http://b.test", &server)]);
    let fetch = |fetcher: &mut Fetcher, target: &str| {
        fetcher
            .fetch(request(target, "http://www.a.test", RequestMode::NoCors))
            .map(|response| response.response_type())
    };
    assert_eq!(
        fetch(&mut fetcher, "http://img.a.test/same-origin"),
        Err(NetworkError::BlockedByResourcePolicy)
    );
    assert_eq!(
        fetch(&mut fetcher, "http://img.a.test/same-site"),
        Ok(ResponseType::Opaque)
    );
    assert_eq!(
        fetch(&mut fetcher, "http://b.test/same-site"),
        Err(NetworkError::BlockedByResourcePolicy)
    );
    assert_eq!(
        fetch(&mut fetcher, "http://b.test/cross"),
        Ok(ResponseType::Opaque)
    );
    // CORP does not apply to CORS requests.
    let cors = fetcher.fetch(request(
        "http://img.a.test/same-origin",
        "http://www.a.test",
        RequestMode::Cors,
    ));
    assert_eq!(
        cors.unwrap_err(),
        NetworkError::Cors(CorsError::MissingAllowOrigin)
    );
}

#[test]
fn unsafe_methods_send_origin_by_referrer_policy() {
    let server = FixtureServer::start([("/form", Reply::response(200, &[], b""))]).unwrap();
    let mut fetcher = fetcher(&[("http://a.test", &server), ("http://b.test", &server)]);
    let post = |target: &str, policy: ReferrerPolicy| FetchRequest {
        method: "POST".to_owned(),
        referrer_policy: policy,
        ..request(target, "http://a.test", RequestMode::NoCors)
    };
    for (target, policy) in [
        (
            "http://a.test/form",
            ReferrerPolicy::StrictOriginWhenCrossOrigin,
        ),
        ("http://a.test/form", ReferrerPolicy::NoReferrer),
        ("http://b.test/form", ReferrerPolicy::SameOrigin),
        ("http://b.test/form", ReferrerPolicy::Origin),
    ] {
        fetcher.fetch(post(target, policy)).unwrap();
    }
    let origins: Vec<Option<String>> = server
        .requests()
        .iter()
        .map(|recorded| recorded.header("origin").map(str::to_owned))
        .collect();
    assert_eq!(
        origins,
        [
            Some("http://a.test".to_owned()),
            Some("null".to_owned()),
            Some("null".to_owned()),
            Some("http://a.test".to_owned()),
        ]
    );
}

#[test]
fn navigations_store_lax_cookies_and_subresources_respect_same_site() {
    let site = FixtureServer::start([
        (
            "/login",
            Reply::response(
                200,
                &[
                    ("Set-Cookie", "session=1"),
                    ("Set-Cookie", "strict=1; SameSite=Strict"),
                ],
                b"",
            ),
        ),
        ("/page", Reply::response(200, &[], b"")),
    ])
    .unwrap();
    let mut fetcher = fetcher(&[("http://a.test", &site)]);
    let navigation = fetcher
        .fetch(FetchRequest::navigation(url("http://a.test/login")))
        .unwrap();
    assert_eq!(navigation.response_type(), ResponseType::Basic);
    assert_eq!(fetcher.cookies().len(), 2);
    assert!(
        site.requests()[0]
            .header("accept")
            .unwrap()
            .starts_with("text/html")
    );
    // A cross-site page embedding a.test: no unpartitioned cookies.
    let mut embedded = request(
        "http://a.test/page",
        "http://evil.test",
        RequestMode::NoCors,
    );
    embedded.credentials = CredentialsMode::Include;
    fetcher.fetch(embedded).unwrap();
    assert_eq!(site.requests()[1].header("cookie"), None);
    // A cross-site link click: Lax and default cookies, not Strict.
    let mut link = FetchRequest::navigation(url("http://a.test/page"));
    link.site_for_cookies = Some(origin("http://evil.test"));
    fetcher.fetch(link).unwrap();
    assert_eq!(site.requests()[2].header("cookie"), Some("session=1"));
    // A same-site navigation gets both.
    fetcher
        .fetch(FetchRequest::navigation(url("http://a.test/page")))
        .unwrap();
    assert_eq!(
        site.requests()[3].header("cookie"),
        Some("session=1; strict=1")
    );
}

#[test]
fn same_origin_credentials_exclude_cross_origin_same_site_requests() {
    let server = FixtureServer::start([
        (
            "/login",
            Reply::response(200, &[("Set-Cookie", "sid=1; Domain=a.test")], b""),
        ),
        (
            "/img",
            Reply::response(200, &[("Set-Cookie", "late=1; Domain=a.test")], b""),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&[
        ("http://img.a.test", &server),
        ("http://www.a.test", &server),
    ]);
    fetcher
        .fetch(FetchRequest::navigation(url("http://www.a.test/login")))
        .unwrap();
    assert_eq!(fetcher.cookies().len(), 1);
    // Cross-origin, same-site, no-cors: opaque tainting, so `same-origin`
    // credentials neither send nor store cookies.
    fetcher
        .fetch(request(
            "http://img.a.test/img",
            "http://www.a.test",
            RequestMode::NoCors,
        ))
        .unwrap();
    assert_eq!(server.requests()[1].header("cookie"), None);
    assert_eq!(fetcher.cookies().len(), 1);
    let mut include = request(
        "http://img.a.test/img",
        "http://www.a.test",
        RequestMode::NoCors,
    );
    include.credentials = CredentialsMode::Include;
    fetcher.fetch(include).unwrap();
    assert_eq!(server.requests()[2].header("cookie"), Some("sid=1"));
    assert_eq!(fetcher.cookies().len(), 2);
}

#[test]
fn referrer_and_fetch_metadata_follow_the_policies() {
    let server = FixtureServer::start([
        (
            "/hop",
            Reply::response(
                302,
                &[("Location", "/end"), ("Referrer-Policy", "origin")],
                b"",
            ),
        ),
        ("/end", Reply::response(200, &[], b"")),
    ])
    .unwrap();
    // 127.0.0.1 is potentially trustworthy, so fetch metadata is sent.
    let base = server.url("/").serialize();
    let mut fetcher = fetcher(&[(base.as_str(), &server)]);
    fetcher.set_accept_language("de");
    let referrer = url(&format!("{base}page?secret=1#frag"));
    let request = |path: &str| FetchRequest {
        referrer: Some(referrer.clone()),
        destination: rustic_net::fetch::Destination::Image,
        ..FetchRequest::new(server.url(path), referrer.origin())
    };
    fetcher.fetch(request("/end")).unwrap();
    fetcher.fetch(request("/hop")).unwrap();
    let log = server.requests();
    assert_eq!(
        log[0].header("referer"),
        Some(format!("{base}page?secret=1").as_str())
    );
    assert_eq!(log[0].header("sec-fetch-dest"), Some("image"));
    assert_eq!(log[0].header("sec-fetch-mode"), Some("no-cors"));
    assert_eq!(log[0].header("sec-fetch-site"), Some("same-origin"));
    assert_eq!(log[0].header("sec-fetch-user"), None);
    assert_eq!(log[0].header("accept-language"), Some("de"));
    // The redirect's Referrer-Policy applies to the next hop.
    assert_eq!(
        log[1].header("referer"),
        Some(format!("{base}page?secret=1").as_str())
    );
    assert_eq!(log[2].header("referer"), Some(base.as_str()));
    let mut typed = FetchRequest::navigation(server.url("/end"));
    typed.referrer = None;
    fetcher.fetch(typed).unwrap();
    let log = server.requests();
    assert_eq!(log[3].header("referer"), None);
    assert_eq!(log[3].header("sec-fetch-site"), Some("none"));
    assert_eq!(log[3].header("sec-fetch-user"), Some("?1"));
    assert_eq!(log[3].header("sec-fetch-dest"), Some("document"));
    // a.test is not potentially trustworthy: no metadata.
    let other = FixtureServer::start([("/", Reply::response(200, &[], b""))]).unwrap();
    let mut plain = self::fetcher(&[("http://a.test", &other)]);
    plain.fetch(request_to("http://a.test/")).unwrap();
    assert_eq!(other.requests()[0].header("sec-fetch-mode"), None);
}

fn request_to(target: &str) -> FetchRequest {
    request(target, "http://a.test", RequestMode::NoCors)
}
