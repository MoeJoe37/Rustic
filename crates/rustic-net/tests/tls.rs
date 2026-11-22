//! The TLS adapter against a TLS fixture server with test certificates
//! (`tests/fixtures/tls`, made by `tools/generate_tls_fixtures.sh`).

use std::io::Write;
use std::net::{Ipv4Addr, TcpListener};
use std::time::Duration;

use rustic_net::cookie::{CookieJar, CookieLimits};
use rustic_net::fetch::{FetchRequest, Fetcher, NetworkError};
use rustic_net::fixture::{FixtureServer, Reply};
use rustic_net::redirect::{FetchError, RedirectMode, fetch_following_redirects};
use rustic_net::site::PublicSuffixList;
use rustic_net::tls::{CertificateError, TlsConfig, TlsSetupError};
use rustic_net::{NetworkPolicy, Request, Transport, TransportError, TransportLimits};
use rustic_url::Url;

const TIMEOUT: Duration = Duration::from_secs(5);
const CA: &[u8] = include_bytes!("fixtures/tls/ca.pem");

fn fixture(name: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tls/");
    (
        std::fs::read(format!("{dir}{name}.pem")).unwrap(),
        std::fs::read(format!("{dir}{name}.key")).unwrap(),
    )
}

fn server(name: &str) -> FixtureServer {
    let (certificate, key) = fixture(name);
    FixtureServer::start_tls(
        [(
            "/",
            Reply::response(200, &[("Content-Type", "text/plain")], b"secure body"),
        )],
        &certificate,
        &key,
    )
    .unwrap()
}

fn url(input: &str) -> Url {
    Url::parse(input, None).unwrap()
}

/// `https://a.test:<port>/` routed to `server`, verified against the test CA.
fn transport_for(server: &FixtureServer, origin: &str, tls: TlsConfig) -> Transport {
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&url(origin), server.address()).unwrap();
    Transport::new(policy, TransportLimits::default())
        .unwrap()
        .with_tls(tls)
}

fn get(transport: &Transport, target: &str) -> Result<(u16, Vec<u8>), TransportError> {
    let exchange = transport.start(Request::get(url(target)).unwrap())?;
    exchange.collect().map(|(head, body)| (head.status(), body))
}

fn a_test(server: &FixtureServer) -> String {
    format!("https://a.test:{}/", server.address().port())
}

#[test]
fn a_trusted_certificate_carries_the_exchange() {
    let server = server("good");
    let roots = TlsConfig::with_roots_pem(CA).unwrap();
    let by_name = a_test(&server);
    let transport = transport_for(&server, &by_name, roots.clone());
    assert_eq!(
        get(&transport, &by_name).unwrap(),
        (200, b"secure body".to_vec())
    );
    // IP addresses are verified against IP subject alternative names.
    let by_ip = server.url("/").serialize();
    let transport = transport_for(&server, &by_ip, roots);
    assert_eq!(get(&transport, &by_ip).unwrap().0, 200);
    let log = server.requests();
    assert_eq!(log.len(), 2);
    assert_eq!(
        log[0].header("host"),
        Some(format!("a.test:{}", server.address().port()).as_str())
    );
}

#[test]
fn certificate_errors_are_typed_and_never_bypassed() {
    for (name, expected) in [
        ("expired", CertificateError::Expired),
        ("wrong-host", CertificateError::NotValidForName),
        ("untrusted", CertificateError::UnknownIssuer),
    ] {
        let server = server(name);
        let origin = a_test(&server);
        let transport = transport_for(&server, &origin, TlsConfig::with_roots_pem(CA).unwrap());
        assert_eq!(
            get(&transport, &origin).unwrap_err(),
            TransportError::Certificate(expected),
            "{name}"
        );
        assert!(server.requests().is_empty(), "{name}: no request is sent");
    }
}

