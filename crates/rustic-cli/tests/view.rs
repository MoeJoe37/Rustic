//! UI-02 desktop window (ADR 0065): `rustic view` opens a native window
//! and presents frames rendered in renderer processes; without a window
//! system it fails cleanly. `render --report-scroll` reports where it
//! painted.

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
