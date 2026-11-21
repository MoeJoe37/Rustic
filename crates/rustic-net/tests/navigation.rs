//! Navigation over the fixture server: redirects one hop at a time,
//! committing, downloads, no-content responses, sniffing, inheritance for
//! local schemes, form submissions and cancellation.

use std::time::Duration;

use rustic_encoding::Encoding;
use rustic_net::cookie::{CookieJar, CookieLimits};
use rustic_net::fetch::{Fetcher, NetworkError};
use rustic_net::fixture::{FixtureServer, Reply};
use rustic_net::navigation::{
    DocumentKind, FrameContainer, Initiator, Navigation, NavigationError, NavigationOutcome,
    NavigationRequest, NavigationState, NavigationStep, PolicyContainer,
};
use rustic_net::redirect::RedirectError;
use rustic_net::referrer::ReferrerPolicy;
use rustic_net::site::{PublicSuffixList, Site};
use rustic_net::{NetworkPolicy, Transport, TransportLimits};
use rustic_url::{Origin, Url};

const TIMEOUT: Duration = Duration::from_secs(5);

fn url(input: &str) -> Url {
    Url::parse(input, None).unwrap()
}

fn fetcher(server: &FixtureServer) -> Fetcher {
    let mut policy = NetworkPolicy::deny_all();
    policy
        .allow(&url("http://a.test"), server.address())
        .unwrap();
    policy
        .allow(&url("http://b.test"), server.address())
        .unwrap();
    let transport = Transport::new(policy, TransportLimits::default()).unwrap();
    let jar = CookieJar::new(PublicSuffixList::bundled(), CookieLimits::default());
    Fetcher::new(transport, jar, TIMEOUT)
}

fn commit(outcome: NavigationOutcome) -> Box<rustic_net::navigation::CommittedNavigation> {
    match outcome {
        NavigationOutcome::Commit(commit) => commit,
        other => panic!("expected a commit, got {other:?}"),
    }
}

fn initiator(at: &str, policy: PolicyContainer) -> Initiator {
    Initiator {
        origin: url(at).origin(),
        url: url(at),
        policy,
    }
}

#[test]
fn redirects_step_by_step_then_commit_with_response_policies() {
    let server = FixtureServer::start([
        (
            "/start",
            Reply::response(301, &[("Location", "http://b.test/next")], b""),
        ),
        (
            "/next",
            Reply::response(
                302,
                &[("Location", "/doc"), ("Referrer-Policy", "no-referrer")],
                b"",
            ),
        ),
        (
            "/doc",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/html; charset=windows-1252"),
                    ("Referrer-Policy", "same-origin"),
                    ("Content-Security-Policy", "script-src 'self'"),
                ],
                b"<p>caf\xe9",
            ),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&server);
    let mut navigation = Navigation::new(
        NavigationRequest::browser(url("http://a.test/start")),
        TIMEOUT,
    );
    assert_eq!(navigation.state(), &NavigationState::Pending);
    let NavigationStep::Redirected(first) = navigation.step(&mut fetcher).unwrap() else {
        panic!("redirect")
    };
    assert_eq!(first.serialize(), "http://b.test/next");
    assert_eq!(navigation.state(), &NavigationState::Redirected(1));
    assert!(matches!(
        navigation.step(&mut fetcher).unwrap(),
        NavigationStep::Redirected(_)
    ));
    let NavigationStep::Finished(outcome) = navigation.step(&mut fetcher).unwrap() else {
        panic!("finish")
    };
    assert_eq!(navigation.state(), &NavigationState::Done);
    let committed = commit(outcome);
    assert_eq!(committed.url().serialize(), "http://b.test/doc");
    assert_eq!(committed.url_list.len(), 3);
    assert_eq!(committed.origin, url("http://b.test").origin());
    assert_eq!(committed.kind, DocumentKind::Html);
    assert_eq!(committed.encoding, Some(Encoding::Windows1252));
    assert_eq!(committed.policy.referrer_policy, ReferrerPolicy::SameOrigin);
    assert_eq!(committed.policy.csp, ["script-src 'self'"]);
    assert_eq!(committed.body.read_all(TIMEOUT).unwrap(), b"<p>caf\xe9");
    // Redirects are fetched by the navigation, never followed silently.
    let log = server.requests();
    assert_eq!(
        log.iter().map(|r| r.target.as_str()).collect::<Vec<_>>(),
        ["/start", "/next", "/doc"]
    );
    assert_eq!(
        log[0].header("sec-fetch-mode"),
        None,
        "a.test is not trustworthy"
    );
    assert!(log[0].header("accept").unwrap().starts_with("text/html"));
}