#[test]
fn the_platform_verifier_does_not_trust_the_test_ca() {
    let server = server("good");
    let origin = a_test(&server);
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&url(&origin), server.address()).unwrap();
    // No `with_tls`: the platform verifier is used.
    let transport = Transport::new(policy, TransportLimits::default()).unwrap();
    let error = get(&transport, &origin).unwrap_err();
    // macOS's trust evaluation reports a test certificate's policy
    // failures (seen natively: a reason rustls maps to `Other`) along with
    // the untrusted issuer; elsewhere the issuer is the reason.
    let other_on_macos =
        cfg!(target_os = "macos") && error == TransportError::Certificate(CertificateError::Other);
    assert!(
        other_on_macos || error == TransportError::Certificate(CertificateError::UnknownIssuer),
        "{error:?}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn tls_against_a_plaintext_peer_fails_without_downgrading() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
    });
    let origin = format!("https://a.test:{}/", address.port());
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&url(&origin), address).unwrap();
    let transport = Transport::new(policy, TransportLimits::default())
        .unwrap()
        .with_tls(TlsConfig::with_roots_pem(CA).unwrap());
    assert_eq!(get(&transport, &origin).unwrap_err(), TransportError::Tls);
    peer.join().unwrap();
    // A plain fixture server closes on a ClientHello: also a TLS failure.
    let plain = FixtureServer::start([("/", Reply::response(200, &[], b"plain"))]).unwrap();
    let origin = format!("https://a.test:{}/", plain.address().port());
    let transport = transport_for(&plain, &origin, TlsConfig::with_roots_pem(CA).unwrap());
    assert_eq!(get(&transport, &origin).unwrap_err(), TransportError::Tls);
    assert!(plain.requests().is_empty());
}

#[test]
fn an_http_to_https_redirect_is_verified() {
    let secure = server("untrusted");
    let start = FixtureServer::start([(
        "/go",
        Reply::response(302, &[("Location", &a_test(&secure))], b""),
    )])
    .unwrap();
    let mut policy = NetworkPolicy::deny_all();
    policy.allow(&start.url("/"), start.address()).unwrap();
    policy
        .allow(&url(&a_test(&secure)), secure.address())
        .unwrap();
    let transport = Transport::new(policy, TransportLimits::default())
        .unwrap()
        .with_tls(TlsConfig::with_roots_pem(CA).unwrap());
    let error = fetch_following_redirects(
        &transport,
        Request::get(start.url("/go")).unwrap(),
        RedirectMode::Follow,
        TIMEOUT,
    )
    .unwrap_err();
    assert_eq!(
        error,
        FetchError::Transport(TransportError::Certificate(CertificateError::UnknownIssuer))
    );
}

#[test]
fn secure_cookies_over_https() {
    let (certificate, key) = fixture("good");
    let server = FixtureServer::start_tls(
        [
            (
                "/login",
                Reply::response(200, &[("Set-Cookie", "__Host-id=1; Secure; Path=/")], b""),
            ),
            ("/page", Reply::response(200, &[], b"")),
        ],
        &certificate,
        &key,
    )
    .unwrap();
    let origin = a_test(&server);
    let transport = transport_for(&server, &origin, TlsConfig::with_roots_pem(CA).unwrap());
    let jar = CookieJar::new(PublicSuffixList::bundled(), CookieLimits::default());
    let mut fetcher = Fetcher::new(transport, jar, TIMEOUT);
    fetcher
        .fetch(FetchRequest::navigation(url(&format!("{origin}login"))))
        .unwrap();
    fetcher
        .fetch(FetchRequest::navigation(url(&format!("{origin}page"))))
        .unwrap();
    assert_eq!(server.requests()[1].header("cookie"), Some("__Host-id=1"));
    let error = fetcher
        .fetch(FetchRequest::navigation(url(&format!(
            "https://b.test:{}/",
            server.address().port()
        ))))
        .unwrap_err();
    assert_eq!(error, NetworkError::Transport(TransportError::Blocked));
}

#[test]
fn root_sets_must_contain_certificates() {
    assert_eq!(
        TlsConfig::with_roots_pem(b"").unwrap_err(),
        TlsSetupError::NoCertificates
    );
    assert_eq!(
        TlsConfig::with_roots_pem(
            b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"
        )
        .unwrap_err(),
        TlsSetupError::InvalidRoot
    );
    assert!(format!("{:?}", TlsConfig::with_roots_pem(CA).unwrap()).contains("custom"));
}
