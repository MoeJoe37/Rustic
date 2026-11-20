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