#[test]
fn no_content_downloads_and_sniffing() {
    let server = FixtureServer::start([
        ("/empty", Reply::response(204, &[], b"")),
        (
            "/file",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/html"),
                    ("Content-Disposition", "attachment; filename=\"r.html\""),
                ],
                b"x",
            ),
        ),
        (
            "/zip",
            Reply::response(200, &[("Content-Type", "application/zip")], b"PK"),
        ),
        (
            "/untyped",
            Reply::response(200, &[], b"  <!DOCTYPE html><title>t</title>"),
        ),
        (
            "/untyped-binary",
            Reply::response(200, &[], b"\x00\x01\x02"),
        ),
        (
            "/nosniff",
            Reply::response(200, &[("X-Content-Type-Options", "nosniff")], b"<p>x"),
        ),
        (
            "/sandboxed",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/html"),
                    ("Content-Security-Policy", "sandbox allow-scripts"),
                ],
                b"",
            ),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&server);
    let mut go = |path: &str| {
        Navigation::new(
            NavigationRequest::browser(url(&format!("http://a.test{path}"))),
            TIMEOUT,
        )
        .run(&mut fetcher)
        .unwrap()
    };
    assert!(matches!(go("/empty"), NavigationOutcome::NoContent));
    let NavigationOutcome::Download(download) = go("/file") else {
        panic!("download")
    };
    assert_eq!(download.sanitized_filename(), "r.html");
    let NavigationOutcome::Download(zip) = go("/zip") else {
        panic!("download")
    };
    assert_eq!(zip.sanitized_filename(), "zip");
    assert_eq!(zip.body.read_all(TIMEOUT).unwrap(), b"PK");
    let sniffed = commit(go("/untyped"));
    assert_eq!(sniffed.kind, DocumentKind::Html);
    assert_eq!(
        sniffed.body.read_all(TIMEOUT).unwrap(),
        b"  <!DOCTYPE html><title>t</title>"
    );
    assert!(matches!(
        go("/untyped-binary"),
        NavigationOutcome::Download(_)
    ));
    assert_eq!(commit(go("/nosniff")).kind, DocumentKind::Text);
    assert!(commit(go("/sandboxed")).origin.is_opaque());
}

