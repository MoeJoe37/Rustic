//! UI-05 incremental frames: `render --frames N` paints N rendering
//! opportunities, repainting only what changed, in split and single-process
//! runs alike.

use std::process::{Command, Output};

use rustic_base::CancellationToken;
use rustic_codecs::{PngLimits, decode_png};

fn rustic(args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustic"));
    command.args(args);
    if args.contains(&"--single-process") {
        command.env_remove("RUSTIC_SANDBOX");
    }
    command.output().expect("run rustic")
}

/// A red box that moves 300px right on each animation frame, and a
/// setInterval that recolors a second box on the third frame.
const PAGE: &str = "<!doctype html><style>body{margin:0} div{width:20px;height:20px}</style>\
    <div id=a style=\"background:#f00\"></div><div id=b style=\"background:#00f\"></div>\
    <script>\
      var a = document.getElementById('a'), b = document.getElementById('b'), x = 0;\
      function step() {\
        x += 300; a.setAttribute('style', 'background:#f00;margin-left:' + x + 'px');\
        requestAnimationFrame(step);\
      }\
      requestAnimationFrame(step);\
      var ticks = 0;\
      setInterval(() => { if (++ticks == 2) b.setAttribute('style', 'background:#0f0'); }, 16);\
    </script>";

fn pixel(path: &std::path::Path, x: usize, y: usize) -> [u8; 4] {
    let bytes = std::fs::read(path).unwrap();
    let image = decode_png(&bytes, PngLimits::default(), &CancellationToken::default()).unwrap();
    let i = (y * image.width as usize + x) * 4;
    [
        image.pixels[i],
        image.pixels[i + 1],
        image.pixels[i + 2],
        image.pixels[i + 3],
    ]
}

#[test]
fn frames_repaint_incrementally_and_match_across_process_modes() {
    let dir = std::env::temp_dir().join(format!("rustic-frames-render-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(&page, PAGE).unwrap();
    let page = page.to_string_lossy().into_owned();
    let mut outputs = Vec::new();
    for (mode, extra) in [("split", None), ("single", Some("--single-process"))] {
        let out = dir
            .join(format!("{mode}.png"))
            .to_string_lossy()
            .into_owned();
        let mut args = vec![
            "render",
            "--run-scripts",
            "--frames",
            "4",
            "--viewport",
            "1000x600",
            "--output",
            &out,
        ];
        args.extend(extra);
        args.push(&page);
        let output = rustic(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = String::from_utf8_lossy(&output.stdout).into_owned();
        // 1000x600 is 4x3 tiles (256px). Frame 0 is full. Then the box
        // moves from tile 1 to 2 (2 tiles), from 2 to 3 while the other box
        // turns green in tile 0 (3), and off the 1000px surface (only the
        // tile it leaves, 1).
        let lines: Vec<_> = report.lines().collect();
        assert_eq!(lines.len(), 4, "{report}");
        assert!(
            lines[0].starts_with("frame 0: repainted 12 of 12 tiles")
                && lines[0].ends_with(", full"),
            "{report}"
        );
        for (line, tiles) in lines[1..].iter().zip([2, 3, 1]) {
            assert!(
                line.contains(&format!("repainted {tiles} of 12 tiles")),
                "{report}"
            );
        }
        let frame = |k: usize| dir.join(format!("{mode}-{k}.png"));
        // Frame k shows the box at 300k px (frame 0 ran one callback).
        for k in 0..3 {
            assert_eq!(
                pixel(&frame(k), 300 * (k + 1) + 5, 5),
                [255, 0, 0, 255],
                "{mode} frame {k}"
            );
            assert_eq!(
                pixel(&frame(k), 5, 5),
                [255, 255, 255, 255],
                "{mode} frame {k}"
            );
        }
        assert_eq!(pixel(&frame(0), 5, 25), [0, 0, 255, 255]);
        assert_eq!(pixel(&frame(3), 5, 25), [0, 255, 0, 255]);
        outputs.push(
            (0..4)
                .map(|k| std::fs::read(frame(k)).unwrap())
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(
        outputs[0], outputs[1],
        "split and single-process frames agree"
    );
    // --frames needs scripts and a count in range.
    for bad in [
        vec!["--frames", "2"],
        vec!["--run-scripts", "--frames", "0"],
    ] {
        let out = dir.join("bad.png").to_string_lossy().into_owned();
        let mut args = vec!["render", "--output", &out];
        args.extend(bad.iter());
        args.push(&page);
        assert_eq!(rustic(&args).status.code(), Some(2), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
