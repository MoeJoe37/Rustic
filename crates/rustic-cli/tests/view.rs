//! UI-02 desktop window (ADR 0065) and UI-01 browser shell (ADR 0074):
//! `rustic view` opens a native window and presents frames from a
//! long-lived renderer; without a window system it fails cleanly. A
//! scripted window (`RUSTIC_TEST_WINDOW_SCRIPT`) is resized, minimized and
//! restored by the real window system and driven through tabs, history,
//! address entry, find and zoom; the browser logs its state after each
//! frame. `render --report-scroll` reports where it painted.

#![cfg(feature = "window")]

use std::process::{Command, Output};

fn rustic(args: &[&str], display: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
    command.args(args);
    if !display {
        command
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env_remove("DISPLAY");
    }
    command.output().expect("run rustic")
}

fn page(name: &str, html: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("rustic-view-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(&page, html).unwrap();
    let shown = page.to_string_lossy().into_owned();
    (dir, shown)
}

const TALL: &str = "<!doctype html><body style=\"margin:0\"><div style=\"height:2000px;\
    background:#00f\"></div>";

#[test]
fn render_reports_the_clamped_snapped_scroll_position() {
    let (dir, page) = page("report", TALL);
    let out = dir.join("out.png").to_string_lossy().into_owned();
    let report = |scroll: &str, scale: &str| {
        let output = rustic(
            &[
                "render",
                "--viewport",
                "100x100",
                "--scale",
                scale,
                "--scroll",
                scroll,
                "--report-scroll",
                "--output",
                &out,
                &page,
            ],
            false,
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert_eq!(report("0,150", "1"), "scroll 0,150\n");
    // Clamped to the 1900px range; snapped to half pixels at scale 2.
    assert_eq!(report("0,5000", "1"), "scroll 0,1900\n");
    assert_eq!(report("0,10.3", "2"), "scroll 0,10.5\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn view_presents_frames_in_a_window_or_fails_cleanly_without_one() {
    let (dir, page) = page("window", TALL);
    let has_display = ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if has_display {
        let output = rustic(
            &["view", "--exit-after", "2", "--window", "320x240", &page],
            true,
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = rustic(&["view", "--exit-after", "1", &page], false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if cfg!(any(windows, target_os = "macos")) {
        // The window system is not found through environment variables
        // here: a desktop session presents and exits, a session without
        // one fails cleanly.
        assert!(
            output.status.success()
                || (output.status.code() == Some(1)
                    && stderr.contains("cannot reach the window system")),
            "{stderr}"
        );
    } else {
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("cannot reach the window system"));
    }
    // The window sets the view; bad options are usage errors.
    for bad in [
        vec!["view", "--viewport", "10x10", &page],
        vec!["view", "--window", "10x10", &page],
        vec!["view", "--exit-after", "0", &page],
        vec!["view"],
    ] {
        assert_eq!(rustic(&bad, false).status.code(), Some(2), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whether this run can open windows: a desktop session on Windows and
/// macOS (the CI runners have one), a display on Linux.
fn has_window_system() -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        return true;
    }
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn scripted(script: &str, page: &str, env: &[(&str, &str)]) -> (Output, Vec<String>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
    command
        .args(["view", "--window", "320x240", page])
        .env("RUSTIC_TEST_WINDOW_SCRIPT", script);
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("run rustic");
    let lines = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("window: ").map(str::to_owned))
        .collect();
    (output, lines)
}

fn states(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .filter_map(|line| line.strip_prefix("state "))
        .collect()
}

const FINDABLE: &str = "<!doctype html><body style=\"margin:0\"><p>alpha beta gamma beta</p>\
    <div style=\"height:2000px;background:#00f\"></div><p>beta end</p>";

#[test]
fn the_window_follows_resizes_and_survives_minimize_and_restore() {
    if !has_window_system() {
        return;
    }
    let (dir, page) = page("lifecycle", FINDABLE);
    let (output, lines) = scripted("resize 400x300; minimize; restore; key End", &page, &[]);
    assert!(
        output.status.success(),
        "{lines:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let opened = lines.iter().find(|l| l.starts_with("opened ")).unwrap();
    let scale: f64 = opened.rsplit(' ').next().unwrap().parse().unwrap();
    let device = |css: f64| (css * scale).round() as u32;
    // The window system applied the resize; the next frame has that size.
    let resized = format!("resized {}x{}", device(400.0), device(300.0));
    assert!(lines.iter().any(|l| l.starts_with(&resized)), "{lines:?}");
    let frame = format!("frame {}x{}", device(400.0), device(300.0));
    assert!(lines.iter().any(|l| l.starts_with(&frame)), "{lines:?}");
    // Nothing is presented while hidden: between "hidden" and "visible"
    // there is no frame.
    if let Some(hidden) = lines.iter().position(|l| l == "hidden") {
        let shown = lines[hidden..]
            .iter()
            .position(|l| l == "visible")
            .map_or(lines.len(), |at| hidden + at);
        assert!(
            !lines[hidden..shown].iter().any(|l| l.starts_with("frame ")),
            "{lines:?}"
        );
    }
    if cfg!(windows) {
        // Windows reports a minimized window as zero-sized.
        assert!(lines.iter().any(|l| l == "minimized"), "{lines:?}");
    }
    // After restoring, the page still answers: End scrolls to the bottom.
    let last = *states(&lines).last().unwrap();
    assert!(!last.contains("scroll=0,0 "), "{last}");
    assert!(lines.last().is_some_and(|l| l == "done"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `compositor.recovery`: a lost window surface is made again and the
/// page keeps presenting into it with its state intact.
#[test]
fn a_lost_window_surface_is_recreated_and_the_page_keeps_presenting() {
    if !has_window_system() {
        return;
    }
    let (dir, page) = page("surface", FINDABLE);
    let (output, lines) = scripted("lose-surface; key End; lose-surface; frame", &page, &[]);
    assert!(
        output.status.success(),
        "{lines:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        lines.iter().filter(|l| *l == "surface dropped").count(),
        2,
        "{lines:?}"
    );
    assert_eq!(
        lines.iter().filter(|l| *l == "surface recreated").count(),
        2,
        "{lines:?}"
    );
    // Every drop is followed by a recreated surface, then a frame.
    for (at, _) in lines
        .iter()
        .enumerate()
        .filter(|(_, l)| *l == "surface dropped")
    {
        let rest = &lines[at..];
        let made = rest.iter().position(|l| l == "surface recreated").unwrap();
        assert!(
            rest[made..].iter().any(|l| l.starts_with("frame ")),
            "{lines:?}"
        );
    }
    // The page kept its state across the second loss: still scrolled.
    let last = *states(&lines).last().unwrap();
    assert!(!last.contains("scroll=0,0 "), "{last}");
    assert!(lines.last().is_some_and(|l| l == "done"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_browser_shell_drives_tabs_history_find_and_zoom() {
    if !has_window_system() {
        return;
    }
    let (dir, first) = page("shell", FINDABLE);
    let second = dir.join("second.html");
    std::fs::write(&second, "<!doctype html><p>second page").unwrap();
    let second = second.to_string_lossy().into_owned();
    let script = format!(
        "key mod+f; text beta; key Enter; key Enter; key Escape; \
         key mod+l; text {second}; key Enter; key alt+Left; key alt+Right; \
         key mod+t; text no such thing; key Enter; key Escape; key mod+w; \
         key mod+=; key mod+=; key mod+0; key mod+-; key F5"
    );
    let (output, lines) = scripted(&script, &first, &[]);
    assert!(
        output.status.success(),
        "{lines:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let states = states(&lines);
    let has = |needle: &str| states.iter().any(|state| state.contains(needle));
    // Find: three matches; next moves to the second, then the third,
    // which is below the fold, so the page scrolled to it.
    assert!(
        has("focus=find load=idle history=1/1 zoom=1 scroll=0,0 find=0/3"),
        "{states:#?}"
    );
    assert!(has("find=1/3"), "{states:#?}");
    assert!(
        states
            .iter()
            .any(|s| s.contains("find=2/3") && !s.contains("scroll=0,0 "))
    );
    // Address entry navigates; back and forward walk the history.
    assert!(has("history=2/2"), "{states:#?}");
    assert!(has("history=1/2"), "{states:#?}");
    assert!(has("second.html"), "{states:#?}");
    // A new tab; a refused entry says why; closing returns to the first.
    assert!(has("tabs=2 active=1 focus=address"), "{states:#?}");
    assert!(
        has("notice=[\"no such thing\" is not an address"),
        "{states:#?}"
    );
    // Zoom in twice, reset, out once; reload settles again.
    assert!(has("zoom=1.25"), "{states:#?}");
    let last = *states.last().unwrap();
    assert!(
        last.starts_with("tabs=1 active=0 focus=page load=idle history=2/2 zoom=0.9"),
        "{last}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fractional_scales_size_the_page_in_device_pixels() {
    // X11 lets a test pick the scale; other systems use the display's.
    if cfg!(any(windows, target_os = "macos")) || std::env::var_os("DISPLAY").is_none() {
        return;
    }
    let (dir, page) = page("scale", FINDABLE);
    let (output, lines) = scripted(
        "frame",
        &page,
        &[("WINIT_X11_SCALE_FACTOR", "1.5"), ("WAYLAND_DISPLAY", "")],
    );
    assert!(
        output.status.success(),
        "{lines:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 320x240 logical at 1.5: 480x360 device pixels; the 56px strip
    // takes 84 rows.
    assert!(
        lines
            .iter()
            .any(|l| l == "frame 480x360 scale 1.5 strip 84"),
        "{lines:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A page whose focused field turns the page green once it holds the
/// composed text and a typed key. The field is not rendered (form controls
/// are not laid out yet), but focus, keys and composition reach it all the
/// same.
const COMPOSE: &str = "<!doctype html><body style=\"margin:0;background:rgb(255,0,0)\">\
    <input id=f style=\"display:none\"><script>\
    const f = document.getElementById('f'); f.focus();\
    f.addEventListener('input', () => { if (f.value == '日本x')\
      document.body.setAttribute('style', 'margin:0;background:rgb(0,128,0)'); });\
    </script>";

#[test]
fn input_methods_compose_into_the_focused_page_field() {
    if !has_window_system() {
        return;
    }
    let (dir, page) = page("compose", COMPOSE);
    let capture = dir.join("frames");
    std::fs::create_dir_all(&capture).unwrap();
    let (output, lines) = scripted(
        "frame; ime-on; preedit にほ; commit 日本; ime-off; key x; frame",
        &page,
        &[("RUSTIC_TEST_WINDOW_CAPTURE", &capture.to_string_lossy())],
    );
    assert!(
        output.status.success(),
        "{lines:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let states = states(&lines);
    // The renderer reports the focused field, so the window enables its
    // input method there; the preedit is a composition in the page.
    assert!(states[0].contains("page-edit=field"), "{states:#?}");
    assert!(
        states.iter().any(|s| s.contains("page-edit=composing")),
        "{states:#?}"
    );
    assert!(states.last().unwrap().contains("page-edit=field"));
    // The last captured frame shows the page after `input` saw "日本x".
    assert_eq!(last_captured_pixel(&capture), [0, 128, 0, 255]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Red until clicked, then green: a page that keeps what happened to it.
const CLICKED: &str = "<!doctype html><body style=\"margin:0;background:rgb(255,0,0)\"><script>\
    document.addEventListener('click', () =>\
      document.body.setAttribute('style', 'margin:0;background:rgb(0,128,0)'));\
    </script>";

/// The pixel near the bottom-left corner of the last captured frame.
fn last_captured_pixel(capture: &std::path::Path) -> [u8; 4] {
    last_captured_corners(capture).0
}

/// The pixels near the bottom-left and bottom-right corners of the last
/// captured frame.
fn last_captured_corners(capture: &std::path::Path) -> ([u8; 4], [u8; 4]) {
    let mut frames: Vec<_> = std::fs::read_dir(capture)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    frames.sort_by_key(|path| {
        path.file_stem()
            .and_then(|stem| stem.to_str()?.strip_prefix("frame-")?.parse::<u32>().ok())
    });
    let bytes = std::fs::read(frames.last().unwrap()).unwrap();
    let image = rustic_codecs::decode_png(
        &bytes,
        rustic_codecs::PngLimits::default(),
        &rustic_base::CancellationToken::default(),
    )
    .unwrap();
    let row = (image.height as usize - 4) * image.width as usize;
    let pixel = |x: usize| -> [u8; 4] {
        let at = (row + x) * 4;
        image.pixels[at..at + 4].try_into().unwrap()
    };
    (pixel(4), pixel(image.width as usize - 4))
}

#[test]
fn background_tabs_keep_their_live_page() {
    if !has_window_system() {
        return;
    }
    let (dir, first) = page("background", CLICKED);
    let second = dir.join("second.html");
    std::fs::write(&second, "<!doctype html><p>second page").unwrap();
    let capture = dir.join("frames");
    std::fs::create_dir_all(&capture).unwrap();
    let script = format!(
        "click 100,180; frame; key mod+t; text {}; key Enter; frame; key mod+1; frame",
        second.to_string_lossy()
    );
    let (output, lines) = scripted(
        &script,
        &first,
        &[("RUSTIC_TEST_WINDOW_CAPTURE", &capture.to_string_lossy())],
    );
    assert!(
        output.status.success(),
        "{lines:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let states = states(&lines);
    // Both tabs' pages are live while the second is shown.
    assert!(
        states
            .iter()
            .any(|s| s.starts_with("tabs=2 active=1") && s.ends_with("live-pages=2")),
        "{states:#?}"
    );
    // Back on the first tab, the page still has its click: it was not
    // loaded again.
    let last = *states.last().unwrap();
    assert!(last.starts_with("tabs=2 active=0"), "{last}");
    assert_eq!(last_captured_pixel(&capture), [0, 128, 0, 255]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The curated site corpus (`tests/corpus`, ADR 0074): each page checks
/// its own layout, images, events or timers and turns its bottom-left
/// square from red to green when they hold. Every page runs in the
/// desktop alpha's window, driven by the real window system, on each
/// desktop platform CI covers.
#[test]
fn the_curated_site_corpus_runs_in_the_desktop_window() {
    if !has_window_system() {
        return;
    }
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    // The browser strip is two 28px rows above the page.
    let pages = [
        ("article", "frame"),
        ("blog", "frame"),
        ("landing", "frame"),
        ("gallery", "frame; frame"),
        ("docs", "frame; wheel 0,-5; frame"),
        ("app", "frame; click 180,76; frame"),
        (
            "motion",
            "frame; frame; frame; frame; frame; frame; frame; frame",
        ),
        ("world", "frame"),
        ("keys", "frame; key a; key shift+B; key Enter; frame"),
    ];
    let scratch = std::env::temp_dir().join(format!("rustic-corpus-{}", std::process::id()));
    let mut failures = Vec::new();
    for (name, script) in pages {
        let capture = scratch.join(name);
        std::fs::create_dir_all(&capture).unwrap();
        let page = corpus.join(format!("{name}.html"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
        command
            .args(["view", "--window", "800x600"])
            .arg(&page)
            .env("RUSTIC_TEST_WINDOW_SCRIPT", script)
            .env("RUSTIC_TEST_WINDOW_CAPTURE", &capture);
        let output = command.output().expect("run rustic");
        if !output.status.success() {
            failures.push(format!(
                "{name}: exit {:?}\n{}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            ));
            continue;
        }
        let (status, corner) = last_captured_corners(&capture);
        if status != [0, 128, 0, 255] {
            failures.push(format!(
                "{name}: status {status:?}\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        // The sticky footer is read from the frame: script rectangles do
        // not re-solve sticky boxes yet (TD-180).
        if name == "docs" && corner != [0, 51, 102, 255] {
            failures.push(format!("docs: sticky footer {corner:?}"));
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(failures.is_empty(), "{failures:#?}");
}
