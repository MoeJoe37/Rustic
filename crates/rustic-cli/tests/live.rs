//! TD-182 long-lived renderer (ADR 0069): `rustic drive` loads a page once
//! in one renderer and sends it input and frame requests, as the desktop
//! window does. The page keeps its state, timers and scroll position
//! between frames, and a renderer that dies or hangs is replaced.

use std::process::{Command, Output};

fn rustic(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
    command.args(args);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("run rustic")
}

fn page(name: &str, html: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("rustic-live-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(&page, html).unwrap();
    let shown = page.to_string_lossy().into_owned();
    (dir, shown)
}

fn drive(page: &str, window: &str, script: &str, env: &[(&str, &str)]) -> (String, String, i32) {
    let output = rustic(&["drive", "--window", window, "--input", script, page], env);
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

/// A page whose color records what happened to it: two clicks turn it
/// green (one click navy), Enter yellow, and a 500 ms timer blue.
const COUNTER: &str = "<!doctype html><body style=\"margin:0;background:red\"><script>\
    let n = 0;\
    const paint = (c) => document.body.setAttribute('style', 'margin:0;background:' + c);\
    document.addEventListener('click', () => { n++; paint(n == 2 ? 'rgb(0,128,0)' : 'rgb(0,0,128)'); });\
    document.addEventListener('keydown', (e) => { if (e.key == 'Enter') paint('rgb(255,255,0)'); });\
    setTimeout(() => paint('rgb(0,0,255)'), 500);\
    </script>";

#[test]
fn the_page_keeps_its_state_timers_and_input_between_frames() {
    let (dir, page) = page("state", COUNTER);
    let (stdout, stderr, code) = drive(
        &page,
        "100x100",
        "frame; probe 1,1; wait 400; frame; probe 1,1; wait 200; frame; probe 1,1; \
         click 5,5; frame; probe 1,1; click 5,5; frame; probe 1,1; key Enter; frame; probe 1,1",
        &[],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        "frame 0: 100x100 scroll 0,0 next 500ms\n\
         probe 1,1: 255,0,0,255\n\
         frame 1: 100x100 scroll 0,0 next 100ms\n\
         probe 1,1: 255,0,0,255\n\
         frame 2: 100x100 scroll 0,0 next none\n\
         probe 1,1: 0,0,255,255\n\
         frame 3: 100x100 scroll 0,0 next none\n\
         probe 1,1: 0,0,128,255\n\
         frame 4: 100x100 scroll 0,0 next none\n\
         probe 1,1: 0,128,0,255\n\
         frame 5: 100x100 scroll 0,0 next none\n\
         probe 1,1: 255,255,0,255\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const TALL: &str = "<!doctype html><body style=\"margin:0\">\
    <div style=\"height:3000px;background:rgb(0,0,255)\"></div><script>\
    document.addEventListener('keydown', () => window.scrollTo(0, 1000));\
    let k = 0;\
    function tick() { k++; if (k < 2) requestAnimationFrame(tick); }\
    requestAnimationFrame(tick);\
    </script>";

#[test]
fn user_scrolls_clamp_and_script_scrolls_are_not_reverted() {
    let (dir, page) = page("scroll", TALL);
    let (stdout, stderr, code) = drive(
        &page,
        "200x100",
        "frame; frame; scroll 0,300; frame; key x; frame; frame; scroll 0,9000; frame; \
         resize 300x150; frame",
        &[],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        // Animation frame callbacks ask for the next frame until they stop.
        "frame 0: 200x100 scroll 0,0 next 16ms\n\
         frame 1: 200x100 scroll 0,0 next none\n\
         frame 2: 200x100 scroll 0,300 next none\n\
         frame 3: 200x100 scroll 0,1000 next none\n\
         frame 4: 200x100 scroll 0,1000 next none\n\
         frame 5: 200x100 scroll 0,2900 next none\n\
         frame 6: 300x150 scroll 0,2900 next none\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn frames_follow_the_window_scale_and_zoom() {
    let (dir, page) = page("scale", TALL);
    let out = dir.join("last.png");
    let out = out.to_string_lossy().into_owned();
    let output = rustic(
        &[
            "drive",
            "--window",
            "50x40",
            "--scale",
            "2",
            "--input",
            "frame; probe 99,79; probe 100,0; zoom 2; frame",
            "--output",
            &out,
            &page,
        ],
        &[],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.starts_with("frame 0: 100x80 scroll 0,0"), "{stdout}");
    assert!(stdout.contains("probe 99,79: 0,0,255,255\nprobe 100,0: none\n"));
    assert!(stdout.contains("frame 1: 100x80"), "{stdout}");
    let png = std::fs::read(&out).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn crashed_and_hung_renderers_are_replaced_then_the_page_gives_up() {
    let (dir, page) = page("recover", COUNTER);
    // Each renderer fails at its second frame: three frames need two
    // replacements, and every frame still arrives.
    let (stdout, stderr, code) = drive(
        &page,
        "40x40",
        "frame; frame; frame",
        &[("RUSTIC_TEST_LIVE_RENDERER", "crash-on-frame:2")],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.matches("frame ").count(), 3, "{stdout}");
    assert!(stdout.ends_with("renderer restarts: 2\n"), "{stdout}");
    assert_eq!(stderr.matches("reloaded in a new renderer").count(), 2);
    // A renderer that hangs is stopped by the watchdog and replaced.
    let output = rustic(
        &[
            "drive",
            "--renderer-timeout",
            "2",
            "--window",
            "40x40",
            "--input",
            "frame; frame",
            &page,
        ],
        &[("RUSTIC_TEST_LIVE_RENDERER", "hang-on-frame:2")],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("did not answer within 2 s"), "{stderr}");
    // Every renderer failing: three replacements, then the page closes.
    let (stdout, stderr, code) = drive(
        &page,
        "40x40",
        "frame",
        &[("RUSTIC_TEST_LIVE_RENDERER", "crash-on-frame:1")],
    );
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(stdout, "");
    assert!(
        stderr.contains("replaced 3 times and the page was closed"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_scripts_and_a_direct_live_render_are_usage_errors() {
    let (dir, page) = page("usage", COUNTER);
    let out = dir.join("x.png").to_string_lossy().into_owned();
    for bad in [
        vec!["drive", &page],
        vec!["drive", "--input", "jump", &page],
        vec!["drive", "--input", "frame", "--window", "0x5", &page],
        vec!["drive", "--input", "frame", "--frames", "2", &page],
        vec!["render", "--live", "--run-scripts", "--output", &out, &page],
        vec![
            "render",
            "--single-process",
            "--live",
            "--run-scripts",
            "--output",
            &out,
            &page,
        ],
    ] {
        let output = rustic(&bad, &[]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{bad:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
