//! The GPU process (M8, UI-05, ADR 0068): out-of-process frames are
//! composited into their pages by a confined `rustic __gpu` child. A GPU
//! process that crashes, hangs or lies is replaced, the frame it was
//! compositing is composited by the broker with the same reference code,
//! and the page's pixels do not change.

use std::path::PathBuf;
use std::process::{Command, Output};

use rustic_base::CancellationToken;
use rustic_codecs::{PngLimits, decode_png};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rustic-gpu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Five frames, each embedding one frame of its own: five
        // compositions with one layer each, then the page's with five.
        let mut page = String::from(
            "<!doctype html><style>body{margin:0;background:#0f0} \
             iframe{border:0;width:40px;height:40px;display:block;position:absolute;top:0}</style>",
        );
        for index in 0..5 {
            page.push_str(&format!(
                "<iframe src=\"outer.html\" style=\"left:{}px\"></iframe>",
                index * 40
            ));
        }
        // Content above the frames, composited through opacity.
        page.push_str(
            "<div style=\"position:absolute;left:0;top:20px;width:10px;height:10px;\
             background:#f00;opacity:.5;z-index:1\"></div>",
        );
        let outer = "<!doctype html><style>body{margin:0;background:#00f} \
             iframe{border:0;width:20px;height:20px;display:block;position:absolute;left:0;top:0}\
             </style><iframe src=\"inner.html\"></iframe>";
        let inner = "<body style=\"margin:0;background:#ff0\">";
        for (file, contents) in [
            ("page.html", page.as_str()),
            ("outer.html", outer),
            ("inner.html", inner),
            ("outside.txt", "secret"),
        ] {
            std::fs::write(root.join(file), contents).unwrap();
        }
        Self(root)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    /// Render the page at 200x100: the exit output and the RGBA pixels.
    fn render(&self, extra: &[&str], env: &[(&str, &str)]) -> (Output, Vec<u8>) {
        let png = self.path("out.png");
        let page = self.path("page.html");
        let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
        command.args(extra);
        command.args(["render", "--viewport", "200x100", "--output", &png, &page]);
        for (name, value) in env {
            command.env(name, value);
        }
        if extra.contains(&"--no-sandbox") || extra.contains(&"--single-process") {
            command.env_remove("RUSTIC_SANDBOX");
        }
        let output = command.output().expect("run rustic");
        let pixels = std::fs::read(&png)
            .ok()
            .and_then(|bytes| {
                decode_png(&bytes, PngLimits::default(), &CancellationToken::default()).ok()
            })
            .map(|image| image.pixels)
            .unwrap_or_default();
        let _ = std::fs::remove_file(&png);
        (output, pixels)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * 200 + x) * 4;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn stopped(stderr: &str) -> usize {
    stderr.matches("the GPU process stopped").count()
}

#[test]
fn frames_composite_in_the_gpu_process_with_the_reference_pixels() {
    let dir = Dir::new("reference");
    let (output, pixels) = dir.render(&[], &[]);
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert!(!err.contains("GPU process"), "{err}");
    // Nested frames, frames and the parent's translucent box above them.
    assert_eq!(pixel(&pixels, 10, 5), [255, 255, 0, 255]);
    assert_eq!(pixel(&pixels, 30, 30), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, 190, 90), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, 5, 25), [128, 0, 128, 255]);
    // A GPU process that always crashes changes nothing on screen: each
    // frame is composited by the broker instead, and after four starts
    // the run stops starting GPU processes.
    let (output, fallback) = dir.render(&[], &[("RUSTIC_TEST_GPU_PROCESS", "crash")]);
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert_eq!(fallback, pixels);
    assert_eq!(stopped(&err), 4, "{err}");
    assert_eq!(err.matches("keeps failing").count(), 1, "{err}");
}

#[test]
fn a_crashed_gpu_process_is_replaced_for_the_next_frame() {
    let dir = Dir::new("replaced");
    let (_, reference) = dir.render(&[], &[]);
    // Each process dies on the page's five-layer composition only: the
    // five one-layer compositions before it share one process.
    let (output, pixels) = dir.render(&[], &[("RUSTIC_TEST_GPU_PROCESS", "crash-on-layers:5")]);
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert_eq!(pixels, reference);
    assert_eq!(stopped(&err), 1, "{err}");
    // Dying on every one-layer composition: each frame gets a fresh
    // process until the four starts are used; the rest is composited here.
    let (output, pixels) = dir.render(&[], &[("RUSTIC_TEST_GPU_PROCESS", "crash-on-layers:1")]);
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert_eq!(pixels, reference);
    assert_eq!(stopped(&err), 4, "{err}");
}

