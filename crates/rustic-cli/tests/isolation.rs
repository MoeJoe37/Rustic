//! The split renderer (M7, ISO-02/ISO-07): document commands run in a
//! child process that holds no authority. A compromised renderer's
//! requests are refused at the broker, a crashing or protocol-breaking
//! renderer ends the command cleanly, and results match a single-process
//! run.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rustic_net::fixture::{FixtureServer, Reply};

fn rustic(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
    command.args(args);
    for (name, value) in env {
        command.env(name, value);
    }
    if args.contains(&"--single-process") {
        // The sandbox confines a renderer process, which this run has not.
        command.env_remove("RUSTIC_SANDBOX");
    }
    command.output().expect("run rustic")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

struct Site(PathBuf);

impl Site {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("rustic-isolation-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("site")).unwrap();
        std::fs::write(root.join("site/page.html"), "<!doctype html><p>x").unwrap();
        std::fs::write(root.join("site/inside.txt"), "inside").unwrap();
        std::fs::write(root.join("outside.txt"), "secret").unwrap();
        Self(root)
    }

    fn page(&self) -> String {
        self.0.join("site/page.html").to_string_lossy().into_owned()
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_compromised_renderer_cannot_reach_beyond_its_local_grant() {
    let site = Site::new("local");
    let output = rustic(
        &["dump-dom", &site.page()],
        &[("RUSTIC_TEST_COMPROMISED_RENDERER", "1")],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "system-file: Blocked\nparent-directory: Blocked\nsibling: read 6 bytes\n\
         unlisted-origin: Blocked\nother-scheme: Blocked\nfetch-unlisted-origin: network error\n\
         storage: Refused\n"
    );
}

#[test]
fn a_compromised_renderer_cannot_reach_beyond_its_network_grant() {
    let server = FixtureServer::start([
        (
            "/",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/html"),
                    ("Content-Security-Policy", "img-src 'self'"),
                ],
                b"<p>x",
            ),
        ),
        ("/inside.txt", Reply::response(200, &[], b"inside")),
    ])
    .unwrap();
    let grant = format!("http://a.test={}", server.address());
    let output = rustic(
        &["dump-dom", "--allow-origin", &grant, "http://a.test/"],
        &[("RUSTIC_TEST_COMPROMISED_RENDERER", "1")],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    let report = text(&output.stdout);
    // Only the granted origin's resource, under the document's CSP, loads.
    assert!(report.contains("system-file: Blocked\n"), "{report}");
    assert!(report.contains("parent-directory: NotFound\n"), "{report}");
    assert!(report.contains("sibling: read 6 bytes\n"), "{report}");
    assert!(report.contains("unlisted-origin: Blocked\n"), "{report}");
    assert!(report.contains("other-scheme: Blocked\n"), "{report}");
    assert!(
        report.contains("fetch-unlisted-origin: network error\n"),
        "{report}"
    );
    // Storage is the broker's for the granted origin: the renderer cannot
    // name another one.
    assert!(report.contains("storage: Keys([])\n"), "{report}");
    let targets: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(targets, ["/", "/outside.txt", "/inside.txt"]);
}

#[test]
fn crashed_and_protocol_breaking_renderers_end_the_command_cleanly() {
    let site = Site::new("crash");
    let output_png = site.0.join("out.png");
    let png = output_png.to_string_lossy().into_owned();
    for (env, expected) in [
        (
            "RUSTIC_TEST_RENDERER_CRASH",
            "renderer process ended before finishing",
        ),
        ("RUSTIC_TEST_COMPROMISED_RENDERER", "broke the IPC protocol"),
    ] {
        let value = if env.contains("COMPROMISED") {
            "garbage"
        } else {
            "1"
        };
        let output = rustic(&["render", "--output", &png, &site.page()], &[(env, value)]);
        assert_eq!(output.status.code(), Some(1), "{env}");
        assert!(
            text(&output.stderr).contains(expected),
            "{}",
            text(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(
            !Path::new(&output_png).exists(),
            "nothing half-made is written"
        );
    }
    // The next run gets a fresh renderer and works.
    let output = rustic(&["render", "--output", &png, &site.page()], &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output_png.exists());
}

#[test]
fn split_and_single_process_runs_agree() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let linked = fixtures.join("linked/page.html");
    let scripted = fixtures.join("scripted/timer.html");
    let cases: Vec<Vec<String>> = vec![
        vec!["dump-style".into(), linked.to_string_lossy().into_owned()],
        vec!["dump-paint".into(), linked.to_string_lossy().into_owned()],
        vec![
            "dump-dom".into(),
            "--run-scripts".into(),
            scripted.to_string_lossy().into_owned(),
        ],
        vec![
            "hit-test".into(),
            "--at".into(),
            "10,10".into(),
            linked.to_string_lossy().into_owned(),
        ],
    ];
    for case in cases {
        let args: Vec<&str> = case.iter().map(String::as_str).collect();
        let split = rustic(&args, &[]);
        let mut single_args = vec!["--single-process"];
        single_args.extend_from_slice(&args);
        let single = rustic(&single_args, &[]);
        assert!(split.status.success(), "{}", text(&split.stderr));
        assert_eq!(split.stdout, single.stdout, "{case:?}");
        assert_eq!(split.stderr, single.stderr, "{case:?}");
    }
}

fn png() -> Vec<u8> {
    let surface =
        rustic_raster::Surface::new(2, 2, rustic_raster::RasterLimits::default()).unwrap();
    rustic_raster::encode_png(&surface, usize::MAX).unwrap()
}

#[test]
fn opaque_response_blocking_and_coep_keep_other_sites_out_of_the_renderer() {
    let image = png();
    let page = |coep: bool| {
        let mut headers = vec![("Content-Type", "text/html")];
        if coep {
            headers.push(("Cross-Origin-Embedder-Policy", "require-corp"));
        }
        Reply::response(
            200,
            &headers,
            b"<!doctype html><img src=http://127.0.0.1/data.json><img src=http://127.0.0.1/pic.png>\
              <img src=http://127.0.0.1/corp.png>",
        )
    };
    let server = FixtureServer::start([
        ("/plain", page(false)),
        ("/isolated", page(true)),
        (
            "/data.json",
            Reply::response(
                200,
                &[("Content-Type", "application/json")],
                b"{\"secret\":1}",
            ),
        ),
        (
            "/pic.png",
            Reply::response(200, &[("Content-Type", "image/png")], &image),
        ),
        (
            "/corp.png",
            Reply::response(
                200,
                &[
                    ("Content-Type", "image/png"),
                    ("Cross-Origin-Resource-Policy", "cross-origin"),
                ],
                &image,
            ),
        ),
    ])
    .unwrap();
    let address = server.address();
    let grants = [
        format!("http://localhost={address}"),
        format!("http://127.0.0.1={address}"),
    ];
    // Both origins are potentially trustworthy (no mixed content), and
    // cross-site to each other.
    let paint = |path: &str| {
        let url = format!("http://localhost{path}");
        let output = rustic(
            &[
                "dump-paint",
                "--allow-origin",
                &grants[0],
                "--allow-origin",
                &grants[1],
                &url,
            ],
            &[],
        );
        assert!(output.status.success(), "{}", text(&output.stderr));
        (text(&output.stdout), text(&output.stderr))
    };
    let (plain, log) = paint("/plain");
    // ORB stops the JSON; both PNGs paint.
    assert!(log.contains("ORB: blocked"), "{log}");
    assert_eq!(plain.matches(" image image=").count(), 2, "{plain}");
    let (isolated, log) = paint("/isolated");
    // COEP require-corp: only the image with CORP may load.
    assert!(log.contains("COEP require-corp"), "{log}");
    assert_eq!(isolated.matches(" image image=").count(), 1, "{isolated}");
}

fn bubblewrap_available() -> bool {
    Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "true"])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Whether this machine can run the renderer sandbox: Windows always
/// (AppContainer), Linux with a working bubblewrap on x86-64 or aarch64.
fn sandbox_available() -> bool {
    if cfg!(windows) {
        return true;
    }
    cfg!(target_os = "linux")
        && cfg!(any(target_arch = "x86_64", target_arch = "aarch64"))
        && bubblewrap_available()
}

const ESCAPES: [&str; 5] = [
    "read-user-file",
    "list-home",
    "write-temp",
    "connect-loopback",
    "spawn-shell",
];

#[test]
fn the_sandbox_denies_files_network_and_processes_or_fails_closed() {
    let site = Site::new("sandbox");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let outside = site.0.join("outside.txt").to_string_lossy().into_owned();
    let home = site.0.to_string_lossy().into_owned();
    let probe = |mode: Option<&str>| {
        let mut args = vec![];
        args.extend(mode);
        let page = site.page();
        args.extend_from_slice(&["dump-dom", &page]);
        let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
        command
            .args(&args)
            .env_remove("RUSTIC_SANDBOX")
            .env("RUSTIC_TEST_COMPROMISED_RENDERER", "escape")
            .env("RUSTIC_TEST_ESCAPE_ADDRESS", &address)
            .env("RUSTIC_TEST_ESCAPE_FILE", &outside)
            .env("RUSTIC_TEST_ESCAPE_HOME", &home)
            .env("RUSTIC_TEST_ESCAPE_DIR", &home);
        command.output().expect("run rustic")
    };
    if !sandbox_available() {
        // No sandbox here: asking for one fails rather than run unconfined.
        let output = probe(Some("--sandbox"));
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        return;
    }
    // The unconfined control run shows the probes would work.
    let open = text(&probe(Some("--no-sandbox")).stdout);
    for escape in ESCAPES {
        assert!(open.contains(&format!("{escape}: allowed")), "{open}");
    }
    // Explicitly and by default, every probe is denied.
    for mode in [Some("--sandbox"), None] {
        let output = probe(mode);
        assert!(output.status.success(), "{}", text(&output.stderr));
        let confined = text(&output.stdout);
        for escape in ESCAPES {
            assert!(
                confined.contains(&format!("{escape}: denied")),
                "{confined}"
            );
        }
    }
    // Nothing was written outside the renderer.
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
    // Real rendering still works inside the sandbox.
    let page = site.page();
    let output = rustic(&["--sandbox", "dump-dom", &page], &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(text(&output.stdout).contains("text \"x\""));
    // The sandbox and the single-process mode exclude each other.
    let output = rustic(&["--sandbox", "--single-process", "dump-dom", &page], &[]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_bad_sandbox_setting_is_a_usage_error() {
    let site = Site::new("sandbox-setting");
    let page = site.page();
    let output = rustic(&["dump-dom", &page], &[("RUSTIC_SANDBOX", "yes")]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn a_hung_renderer_is_stopped_by_the_watchdog() {
    let site = Site::new("hang");
    let page = site.page();
    let started = std::time::Instant::now();
    let output = rustic(
        &["--renderer-timeout", "1", "dump-dom", &page],
        &[("RUSTIC_TEST_RENDERER_HANG", "1")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        text(&output.stderr).contains("did not finish within 1 s"),
        "{}",
        text(&output.stderr)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
}

fn script_site() -> FixtureServer {
    FixtureServer::start([
        (
            "/",
            Reply::response(
                200,
                &[("Content-Type", "text/html")],
                b"<!doctype html><body>\
<script src=/crash.js></script>\
<script src=/ok.js></script>",
            ),
        ),
        (
            "/crash.js",
            Reply::response(
                200,
                &[("Content-Type", "text/javascript")],
                b"document.body.setAttribute('data-crash', 'ran');",
            ),
        ),
        (
            "/ok.js",
            Reply::response(
                200,
                &[("Content-Type", "text/javascript")],
                b"document.body.setAttribute('data-ok', 'ran');",
            ),
        ),
    ])
    .unwrap()
}

#[test]
fn a_crashed_network_process_fails_its_requests_and_is_replaced() {
    let server = script_site();
    let grant = format!("http://a.test={}", server.address());
    let output = rustic(
        &[
            "dump-dom",
            "--run-scripts",
            "--allow-origin",
            &grant,
            "http://a.test/",
        ],
        &[("RUSTIC_TEST_NETWORK_PROCESS", "crash-on:/crash.js")],
    );
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    let dom = text(&output.stdout);
    // The script whose exchange died is a network error; the next one
    // runs, fetched by a fresh network process.
    assert!(!dom.contains("data-crash"), "{dom}");
    assert!(dom.contains("local=\"data-ok\" value=\"ran\""), "{dom}");
    assert!(err.contains("network process stopped"), "{err}");
    let targets: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        targets,
        ["/", "/ok.js"],
        "the crashed exchange sent nothing"
    );
}

#[test]
fn a_network_process_that_crashes_every_time_cannot_loop() {
    let server = script_site();
    let grant = format!("http://a.test={}", server.address());
    let output = rustic(
        &["dump-dom", "--allow-origin", &grant, "http://a.test/"],
        &[("RUSTIC_TEST_NETWORK_PROCESS", "crash")],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    assert!(server.requests().is_empty());
    // A single-process run has no network process and is unaffected.
    let output = rustic(
        &[
            "dump-dom",
            "--single-process",
            "--allow-origin",
            &grant,
            "http://a.test/",
        ],
        &[("RUSTIC_TEST_NETWORK_PROCESS", "crash")],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
}

#[test]
fn a_compromised_network_process_is_cut_off_by_the_broker() {
    let server = script_site();
    let grant = format!("http://a.test={}", server.address());
    for mode in [
        "credit-overrun",
        "bad-status",
        "foreign-exchange",
        "unknown-error",
    ] {
        let output = rustic(
            &["dump-dom", "--allow-origin", &grant, "http://a.test/"],
            &[("RUSTIC_TEST_NETWORK_PROCESS", mode)],
        );
        let err = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{mode}: {err}");
        assert!(output.stdout.is_empty(), "{mode}");
        assert!(
            err.contains("network process") && !err.contains("panicked"),
            "{mode}: {err}"
        );
    }
}
