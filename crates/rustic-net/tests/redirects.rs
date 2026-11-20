//! Fetch redirect rules over the loopback fixture server.

use std::time::Duration;

use rustic_net::fixture::{FixtureServer, Reply};
use rustic_net::redirect::{
    FetchError, Fetched, MAX_REDIRECTS, RedirectError, RedirectMode, fetch_following_redirects,
};
use rustic_net::{NetEvent, NetworkPolicy, Request, Transport, TransportError, TransportLimits};
use rustic_url::Url;

const HEAD_TIMEOUT: Duration = Duration::from_secs(5);

fn redirect(status: u16, location: &str) -> Reply {
    Reply::response(status, &[("Location", location)], b"redirect body")
}

fn transport(servers: &[&FixtureServer]) -> Transport {
    let mut policy = NetworkPolicy::deny_all();
    for server in servers {
        policy.allow(&server.url("/"), server.address()).unwrap();
    }
    Transport::new(policy, TransportLimits::default()).unwrap()
}

fn fetch(
    transport: &Transport,
    request: Request,
    mode: RedirectMode,
) -> Result<Fetched, FetchError> {
    fetch_following_redirects(transport, request, mode, HEAD_TIMEOUT)
}

fn body(fetched: Fetched) -> Vec<u8> {
    let mut body = Vec::new();
    while let Some(event) = fetched.exchange.next_event(Some(HEAD_TIMEOUT)) {
        match event {
            NetEvent::Body(bytes) => body.extend_from_slice(&bytes),
            NetEvent::Finished => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    body
}

#[test]
fn follows_a_chain_and_records_the_url_list() {
    let server = FixtureServer::start([
        ("/a", redirect(301, "/b?x=1")),
        ("/b", redirect(302, "c")),
        ("/c", Reply::response(200, &[], b"final")),
    ])
    .unwrap();
    let transport = transport(&[&server]);
    let fetched = fetch(
        &transport,
        Request::get(server.url("/a#top")).unwrap(),
        RedirectMode::Follow,
    )
    .unwrap();
    let urls: Vec<String> = fetched.url_list.iter().map(Url::serialize).collect();
    let base = format!("http://{}", server.address());
    assert_eq!(
        urls,
        [
            format!("{base}/a#top"),
            format!("{base}/b?x=1#top"),
            format!("{base}/c#top")
        ]
    );
    assert_eq!(fetched.head.status(), 200);
    assert_eq!(body(fetched), b"final");
    let targets: Vec<String> = server
        .requests()
        .into_iter()
        .map(|request| request.target)
        .collect();
    assert_eq!(targets, ["/a", "/b?x=1", "/c"]);
}

#[test]
fn a_location_fragment_replaces_the_request_fragment() {
    let server = FixtureServer::start([
        ("/a", redirect(307, "/b#own")),
        ("/b", Reply::response(200, &[], b"")),
    ])
    .unwrap();
    let transport = transport(&[&server]);
    let fetched = fetch(
        &transport,
        Request::get(server.url("/a#top")).unwrap(),
        RedirectMode::Follow,
    )
    .unwrap();
    assert_eq!(fetched.url_list[1].fragment(), Some("own"));
}

#[test]
fn methods_and_bodies_follow_the_status_rules() {
    let server = FixtureServer::start([
        ("/301", redirect(301, "/done")),
        ("/302", redirect(302, "/done")),
        ("/303", redirect(303, "/done")),
        ("/307", redirect(307, "/done")),
        ("/308", redirect(308, "/done")),
        ("/done", Reply::response(200, &[], b"ok")),
    ])
    .unwrap();
    let transport = transport(&[&server]);
    let cases = [
        ("POST", "/301", "GET", false),
        ("POST", "/302", "GET", false),
        ("POST", "/303", "GET", false),
        ("PUT", "/303", "GET", false),
        ("HEAD", "/303", "HEAD", true),
        ("PUT", "/301", "PUT", true),
        ("POST", "/307", "POST", true),
        ("POST", "/308", "POST", true),
    ];
    for (method, path, expected_method, keeps_body) in cases {
        let mut request = Request::new(method, server.url(path)).unwrap();
        request.header("Content-Type", b"text/plain").unwrap();
        request.header("X-Other", b"kept").unwrap();
        request.set_body(b"payload".to_vec());
        fetch(&transport, request, RedirectMode::Follow).unwrap();
        let requests = server.requests();
        let hop = requests.last().unwrap();
        let case = format!("{method} {path}");
        assert_eq!(hop.target, "/done", "{case}");
        assert_eq!(hop.method, expected_method, "{case}");
        assert_eq!(hop.header("x-other"), Some("kept"), "{case}");
        if keeps_body {
            assert_eq!(
                (hop.body.as_slice(), hop.header("content-type")),
                (&b"payload"[..], Some("text/plain")),
                "{case}"
            );
        } else {
            assert_eq!(
                (hop.body.len(), hop.header("content-type")),
                (0, None),
                "{case}"
            );
        }
    }
}

#[test]
fn every_hop_is_checked_against_the_policy() {
    let allowed = FixtureServer::start([("/", Reply::response(200, &[], b"ok"))]).unwrap();
    let other = FixtureServer::start([("/", Reply::response(200, &[], b"secret"))]).unwrap();
    let start = FixtureServer::start([
        ("/to-other", redirect(302, &other.url("/").serialize())),
        ("/to-allowed", redirect(302, &allowed.url("/").serialize())),
        (
            "/to-https",
            redirect(
                302,
                &format!("https://127.0.0.1:{}/", allowed.address().port()),
            ),
        ),
    ])
    .unwrap();
    let transport = transport(&[&start, &allowed]);
    let error = fetch(
        &transport,
        Request::get(start.url("/to-other")).unwrap(),
        RedirectMode::Follow,
    )
    .unwrap_err();
    assert_eq!(error, FetchError::Transport(TransportError::Blocked));
    assert_eq!(other.connections(), 0);
    let fetched = fetch(
        &transport,
        Request::get(start.url("/to-allowed")).unwrap(),
        RedirectMode::Follow,
    )
    .unwrap();
    assert_eq!(body(fetched), b"ok");

    // An allowed https origin fails in TLS rather than downgrading.
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&start.url("/"), start.address()).unwrap();
    let https = Url::parse(
        &format!("https://127.0.0.1:{}/", allowed.address().port()),
        None,
    )
    .unwrap();
    policy.allow(&https, allowed.address()).unwrap();
    let transport = Transport::new(policy, TransportLimits::default()).unwrap();
    let error = fetch(
        &transport,
        Request::get(start.url("/to-https")).unwrap(),
        RedirectMode::Follow,
    )
    .unwrap_err();
    // The plain server cannot complete a TLS handshake.
    assert_eq!(error, FetchError::Transport(TransportError::Tls));
    assert_eq!(allowed.requests().len(), 1, "never retried over http");
}

#[test]
fn cross_origin_redirects_drop_authorization() {
    let second = FixtureServer::start([("/", Reply::response(200, &[], b""))]).unwrap();
    let first = FixtureServer::start([
        ("/same", redirect(302, "/landing")),
        ("/landing", Reply::response(200, &[], b"")),
        ("/cross", redirect(302, &second.url("/").serialize())),
    ])
    .unwrap();
    let transport = transport(&[&first, &second]);
    for path in ["/same", "/cross"] {
        let mut request = Request::get(first.url(path)).unwrap();
        request.header("Authorization", b"Basic dTpw").unwrap();
        fetch(&transport, request, RedirectMode::Follow).unwrap();
    }
    let same = first.requests();
    assert_eq!(same[1].target, "/landing");
    assert_eq!(same[1].header("authorization"), Some("Basic dTpw"));
    assert_eq!(second.requests()[0].header("authorization"), None);
}

#[test]
fn the_twenty_first_redirect_is_an_error() {
    let routes: Vec<(String, Reply)> = (0..MAX_REDIRECTS)
        .map(|hop| (format!("/{hop}"), redirect(302, &format!("/{}", hop + 1))))
        .collect();
    let mut routes: Vec<(&str, Reply)> = routes
        .iter()
        .map(|(path, reply)| (path.as_str(), reply.clone()))
        .collect();
    routes.push(("/20", Reply::response(200, &[], b"twenty")));
    routes.push(("/loop", redirect(302, "/loop")));
    let server = FixtureServer::start(routes).unwrap();
    let transport = transport(&[&server]);
    let fetched = fetch(
        &transport,
        Request::get(server.url("/0")).unwrap(),
        RedirectMode::Follow,
    )
    .unwrap();
    assert_eq!(fetched.url_list.len(), MAX_REDIRECTS + 1);
    assert_eq!(body(fetched), b"twenty");
    let before = server.requests().len();
    let error = fetch(
        &transport,
        Request::get(server.url("/loop")).unwrap(),
        RedirectMode::Follow,
    )
    .unwrap_err();
    assert_eq!(error, FetchError::Redirect(RedirectError::TooManyRedirects));
    // The first request and twenty redirects were sent, no more.
    assert_eq!(server.requests().len() - before, MAX_REDIRECTS + 1);
}

#[test]
fn invalid_locations_modes_and_non_redirects() {
    let server = FixtureServer::start([
        ("/bad", redirect(302, "http://[::1")),
        ("/data", redirect(302, "data:text/plain,x")),
        (
            "/two",
            Reply::response(302, &[("Location", "/a"), ("Location", "/b")], b""),
        ),
        (
            "/same-twice",
            Reply::response(302, &[("Location", "/ok"), ("Location", "/ok")], b""),
        ),
        ("/no-location", Reply::response(302, &[], b"kept")),
        ("/300", redirect(300, "/ok")),
        ("/moved", redirect(302, "/ok")),
        ("/ok", Reply::response(200, &[], b"ok")),
    ])
    .unwrap();
    let transport = transport(&[&server]);
    let run = |path: &str, mode| fetch(&transport, Request::get(server.url(path)).unwrap(), mode);
    let redirect_error = |path: &str| run(path, RedirectMode::Follow).unwrap_err();
    assert_eq!(
        redirect_error("/bad"),
        FetchError::Redirect(RedirectError::InvalidLocation)
    );
    assert_eq!(
        redirect_error("/data"),
        FetchError::Redirect(RedirectError::UnsupportedScheme)
    );
    assert_eq!(
        redirect_error("/two"),
        FetchError::Redirect(RedirectError::InvalidLocation)
    );
    assert_eq!(
        run("/same-twice", RedirectMode::Follow)
            .unwrap()
            .head
            .status(),
        200
    );
    let kept = run("/no-location", RedirectMode::Follow).unwrap();
    assert_eq!((kept.head.status(), kept.url_list.len()), (302, 1));
    assert_eq!(body(kept), b"kept");
    assert_eq!(
        run("/300", RedirectMode::Follow).unwrap().head.status(),
        300
    );
    assert_eq!(
        run("/moved", RedirectMode::Error).unwrap_err(),
        FetchError::Redirect(RedirectError::RedirectForbidden)
    );
    let manual = run("/moved", RedirectMode::Manual).unwrap();
    assert_eq!((manual.head.status(), manual.url_list.len()), (302, 1));
}
