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

/// Ten 300px bands under a translucent fixed header.
fn bands(script: &str) -> String {
    let colors = [
        "#f00", "#0f0", "#00f", "#ff0", "#0ff", "#f0f", "#800", "#080", "#008", "#880",
    ];
    let mut page = String::from(
        "<!doctype html><style>body{margin:0} .band{height:300px}\
         #head{position:fixed;top:0;left:0;width:400px;height:40px;background:rgba(0,0,0,0.5)}\
         </style><div id=head></div>",
    );
    for color in colors {
        page.push_str(&format!(
            "<div class=band style=\"background:{color}\"></div>"
        ));
    }
    page.push_str(script);
    page
}

#[test]
fn scrolled_frames_reuse_cached_content_under_a_fixed_overlay() {
    let dir = std::env::temp_dir().join(format!("rustic-scene-render-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let animated = dir.join("animated.html");
    std::fs::write(
        &animated,
        bands(
            "<script>function step() { scrollBy(0, 100); requestAnimationFrame(step) }\
             requestAnimationFrame(step)</script>",
        ),
    )
    .unwrap();
    let still = dir.join("still.html");
    std::fs::write(&still, bands("")).unwrap();
    let out = dir.join("anim.png").to_string_lossy().into_owned();
    let animated = animated.to_string_lossy().into_owned();
    let output = rustic(&[
        "render",
        "--run-scripts",
        "--frames",
        "4",
        "--viewport",
        "400x300",
        "--output",
        &out,
        &animated,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout).into_owned();
    let lines: Vec<_> = report.lines().collect();
    // 400x300 spans two tile columns. Frame 0 (scrolled to 100, rows
    // 0-1 of 256px) paints 4 tiles; 200 reuses them; 300 exposes row 2
    // (2 tiles); 400 reuses rows 1-2. The overlay is the whole window.
    assert!(
        lines[0].contains("scene painted 4 content tiles, reused 0"),
        "{report}"
    );
    for line in &lines {
        assert!(line.contains("overlay 120000 pixels"), "{report}");
    }
    let painted: Vec<usize> = lines
        .iter()
        .map(|line| {
            line.split("scene painted ")
                .nth(1)
                .unwrap()
                .split(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert_eq!(painted, [4, 0, 2, 0], "{report}");
    // Each frame equals a fresh render of the still page at that position.
    for k in 0..4 {
        let fresh = dir
            .join(format!("fresh-{k}.png"))
            .to_string_lossy()
            .into_owned();
        let scroll = format!("0,{}", 100 * (k + 1));
        let still = still.to_string_lossy().into_owned();
        let output = rustic(&[
            "render",
            "--viewport",
            "400x300",
            "--scroll",
            &scroll,
            "--output",
            &fresh,
            &still,
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let frame = std::fs::read(dir.join(format!("anim-{k}.png"))).unwrap();
        assert_eq!(frame, std::fs::read(&fresh).unwrap(), "frame {k}");
    }
    // Under the header (alpha 128) the band shows at 127/255; below, as is.
    // Frame 2 is scrolled to 300: the second band (green) is at the top.
    let frame = dir.join("anim-2.png");
    assert_eq!(pixel(&frame, 5, 5), [0, 127, 0, 255]);
    assert_eq!(pixel(&frame, 5, 100), [0, 255, 0, 255]);
    let _ = std::fs::remove_dir_all(&dir);
}
