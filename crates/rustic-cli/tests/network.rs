//! The CLI loading a document, its stylesheets, scripts and images over
//! HTTP from a loopback fixture server (the M6 demo shape): response
//! charset, redirects, cookies, CSP and a blocked cross-origin resource.

use std::process::{Command, Output};

use rustic_net::fixture::{FixtureServer, Reply};

fn rustic(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args(args)
        .output()
        .expect("run rustic")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const PAGE: &[u8] = b"<!doctype html><html><head>\
<link rel=stylesheet href=/moved.css>\
<link rel=stylesheet href=/plain.txt>\
<script src=/app.js></script>\
<script src=http://b.test/evil.js></script>\
<script>document.body.setAttribute('data-inline', 'blocked');</script>\
<script nonce=n1>document.documentElement.setAttribute('data-nonce', 'ran');</script>\
</head><body><p id=t>caf\xe9</p><img src=/missing.png></body></html>";

fn server() -> FixtureServer {
    FixtureServer::start([
        (
            "/",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/html; charset=windows-1252"),
                    (
                        "Content-Security-Policy",
                        "script-src 'self' 'nonce-n1'; style-src 'self'",
                    ),
                    ("Set-Cookie", "session=1; Path=/"),
                ],
                PAGE,
            ),
        ),
        (
            "/moved.css",
            Reply::response(302, &[("Location", "/style.css")], b""),
        ),
        (
            "/style.css",
            Reply::response(
                200,
                &[("Content-Type", "text/css")],
                b"#t { color: rgb(1, 2, 3) }",
            ),
        ),
        (
            "/plain.txt",
            Reply::response(
                200,
                &[("Content-Type", "text/plain")],
                b"#t { margin-left: 7px }",
            ),
        ),
        (
            "/app.js",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/javascript; charset=utf-8"),
                    ("X-Content-Type-Options", "nosniff"),
                ],
                "document.documentElement.setAttribute('data-app', 'r\u{e1}n');".as_bytes(),
            ),
        ),
        (
            "/evil.js",
            Reply::response(
                200,
                &[("Content-Type", "text/javascript")],
                b"document.documentElement.setAttribute('data-evil', 'ran');",
            ),
        ),
    ])
    .unwrap()
}

fn grants(server: &FixtureServer) -> [String; 2] {
    let address = server.address();
    [
        format!("http://a.test={address}"),
        format!("http://b.test={address}"),
    ]
}

#[test]
fn documents_load_over_http_with_policies_applied() {
    let server = server();
    let [a, b] = grants(&server);
    let args = [
        "dump-dom",
        "--run-scripts",
        "--allow-origin",
        &a,
        "--allow-origin",
        &b,
        "http://a.test/",
    ];
    let output = rustic(&args);
    assert!(output.status.success(), "{}", stderr(&output));
    let dom = stdout(&output);
    // The response charset decodes byte 0xE9.
    assert!(dom.contains("café"), "{dom}");
    // Same-origin script ran; the cross-origin one and the unnonced inline
    // script were refused by CSP; the nonced inline script ran.
    // The script's UTF-8 `charset` wins over the document's windows-1252.
    assert!(
        dom.contains("local=\"data-app\" value=\"r\u{e1}n\""),
        "{dom}"
    );
    assert!(dom.contains("local=\"data-nonce\" value=\"ran\""), "{dom}");
    assert!(!dom.contains("local=\"data-evil\""), "{dom}");
    assert!(!dom.contains("local=\"data-inline\""), "{dom}");
    let log = stderr(&output);
    assert!(log.contains("CSP: Refused"), "{log}");
    // The CSP refusal happened before any request reached b.test.
    let requests = server.requests();
    assert!(!requests.iter().any(|r| r.target == "/evil.js"));
    // The cookie set by the document is sent with its subresources.
    let script = requests.iter().find(|r| r.target == "/app.js").unwrap();
    assert_eq!(script.header("cookie"), Some("session=1"));
    // Fetch metadata goes only to potentially trustworthy URLs.
    assert_eq!(script.header("sec-fetch-dest"), None);
    assert_eq!(script.header("referer"), Some("http://a.test/"));
}

