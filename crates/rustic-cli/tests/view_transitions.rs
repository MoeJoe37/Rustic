//! View transitions through rendered frames: `ready` after the update
//! callback, `finished` after the default group animation, and duplicate
//! `view-transition-name`s skipping the transition.

use std::process::Command;

use rustic_base::CancellationToken;
use rustic_codecs::{PngLimits, decode_png};

/// Render `body_css` and the script for 30 frames; the body's color at
/// each of `frames`.
fn colors(name: &str, css: &str, frames: &[usize]) -> Vec<[u8; 3]> {
    let dir = std::env::temp_dir().join(format!("rustic-vt-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(
        &page,
        format!(
            "<!doctype html><style>body{{margin:0;height:100px}} {css}</style>\
             <div id=a></div><div id=b></div><script>\
             var paint = color => document.body.setAttribute('style', 'background:' + color);\
             var ready = false;\
             var vt = document.startViewTransition(() => paint('#00f'));\
             vt.ready.then(() => {{ ready = true; paint('#f0f'); }}, () => paint('#800'));\
             vt.finished.then(() => {{ if (ready) paint('#0f0'); }});\
             </script>"
        ),
    )
    .unwrap();
    let png = dir.join("out.png");
    let output = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args([
            "render",
            "--run-scripts",
            "--frames",
            "30",
            "--viewport",
            "50x50",
            "--output",
        ])
        .arg(&png)
        .arg(&page)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = frames
        .iter()
        .map(|frame| {
            let bytes = std::fs::read(dir.join(format!("out-{frame}.png"))).unwrap();
            let image =
                decode_png(&bytes, PngLimits::default(), &CancellationToken::default()).unwrap();
            let i = (5 * 50 + 5) * 4;
            [image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]]
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

#[test]
fn named_elements_finish_after_the_group_animation() {
    // Frames are 1/60 s apart: ready at once, finished after 250 ms.
    assert_eq!(
        colors("named", "#a { view-transition-name: hero }", &[0, 5, 29]),
        [[255, 0, 255], [255, 0, 255], [0, 255, 0]]
    );
    // Without named elements, finished follows ready at once.
    assert_eq!(colors("none", "", &[0]), [[0, 255, 0]]);
}

#[test]
fn duplicate_names_skip_the_transition() {
    assert_eq!(
        colors(
            "duplicate",
            "#a, #b { view-transition-name: hero }",
            &[0, 29]
        ),
        [[136, 0, 0], [136, 0, 0]]
    );
}
