//! Out-of-process frames (M7, ISO-01, ADR 0062): every `iframe` document
//! renders in a renderer of its own with a grant of its own, the broker
//! composites its pixels under what the parent paints above it, and a
//! frame that is refused, crashes or lies costs only its own pixels.

use std::path::PathBuf;
use std::process::{Command, Output};

use rustic_base::CancellationToken;
use rustic_codecs::{PngLimits, decode_png};
use rustic_net::fixture::{FixtureServer, Reply};

fn rustic(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
    command.args(args);
    for (name, value) in env {
        command.env(name, value);
    }
    if args.contains(&"--single-process") {
        command.env_remove("RUSTIC_SANDBOX");
    }
    command.output().expect("run rustic")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("rustic-frames-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, contents) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        Self(root)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Render `target` at 200x100 and return the RGBA pixels.
fn render(dir: &Dir, extra: &[&str], target: &str, env: &[(&str, &str)]) -> (Output, Vec<u8>) {
    let png = dir.path("out.png");
    let mut args = vec!["render", "--viewport", "200x100", "--output", &png];
    args.extend_from_slice(extra);
    args.push(target);
    let output = rustic(&args, env);
    let pixels = std::fs::read(&png)
        .ok()
        .and_then(|bytes| {
            decode_png(&bytes, PngLimits::default(), &CancellationToken::default()).ok()
        })
        .map(|image| {
            assert_eq!((image.width, image.height), (200, 100));
            image.pixels
        })
        .unwrap_or_default();
    let _ = std::fs::remove_file(&png);
    (output, pixels)
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * 200 + x) * 4;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// One frame per 40px column: `<iframe>`s 40x40 at x = 0, 40, 80, ….
fn frames_page(frames: &[&str]) -> String {
    let mut page = String::from(
        "<!doctype html><style>body{margin:0;background:#0f0} \
         iframe{border:0;width:40px;height:40px;display:block;position:absolute;top:0}</style>",
    );
    for (index, frame) in frames.iter().enumerate() {
        page.push_str(&format!(
            "<iframe {frame} style=\"left:{}px\"></iframe>",
            index * 40
        ));
    }
    page
}

const BLUE_PAGE: &str = "<body style=\"margin:0;background:#00f\">";

#[test]
fn local_frames_render_in_their_own_renderers_under_parent_content() {
    let page = frames_page(&[
        "src=\"frame.html\"",
        "src=\"sub/frame.html\"",
        "srcdoc=\"<body style='background:#00f'>\"",
        "src=\"data:text/html,<body style='background:%2300f'>\"",
        "src=\"../outside.html\"",
    ])
    .replace(
        "</style>",
        "</style><div style=\"position:absolute;left:0;top:20px;width:10px;height:10px;\
         background:#f00;opacity:.5;z-index:1\"></div>",
    );
    let dir = Dir::new(
        "local",
        &[
            ("site/page.html", &page),
            ("site/frame.html", BLUE_PAGE),
            ("site/sub/frame.html", BLUE_PAGE),
            ("outside.html", BLUE_PAGE),
        ],
    );
    let (output, pixels) = render(&dir, &[], &dir.path("site/page.html"), &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    for column in 0..4 {
        assert_eq!(pixel(&pixels, column * 40 + 20, 10), BLUE, "frame {column}");
    }
    // A file outside the document's directory is never granted: white.
    assert_eq!(pixel(&pixels, 180, 10), WHITE);
    assert!(text(&output.stderr).contains("outside.html"));
    // Content above a frame stays above it, composited through opacity.
    assert_eq!(pixel(&pixels, 5, 25), [128, 0, 128, 255]);
    assert_eq!(pixel(&pixels, 20, 60), GREEN);
    // The same page with frames painted white in one process (TD-178).
    let (output, single) = render(
        &dir,
        &["--single-process"],
        &dir.path("site/page.html"),
        &[],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(pixel(&single, 20, 10), WHITE);
}

#[test]
fn a_frame_grant_is_its_own_directory_and_a_failing_frame_costs_only_its_pixels() {
    let page = frames_page(&["src=\"sub/frame.html\"", "src=\"frame.html\""]);
    let dir = Dir::new(
        "grant",
        &[
            ("site/page.html", &page),
            ("site/inside.txt", "inside"),
            ("site/frame.html", BLUE_PAGE),
            ("site/sub/frame.html", BLUE_PAGE),
        ],
    );
    let target = dir.path("site/page.html");
    // A compromised frame renderer cannot read its parent's files.
    let (output, pixels) = render(
        &dir,
        &[],
        &target,
        &[
            ("RUSTIC_TEST_FAULT_URL", "sub/frame.html"),
            ("RUSTIC_TEST_FAULT", "probe"),
        ],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("frame probe parent-file: Blocked"),
        "{stderr}"
    );
    assert!(
        stderr.contains("frame probe other-site: Blocked"),
        "{stderr}"
    );
    assert_eq!(pixel(&pixels, 20, 10), BLUE);
    // A crashed frame renderer, and one that claims a frame it was never
    // given, lose their frame; the page and the other frame render.
    for fault in ["crash", "forge"] {
        let (output, pixels) = render(
            &dir,
            &[],
            &target,
            &[
                ("RUSTIC_TEST_FAULT_URL", "sub/frame.html"),
                ("RUSTIC_TEST_FAULT", fault),
            ],
        );
        assert!(output.status.success(), "{fault}: {}", text(&output.stderr));
        assert!(text(&output.stderr).contains("frame \"file:"), "{fault}");
        assert_eq!(pixel(&pixels, 20, 10), WHITE, "{fault}");
        assert_eq!(pixel(&pixels, 60, 10), BLUE, "{fault}");
    }
    // A page that frames itself stops at the nesting limit.
    let dir = Dir::new(
        "nested",
        &[(
            "site/page.html",
            "<body style=\"margin:0;background:#00f\">\
             <iframe src=\"page.html\" style=\"border:0;width:100px;height:50px\"></iframe>",
        )],
    );
    let (output, pixels) = render(&dir, &[], &dir.path("site/page.html"), &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(pixel(&pixels, 150, 80), BLUE);
}

#[test]
fn network_frames_follow_frame_policies_and_third_party_rules() {
    let html = ("Content-Type", "text/html");
    let blue = BLUE_PAGE.as_bytes();
    let parent = frames_page(&[
        "src=\"http://b.test/frame\"",
        "src=\"http://b.test/deny\"",
        "src=\"http://b.test/ancestors\"",
        "src=\"http://b.test/sandboxed\" sandbox",
        "src=\"/same\"",
    ]);
    let server = FixtureServer::start([
        ("/", Reply::response(200, &[html], parent.as_bytes())),
        (
            "/strict",
            Reply::response(
                200,
                &[html, ("Content-Security-Policy", "frame-src 'self'")],
                parent.as_bytes(),
            ),
        ),
        (
            "/frame",
            Reply::response(
                200,
                &[html, ("Set-Cookie", "lax=1; SameSite=Lax; Max-Age=600")],
                blue,
            ),
        ),
        (
            "/deny",
            Reply::response(200, &[html, ("X-Frame-Options", "DENY")], blue),
        ),
        (
            "/ancestors",
            Reply::response(
                200,
                &[
                    html,
                    ("Content-Security-Policy", "frame-ancestors http://c.test"),
                ],
                blue,
            ),
        ),
        (
            "/sandboxed",
            Reply::response(
                200,
                &[html],
                b"<body style=\"margin:0;background:#00f\">\
                  <script>document.body.style.background = '#f00'</script>",
            ),
        ),
        (
            "/same",
            Reply::response(200, &[html, ("X-Frame-Options", "SAMEORIGIN")], blue),
        ),
    ])
    .unwrap();
    let a = format!("http://a.test={}", server.address());
    let b = format!("http://b.test={}", server.address());
    let dir = Dir::new("network", &[("placeholder", "")]);
    let grants = ["--allow-origin", &a, "--allow-origin", &b, "--run-scripts"];
    let (output, pixels) = render(&dir, &grants, "http://a.test/", &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(pixel(&pixels, 20, 10), BLUE);
    assert_eq!(pixel(&pixels, 60, 10), WHITE, "X-Frame-Options: DENY");
    assert_eq!(pixel(&pixels, 100, 10), WHITE, "frame-ancestors");
    assert_eq!(pixel(&pixels, 140, 10), BLUE, "sandbox: no scripts");
    assert_eq!(pixel(&pixels, 180, 10), BLUE, "SAMEORIGIN");
    let log = server.requests();
    let frame = log.iter().find(|r| r.target == "/frame").unwrap();
    assert_eq!(frame.header("referer"), Some("http://a.test/"));
    // With a persistent profile: b.test sets a SameSite=Lax cookie as a
    // top-level page, which its frame under a.test does not get (a third
    // party), though b.test as a top level does.
    let profile = dir.path("profile");
    let with_profile = |target: &str| {
        let mut args = grants.to_vec();
        args.extend(["--profile", &profile]);
        let (output, _) = render(&dir, &args, target, &[]);
        assert!(output.status.success(), "{}", text(&output.stderr));
        server
            .requests()
            .last()
            .unwrap()
            .header("cookie")
            .map(str::to_owned)
    };
    with_profile("http://b.test/frame");
    assert_eq!(
        with_profile("http://b.test/frame"),
        Some("lax=1".to_owned())
    );
    with_profile("http://a.test/");
    let log = server.requests();
    let framed = log.iter().rev().find(|r| r.target == "/frame").unwrap();
    assert_eq!(framed.header("cookie"), None);
    // The parent's frame-src: only same-origin frames load.
    let (output, pixels) = render(&dir, &grants, "http://a.test/strict", &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(pixel(&pixels, 20, 10), WHITE, "frame-src 'self'");
    assert_eq!(pixel(&pixels, 180, 10), BLUE);
    assert!(text(&output.stderr).contains("frame-src"));
}
