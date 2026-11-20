//! The M6 demo and gate (ROADMAP §9): explicitly allowlisted test-server
//! origins serve a page whose redirect, cookie, stylesheet, script, image
//! and `fetch()` requests all load, while a cross-origin read is denied;
//! the run uses a private profile, never personal browsing data.

use std::process::{Command, Output};

use rustic_net::fixture::{FixtureServer, Reply};

fn rustic(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args(args)
        .output()
        .expect("run rustic")
}

fn png() -> Vec<u8> {
    let surface =
        rustic_raster::Surface::new(3, 2, rustic_raster::RasterLimits::default()).unwrap();
    rustic_raster::encode_png(&surface, usize::MAX).unwrap()
}

#[test]
fn m6_demo_loads_an_allowlisted_site_and_denies_cross_origin_reads() {
    let image = png();
    let server = FixtureServer::start([
        ("/", Reply::response(302, &[("Location", "/page")], b"")),
        (
            "/page",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/html; charset=utf-8"),
                    ("Set-Cookie", "visit=1; Path=/; HttpOnly"),
                    (
                        "Content-Security-Policy",
                        "default-src 'self'; connect-src 'self' http://b.test",
                    ),
                ],
                b"<!doctype html><link rel=stylesheet href=/site.css>\
                  <div id=box></div><img src=/logo.png>\
                  <script src=/app.js></script>",
            ),
        ),
        (
            "/site.css",
            Reply::response(
                200,
                &[("Content-Type", "text/css")],
                b"#box { width: 20px; height: 10px; background: rgb(4, 5, 6) }",
            ),
        ),
        (
            "/logo.png",
            Reply::response(200, &[("Content-Type", "image/png")], &image),
        ),
        (
            "/app.js",
            Reply::response(
                200,
                &[("Content-Type", "text/javascript")],
                b"var out = {};\
                  function done(key, value) { out[key] = value;\
                    document.documentElement.setAttribute('data-' + key, value); }\
                  fetch('/api').then(r => r.json()).then(v => done('api', v.ok));\
                  fetch('http://b.test/private').then(r => r.text())\
                    .then(t => done('cross', t), e => done('cross', e.name));\
                  localStorage.setItem('seen', '1');\
                  done('storage', localStorage.getItem('seen'));",
            ),
        ),
        (
            "/api",
            Reply::response(
                200,
                &[("Content-Type", "application/json")],
                b"{\"ok\":\"yes\"}",
            ),
        ),
        ("/private", Reply::response(200, &[], b"secret")),
    ])
    .unwrap();
    let address = server.address();
    let a = format!("http://a.test={address}");
    let b = format!("http://b.test={address}");
    let common = ["--private", "--allow-origin", &a, "--allow-origin", &b];
    let run = |command: &str, scripts: bool| {
        let mut args = vec![command];
        if scripts {
            args.push("--run-scripts");
        }
        args.extend_from_slice(&common);
        args.push("http://a.test/");
        let output = rustic(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let dom = run("dump-dom", true);
    assert!(dom.contains("local=\"data-api\" value=\"yes\""), "{dom}");
    assert!(dom.contains("local=\"data-storage\" value=\"1\""), "{dom}");
    // The cross-origin response carries no CORS header: script cannot read it.
    assert!(
        dom.contains("local=\"data-cross\" value=\"TypeError\""),
        "{dom}"
    );
    assert!(!dom.contains("secret"));
    let paint = run("dump-paint", false);
    assert!(
        paint.contains("rect=(8,8,20,10) color=#040506ff"),
        "{paint}"
    );
    assert!(paint.contains(" image image=0 "), "{paint}");
    assert!(paint.contains(" size=3x2 "), "{paint}");
    let requests = server.requests();
    let find = |target: &str| {
        requests
            .iter()
            .find(|request| request.target == target)
            .unwrap_or_else(|| panic!("{target} was not requested"))
    };
    // The redirect was followed and the cookie it led to was sent with
    // every same-site subresource and fetch.
    find("/");
    for target in ["/site.css", "/logo.png", "/app.js", "/api"] {
        assert_eq!(find(target).header("cookie"), Some("visit=1"), "{target}");
    }
    // The cross-site fetch was CORS mode, sent its Origin and no cookie.
    let cross = find("/private");
    assert_eq!(cross.header("origin"), Some("http://a.test"));
    assert_eq!(cross.header("cookie"), None);
}

#[test]
fn m6_gate_refuses_unlisted_origins_and_bad_certificates_without_bypass() {
    let server = FixtureServer::start([("/", Reply::response(200, &[], b"<p>x"))]).unwrap();
    let grant = format!("http://a.test={}", server.address());
    for url in ["http://b.test/", "https://a.test/"] {
        let output = rustic(&["dump-dom", "--allow-origin", &grant, url]);
        assert_eq!(output.status.code(), Some(1), "{url}");
        assert!(output.stdout.is_empty(), "{url}");
    }
    assert!(server.requests().is_empty());
}