#[test]
fn stylesheets_follow_redirects_and_need_text_css() {
    let server = server();
    let [a, b] = grants(&server);
    let output = rustic(&[
        "dump-style",
        "--allow-origin",
        &a,
        "--allow-origin",
        &b,
        "http://a.test/",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let dump = stdout(&output);
    assert!(dump.contains("rgb(1, 2, 3)"), "{dump}");
    assert!(!dump.contains("margin-left: 7px"), "{dump}");
    assert!(stderr(&output).contains("not text/css"));
}

#[test]
fn ungranted_origins_and_bad_grants_fail_without_sending() {
    let server = server();
    let [a, _] = grants(&server);
    // c.test is not granted: the navigation fails in the transport.
    let output = rustic(&["dump-dom", "--allow-origin", &a, "http://c.test/"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("navigation failed"),
        "{}",
        stderr(&output)
    );
    // Grants must route to loopback.
    let output = rustic(&[
        "dump-dom",
        "--allow-origin",
        "http://a.test=192.0.2.1:80",
        "http://a.test/",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(server.requests().is_empty());
}

#[test]
fn https_documents_verify_certificates_against_the_given_roots() {
    let tls = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rustic-net/tests/fixtures/tls/"
    );
    let read = |name: &str| std::fs::read(format!("{tls}{name}")).unwrap();
    let server = FixtureServer::start_tls(
        [(
            "/",
            Reply::response(
                200,
                &[("Content-Type", "text/html")],
                b"<!doctype html><p>secure",
            ),
        )],
        &read("good.pem"),
        &read("good.key"),
    )
    .unwrap();
    let grant = format!("https://a.test={}", server.address());
    let ca = format!("{tls}ca.pem");
    let output = rustic(&[
        "dump-dom",
        "--ca-file",
        &ca,
        "--allow-origin",
        &grant,
        "https://a.test/",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("text \"secure\""));
    // Other roots: the certificate is refused and nothing is bypassed.
    let other = format!("{tls}untrusted-ca.pem");
    let output = rustic(&[
        "dump-dom",
        "--ca-file",
        &other,
        "--allow-origin",
        &grant,
        "https://a.test/",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains("certificate"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn undeclared_encodings_fall_back_by_locale_and_late_declarations_reload() {
    // The declaration lies beyond the 64 KiB prescan.
    let mut late = b"<!doctype html><p>caf\xe9</p>".to_vec();
    for _ in 0..8 * 1024 {
        late.extend_from_slice(b"<b>x</b> ");
    }
    late.extend_from_slice(b"<meta charset=koi8-r>");
    let server = FixtureServer::start([
        (
            "/plain",
            Reply::response(200, &[("Content-Type", "text/html")], b"<p>caf\xe9"),
        ),
        (
            "/late",
            Reply::response(200, &[("Content-Type", "text/html")], &late),
        ),
        (
            "/certain",
            Reply::response(
                200,
                &[("Content-Type", "text/html; charset=windows-1252")],
                &late,
            ),
        ),
    ])
    .unwrap();
    let grant = format!("http://a.test={}", server.address());
    let dump = |extra: &[&str], path: &str| {
        let url = format!("http://a.test{path}");
        let mut args = vec!["dump-dom", "--allow-origin", &grant];
        args.extend_from_slice(extra);
        args.push(&url);
        let output = rustic(&args);
        assert!(output.status.success(), "{}", stderr(&output));
        output
    };
    assert!(stdout(&dump(&[], "/plain")).contains("text \"caf\u{e9}\""));
    assert!(stdout(&dump(&["--locale", "ru"], "/plain")).contains("text \"caf\u{439}\""));
    // KOI8-R maps 0xE9 to U+0418; the reload is announced.
    let output = dump(&[], "/late");
    assert!(stdout(&output).contains("text \"caf\u{418}\""));
    assert!(stderr(&output).contains("reloading the document as KOI8-R"));
    // A response charset is certain: the late declaration is ignored.
    let output = dump(&[], "/certain");
    assert!(stdout(&output).contains("text \"caf\u{e9}\""));
    assert!(!stderr(&output).contains("reloading"));
}

#[test]
fn profiles_keep_local_storage_and_persistent_cookies_and_private_writes_nothing() {
    let server = FixtureServer::start([(
        "/",
        Reply::response(
            200,
            &[
                ("Content-Type", "text/html"),
                ("Set-Cookie", "kept=1; Max-Age=3600"),
                ("Set-Cookie", "session=1"),
            ],
            b"<!doctype html><script>\
              var n = Number(localStorage.getItem('n') || 0) + 1;\
              localStorage.setItem('n', n);\
              sessionStorage.setItem('s', 'x');\
              document.documentElement.setAttribute('data-n', n);\
              </script>",
        ),
    )])
    .unwrap();
    let grant = format!("http://a.test={}", server.address());
    let dir = std::env::temp_dir().join(format!("rustic-cli-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let profile = dir.to_str().unwrap().to_owned();
    let run = |extra: &[&str]| {
        let mut args = vec!["dump-dom", "--run-scripts", "--allow-origin", &grant];
        args.extend_from_slice(extra);
        args.push("http://a.test/");
        let output = rustic(&args);
        assert!(output.status.success(), "{}", stderr(&output));
        stdout(&output)
    };
    let count = |dom: &str| {
        dom.split("local=\"data-n\" value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .map(str::to_owned)
    };
    assert_eq!(count(&run(&["--profile", &profile])).as_deref(), Some("1"));
    assert_eq!(count(&run(&["--profile", &profile])).as_deref(), Some("2"));
    // The second run sent only the persistent cookie saved by the first.
    let requests = server.requests();
    assert_eq!(requests[0].header("cookie"), None);
    assert_eq!(requests[1].header("cookie"), Some("kept=1"));
    // Ephemeral and private runs start empty and leave the profile alone.
    assert_eq!(count(&run(&[])).as_deref(), Some("1"));
    assert_eq!(count(&run(&["--private"])).as_deref(), Some("1"));
    assert_eq!(count(&run(&["--profile", &profile])).as_deref(), Some("3"));
    // A held lock refuses a second user of the profile.
    std::fs::write(dir.join("LOCK"), "1\n").unwrap();
    let mut args = vec!["dump-dom", "--run-scripts", "--allow-origin", &grant];
    args.extend_from_slice(&["--profile", &profile, "http://a.test/"]);
    let output = rustic(&args);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("in use"), "{}", stderr(&output));
    let output = rustic(&["dump-dom", "--profile", &profile, "--private", "x.html"]);
    assert_eq!(output.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn csp_blocks_unnonced_inline_styles_and_style_attributes() {
    let server = FixtureServer::start([(
        "/",
        Reply::response(
            200,
            &[
                ("Content-Type", "text/html"),
                ("Content-Security-Policy", "style-src 'nonce-ok'"),
            ],
            b"<!doctype html><style nonce=ok>#a { color: rgb(1, 1, 1) }</style>\
              <style>#a { margin-left: 9px }</style>\
              <p id=a style=\"padding-left: 5px\">x</p>",
        ),
    )])
    .unwrap();
    let grant = format!("http://a.test={}", server.address());
    let output = rustic(&["dump-style", "--allow-origin", &grant, "http://a.test/"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let dump = stdout(&output);
    assert!(dump.contains("@style 1: applied"), "{dump}");
    assert!(
        dump.contains("@style 2: blocked: Content Security Policy"),
        "{dump}"
    );
    assert!(dump.contains("rgb(1, 1, 1)"), "{dump}");
    assert!(!dump.contains("margin-left: 9px"), "{dump}");
    assert!(!dump.contains("padding-left: 5px"), "{dump}");
    assert!(stderr(&output).contains("style-src"), "{}", stderr(&output));
}