#[test]
fn local_schemes_inherit_or_get_opaque_origins() {
    let server = FixtureServer::start([(
        "/to-data",
        Reply::response(302, &[("Location", "data:text/html,x")], b""),
    )])
    .unwrap();
    let mut fetcher = fetcher(&server);
    let policy = PolicyContainer {
        referrer_policy: ReferrerPolicy::NoReferrer,
        csp: vec!["img-src 'none'".to_owned()],
        csp_report_only: Vec::new(),
    };
    let parent = initiator("https://parent.test/page", policy.clone());
    let blank = |initiator: Option<Initiator>| NavigationRequest {
        initiator,
        ..NavigationRequest::browser(url("about:blank"))
    };
    let inherited = commit(
        Navigation::new(blank(Some(parent.clone())), TIMEOUT)
            .run(&mut fetcher)
            .unwrap(),
    );
    assert_eq!(inherited.origin, parent.origin);
    assert_eq!(inherited.policy, policy);
    assert!(
        commit(
            Navigation::new(blank(None), TIMEOUT)
                .run(&mut fetcher)
                .unwrap()
        )
        .origin
        .is_opaque()
    );
    let srcdoc = NavigationRequest {
        initiator: Some(parent.clone()),
        srcdoc: Some("<p>inline".to_owned()),
        ..NavigationRequest::browser(url("about:srcdoc"))
    };
    let committed = commit(Navigation::new(srcdoc, TIMEOUT).run(&mut fetcher).unwrap());
    assert_eq!(committed.origin, parent.origin);
    assert_eq!(committed.body.read_all(TIMEOUT).unwrap(), b"<p>inline");
    let no_text = NavigationRequest {
        initiator: Some(parent.clone()),
        ..NavigationRequest::browser(url("about:srcdoc"))
    };
    assert_eq!(
        Navigation::new(no_text, TIMEOUT)
            .run(&mut fetcher)
            .unwrap_err(),
        NavigationError::InvalidSrcdoc
    );
    // data: — opaque origin when typed, blocked from pages and redirects.
    let data = commit(
        Navigation::new(
            NavigationRequest::browser(url("data:text/plain,hi%21")),
            TIMEOUT,
        )
        .run(&mut fetcher)
        .unwrap(),
    );
    assert!(data.origin.is_opaque());
    assert_eq!(data.kind, DocumentKind::Text);
    assert_eq!(data.body.read_all(TIMEOUT).unwrap(), b"hi!");
    let from_page = NavigationRequest {
        initiator: Some(parent),
        ..NavigationRequest::browser(url("data:text/html,x"))
    };
    assert_eq!(
        Navigation::new(from_page, TIMEOUT)
            .run(&mut fetcher)
            .unwrap_err(),
        NavigationError::DataUrlBlocked
    );
    assert_eq!(
        Navigation::new(
            NavigationRequest::browser(url("http://a.test/to-data")),
            TIMEOUT
        )
        .run(&mut fetcher)
        .unwrap_err(),
        NavigationError::DataUrlBlocked
    );
    assert_eq!(
        Navigation::new(
            NavigationRequest::browser(url("javascript:alert(1)")),
            TIMEOUT
        )
        .run(&mut fetcher)
        .unwrap_err(),
        NavigationError::UnsupportedScheme
    );
    assert_eq!(
        Navigation::new(NavigationRequest::browser(url("http://c.test/")), TIMEOUT)
            .run(&mut fetcher)
            .unwrap_err(),
        NavigationError::Network(NetworkError::Transport(rustic_net::TransportError::Blocked))
    );
}