#[test]
fn a_lying_or_hung_gpu_process_is_cut_off_and_the_page_is_unchanged() {
    let dir = Dir::new("lying");
    let (_, reference) = dir.render(&[], &[]);
    for (mode, message) in [
        ("wrong-size", "changed the page's size"),
        ("wrong-id", "answered another request"),
        ("not-premultiplied", "not premultiplied"),
    ] {
        let (output, pixels) = dir.render(&[], &[("RUSTIC_TEST_GPU_PROCESS", mode)]);
        let err = text(&output.stderr);
        assert!(output.status.success(), "{mode}: {err}");
        assert_eq!(pixels, reference, "{mode}");
        assert!(err.contains(message), "{mode}: {err}");
    }
    let started = std::time::Instant::now();
    let (output, pixels) = dir.render(
        &["--renderer-timeout", "16"],
        &[("RUSTIC_TEST_GPU_PROCESS", "hang")],
    );
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert_eq!(pixels, reference);
    // Each hung process is stopped after 2 s (an eighth of the renderer
    // timeout), four times, and the rest is composited here.
    assert_eq!(stopped(&err), 4, "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60));
}

#[test]
fn single_process_runs_have_no_gpu_process() {
    let dir = Dir::new("single");
    let (output, _) = dir.render(
        &["--single-process"],
        &[("RUSTIC_TEST_GPU_PROCESS", "crash")],
    );
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert!(!err.contains("GPU process"), "{err}");
}

fn sandbox_available() -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        return true;
    }
    cfg!(target_os = "linux")
        && cfg!(any(target_arch = "x86_64", target_arch = "aarch64"))
        && Command::new("bwrap")
            .args(["--ro-bind", "/", "/", "true"])
            .output()
            .is_ok_and(|output| output.status.success())
}

#[test]
fn the_sandboxed_gpu_process_cannot_reach_files_network_or_programs() {
    if !sandbox_available() {
        return;
    }
    let dir = Dir::new("escape");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let outside = dir.path("outside.txt");
    let home = dir.0.to_string_lossy().into_owned();
    let env = [
        ("RUSTIC_TEST_GPU_PROCESS", "escape"),
        ("RUSTIC_TEST_ESCAPE_ADDRESS", address.as_str()),
        ("RUSTIC_TEST_ESCAPE_FILE", outside.as_str()),
        ("RUSTIC_TEST_ESCAPE_HOME", home.as_str()),
        ("RUSTIC_TEST_ESCAPE_DIR", home.as_str()),
    ];
    let probes = [
        "read-user-file",
        "list-home",
        "write-temp",
        "connect-loopback",
        "spawn-shell",
    ];
    // Unconfined, the probes work (the control run)...
    let (output, _) = dir.render(&["--no-sandbox"], &env);
    let open = text(&output.stderr);
    assert!(output.status.success(), "{open}");
    for probe in probes {
        assert!(open.contains(&format!("{probe}: allowed")), "{open}");
    }
    // ...and confined, which is the default, every one is denied.
    let (output, pixels) = dir.render(&["--sandbox"], &env);
    let confined = text(&output.stderr);
    assert!(output.status.success(), "{confined}");
    assert!(!pixels.is_empty());
    for probe in probes {
        assert!(confined.contains(&format!("{probe}: denied")), "{confined}");
        assert!(
            !confined.contains(&format!("{probe}: allowed")),
            "{confined}"
        );
    }
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
}

/// ADR 0071: with an adapter the GPU process composites on the GPU, and
/// the page is byte-identical to the CPU reference; a lost device is
/// reopened and the frame still arrives. Where the confined process finds
/// no adapter it composites on the CPU (the test then checks only the
/// pixels), unless `RUSTIC_REQUIRE_GPU=1` says this machine has one.
#[test]
fn the_hardware_compositor_matches_the_reference_and_recovers_a_lost_device() {
    let dir = Dir::new("hardware");
    let (output, reference) = dir.render(&[], &[("RUSTIC_GPU", "cpu")]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let (output, pixels) = dir.render(
        &[],
        &[("RUSTIC_GPU", "any"), ("RUSTIC_TEST_GPU_DEVICE", "report")],
    );
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert_eq!(pixels, reference, "{err}");
    if !err.contains("composited on ") {
        assert!(
            std::env::var_os("RUSTIC_REQUIRE_GPU").is_none(),
            "this machine should have a GPU adapter: {err}"
        );
        eprintln!("no adapter reached the GPU process; the CPU composited: {err}");
        return;
    }
    // Six compositions (five one-layer, then the five-layer page); the
    // device is lost before the second and reopened for it.
    let (output, pixels) = dir.render(
        &[],
        &[
            ("RUSTIC_GPU", "any"),
            ("RUSTIC_TEST_GPU_DEVICE", "report,lose-on:2"),
        ],
    );
    let err = text(&output.stderr);
    assert!(output.status.success(), "{err}");
    assert_eq!(pixels, reference, "{err}");
    assert_eq!(
        err.matches("device was lost; reopening").count(),
        1,
        "{err}"
    );
    assert_eq!(err.matches("composited on ").count(), 6, "{err}");
}