#[test]
fn form_submissions_navigate_with_get_queries_and_post_bodies() {
    let server = FixtureServer::start([
        (
            "/search",
            Reply::response(200, &[("Content-Type", "text/html")], b"results"),
        ),
        ("/post", Reply::response(303, &[("Location", "/done")], b"")),
        (
            "/done",
            Reply::response(200, &[("Content-Type", "text/html")], b"ok"),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&server);
    let page = initiator("http://a.test/form.html", PolicyContainer::default());
    let entries = vec![
        ("q".to_owned(), "a b".to_owned()),
        ("lang".to_owned(), "é".to_owned()),
    ];
    let get = NavigationRequest::form_submission(
        page.clone(),
        "GET",
        "/search?old=1#top",
        &entries,
        Encoding::Utf8,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        get.url.serialize(),
        "http://a.test/search?q=a+b&lang=%C3%A9#top"
    );
    commit(Navigation::new(get, TIMEOUT).run(&mut fetcher).unwrap());
    let post = NavigationRequest::form_submission(
        page.clone(),
        "post",
        "/post",
        &entries,
        Encoding::Windows1252,
    )
    .unwrap()
    .unwrap();
    let done = commit(Navigation::new(post, TIMEOUT).run(&mut fetcher).unwrap());
    assert_eq!(done.url().serialize(), "http://a.test/done");
    let log = server.requests();
    assert_eq!(log[0].target, "/search?q=a+b&lang=%C3%A9");
    assert_eq!(log[0].header("referer"), Some("http://a.test/form.html"));
    assert_eq!(log[1].method, "POST");
    assert_eq!(log[1].body, b"q=a+b&lang=%E9");
    assert_eq!(
        log[1].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(log[1].header("origin"), Some("http://a.test"));
    // 303 turns the POST into a bodiless GET without Content-Type.
    assert_eq!((log[2].method.as_str(), log[2].body.len()), ("GET", 0));
    assert_eq!(log[2].header("content-type"), None);
    assert!(
        NavigationRequest::form_submission(page.clone(), "dialog", "", &entries, Encoding::Utf8)
            .unwrap()
            .is_none()
    );
    let empty_action = NavigationRequest::form_submission(page, "get", "", &[], Encoding::Utf8)
        .unwrap()
        .unwrap();
    assert_eq!(empty_action.url.serialize(), "http://a.test/form.html?");
}

#[test]
fn cancellation_and_redirect_limits() {
    let server = FixtureServer::start([
        ("/loop", Reply::response(302, &[("Location", "/loop")], b"")),
        ("/a", Reply::response(302, &[("Location", "/b")], b"")),
    ])
    .unwrap();
    let mut fetcher = fetcher(&server);
    let mut navigation =
        Navigation::new(NavigationRequest::browser(url("http://a.test/a")), TIMEOUT);
    let token = navigation.cancellation();
    assert!(matches!(
        navigation.step(&mut fetcher).unwrap(),
        NavigationStep::Redirected(_)
    ));
    token.cancel();
    assert_eq!(
        navigation.step(&mut fetcher).unwrap_err(),
        NavigationError::Cancelled
    );
    assert_eq!(navigation.state(), &NavigationState::Done);
    assert_eq!(
        server.requests().len(),
        1,
        "nothing is fetched after cancelling"
    );
    assert_eq!(
        Navigation::new(
            NavigationRequest::browser(url("http://a.test/loop")),
            TIMEOUT
        )
        .run(&mut fetcher)
        .unwrap_err(),
        NavigationError::Network(NetworkError::Redirect(RedirectError::TooManyRedirects))
    );
    assert_eq!(server.requests().len(), 1 + 21);
    let _ = Origin::opaque();
}

fn frame(src: &str, parent: &str, sandbox: Option<&[&str]>) -> NavigationRequest {
    let parent_origin = url(parent).origin();
    let same_site = parent_origin.is_same_origin(&url(src).origin());
    NavigationRequest {
        initiator: Some(initiator(parent, PolicyContainer::default())),
        user_activation: false,
        container: Some(FrameContainer {
            top_level_site: PublicSuffixList::bundled().site(&parent_origin),
            ancestors: vec![parent_origin],
            same_site_ancestors: same_site,
            sandbox: sandbox.map(|tokens| tokens.iter().map(|&t| t.to_owned()).collect()),
        }),
        ..NavigationRequest::browser(url(src))
    }
}

#[test]
fn frames_obey_frame_ancestors_x_frame_options_sandbox_and_cookie_context() {
    let html = ("Content-Type", "text/html");
    let server = FixtureServer::start([
        (
            "/set",
            Reply::response(200, &[html, ("Set-Cookie", "lax=1; SameSite=Lax")], b"set"),
        ),
        ("/plain", Reply::response(200, &[html], b"<p>frame")),
        (
            "/deny",
            Reply::response(200, &[html, ("X-Frame-Options", "DENY")], b""),
        ),
        (
            "/same",
            Reply::response(200, &[html, ("X-Frame-Options", "sameorigin")], b""),
        ),
        (
            "/conflict",
            Reply::response(
                200,
                &[html, ("X-Frame-Options", "SAMEORIGIN, ALLOWALL")],
                b"",
            ),
        ),
        (
            "/ancestors",
            Reply::response(
                200,
                &[
                    html,
                    ("X-Frame-Options", "DENY"),
                    ("Content-Security-Policy", "frame-ancestors http://a.test"),
                ],
                b"",
            ),
        ),
    ])
    .unwrap();
    let mut fetcher = fetcher(&server);
    commit(
        Navigation::new(
            NavigationRequest::browser(url("http://b.test/set")),
            TIMEOUT,
        )
        .run(&mut fetcher)
        .unwrap(),
    );
    let run = |fetcher: &mut Fetcher, request| Navigation::new(request, TIMEOUT).run(fetcher);
    // A cross-site frame is a nested navigation: iframe destination, no
    // SameSite=Lax cookies (the top level is another site).
    let committed = commit(
        run(
            &mut fetcher,
            frame("http://b.test/plain", "http://a.test/", None),
        )
        .unwrap(),
    );
    assert_eq!(committed.origin, url("http://b.test/").origin());
    let log = server.requests();
    let request = log.last().unwrap();
    assert_eq!(request.header("cookie"), None);
    // A same-site frame sends them.
    commit(
        run(
            &mut fetcher,
            frame("http://b.test/plain", "http://b.test/", None),
        )
        .unwrap(),
    );
    assert_eq!(
        server.requests().last().unwrap().header("cookie"),
        Some("lax=1")
    );
    // X-Frame-Options.
    let refused = |fetcher: &mut Fetcher, src: &str, parent: &str| {
        matches!(
            run(fetcher, frame(src, parent, None)),
            Err(NavigationError::FrameRefused)
        )
    };
    assert!(refused(
        &mut fetcher,
        "http://b.test/deny",
        "http://b.test/"
    ));
    assert!(refused(
        &mut fetcher,
        "http://b.test/same",
        "http://a.test/"
    ));
    assert!(!refused(
        &mut fetcher,
        "http://b.test/same",
        "http://b.test/"
    ));
    assert!(refused(
        &mut fetcher,
        "http://b.test/conflict",
        "http://b.test/"
    ));
    // frame-ancestors replaces X-Frame-Options.
    assert!(!refused(
        &mut fetcher,
        "http://b.test/ancestors",
        "http://a.test/"
    ));
    assert!(refused(
        &mut fetcher,
        "http://b.test/ancestors",
        "http://b.test/"
    ));
    // Top-level navigations ignore both.
    commit(
        run(
            &mut fetcher,
            NavigationRequest::browser(url("http://b.test/deny")),
        )
        .unwrap(),
    );
    // sandbox without allow-same-origin: an opaque origin; srcdoc too.
    let sandboxed = commit(
        run(
            &mut fetcher,
            frame(
                "http://b.test/plain",
                "http://a.test/",
                Some(&["allow-scripts"]),
            ),
        )
        .unwrap(),
    );
    assert!(sandboxed.origin.is_opaque());
    let kept = commit(
        run(
            &mut fetcher,
            frame(
                "http://b.test/plain",
                "http://a.test/",
                Some(&["allow-same-origin"]),
            ),
        )
        .unwrap(),
    );
    assert!(!kept.origin.is_opaque());
    let srcdoc = NavigationRequest {
        srcdoc: Some("<p>x".to_owned()),
        ..frame("about:srcdoc", "http://a.test/", Some(&[]))
    };
    assert!(
        commit(run(&mut fetcher, srcdoc).unwrap())
            .origin
            .is_opaque()
    );
    // data: frames load (opaque), unlike data: navigations of a top level
    // from a page.
    let data = commit(
        run(
            &mut fetcher,
            frame("data:text/html,<p>d", "http://a.test/", None),
        )
        .unwrap(),
    );
    assert!(data.origin.is_opaque());
    let container = FrameContainer {
        ancestors: Vec::new(),
        top_level_site: Site::parse_serialized("http://a.test").unwrap(),
        same_site_ancestors: false,
        sandbox: Some(vec!["allow-scripts".to_owned()]),
    };
    assert!(container.allows_scripts() && container.sandboxes_origin());
}
