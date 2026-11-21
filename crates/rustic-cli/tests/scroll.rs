//! UI-05 scrolling in the CLI: `--scroll` and `--scroll-box` move painted
//! content, hit testing follows, fixed boxes stay and positions clamp.

use std::process::{Command, Output};

use rustic_base::CancellationToken;
use rustic_codecs::{PngLimits, decode_png};

fn rustic(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args(args)
        .output()
        .expect("run rustic")
}

const PAGE: &str = "<!doctype html><style>body{margin:0} div{height:100px}</style>\
    <div style=\"background:#f00\"></div><div style=\"background:#00f\"></div>\
    <section style=\"overflow:auto;height:20px\">\
      <div style=\"background:#ff0\"></div><div style=\"background:#0ff\"></div>\
    </section>\
    <div style=\"position:fixed;top:0;left:40px;width:10px;height:10px;background:#0f0\"></div>\
    <div style=\"height:1000px\"></div>";

/// Render PAGE in a 50x50 window and read the pixels at `points`.
fn pixels(name: &str, extra: &[&str], points: &[(usize, usize)]) -> Vec<[u8; 4]> {
    let dir = std::env::temp_dir().join(format!("rustic-scroll-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(&page, PAGE).unwrap();
    let png = dir.join("out.png");
    let (page, png_arg) = (page.to_string_lossy(), png.to_string_lossy());
    let mut args = vec!["render", "--viewport", "50x50", "--output", &png_arg];
    args.extend_from_slice(extra);
    args.push(&page);
    let output = rustic(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(&png).unwrap();
    let image = decode_png(&bytes, PngLimits::default(), &CancellationToken::default()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    points
        .iter()
        .map(|&(x, y)| {
            let i = (y * 50 + x) * 4;
            [
                image.pixels[i],
                image.pixels[i + 1],
                image.pixels[i + 2],
                image.pixels[i + 3],
            ]
        })
        .collect()
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const CYAN: [u8; 4] = [0, 255, 255, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

#[test]
fn the_viewport_and_scroll_containers_scroll_without_moving_fixed_boxes() {
    let at = [(5, 5), (45, 5)];
    assert_eq!(pixels("none", &[], &at), [RED, GREEN]);
    assert_eq!(pixels("view", &["--scroll", "0,100"], &at), [BLUE, GREEN]);
    // The section (fragment 4: html, body, two divs, then it) starts at
    // y=200 and shows 20px of its 200px of content.
    let section = [(5, 5)];
    assert_eq!(pixels("box0", &["--scroll", "0,200"], &section), [YELLOW]);
    assert_eq!(
        pixels(
            "box1",
            &["--scroll", "0,200", "--scroll-box", "4=0,150"],
            &section
        ),
        [CYAN]
    );
    // Positions clamp: past the end shows the last 20px (cyan), not white.
    assert_eq!(
        pixels(
            "clamp",
            &["--scroll", "0,200", "--scroll-box", "4=0,9999"],
            &section
        ),
        [CYAN]
    );
}

#[test]
fn hit_testing_follows_the_scroll_position_and_bad_values_are_usage_errors() {
    let dir = std::env::temp_dir().join(format!("rustic-scroll-hit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(&page, PAGE).unwrap();
    let page = page.to_string_lossy();
    let hit = |extra: &[&str]| {
        let mut args = vec!["hit-test", "--at", "5,5"];
        args.extend_from_slice(extra);
        args.push(&page);
        String::from_utf8_lossy(&rustic(&args).stdout).into_owned()
    };
    // hit-test's 1280x720 viewport over 1220px of page: 500px of range.
    assert!(hit(&[]).contains("box=2 "), "{}", hit(&[]));
    assert!(
        hit(&["--scroll", "0,150"]).contains("box=3 "),
        "{}",
        hit(&["--scroll", "0,150"])
    );
    for bad in [
        ["--scroll", "1"],
        ["--scroll", "x,1"],
        ["--scroll-box", "a=1,1"],
    ] {
        let mut args = vec!["dump-paint"];
        args.extend_from_slice(&bad);
        args.push(&page);
        assert_eq!(rustic(&args).status.code(), Some(2), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// PAGE plus a script that scrolls the viewport and the section, and
/// records what it observes in `data-*` attributes.
const SCRIPTED: &str = "<script>\
    var seen = [];\
    document.addEventListener('scroll', () => seen.push('doc'));\
    var section = document.querySelector('section');\
    section.addEventListener('scroll', () => seen.push('section'));\
    window.scrollTo(0, 50);\
    window.scrollBy({top: 50});\
    window.scrollTo(0, 100);\
    section.scrollTop = 150;\
    var root = document.documentElement;\
    var info = [scrollX, scrollY, pageYOffset, root.scrollTop, section.scrollTop,\
      section.scrollLeft, document.body.scrollTop,\
      section.querySelector('div').getBoundingClientRect().top];\
    section.scrollTop = 1e9;\
    info.push(section.scrollTop);\
    window.scrollTo({top: -5});\
    info.push(scrollY);\
    window.scrollTo(0, 100);\
    requestAnimationFrame(() => {\
      document.body.setAttribute('data-info', info.join());\
      document.body.setAttribute('data-seen', seen.join());\
    });\
    </script>";

#[test]
fn scripts_scroll_the_viewport_and_containers_and_the_render_follows() {
    let dir = std::env::temp_dir().join(format!("rustic-scroll-js-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(&page, format!("{PAGE}{SCRIPTED}")).unwrap();
    let page_arg = page.to_string_lossy().into_owned();
    // Scripts lay out at the 1280x720 script view: 1220px of page.
    let output = rustic(&["dump-dom", "--run-scripts", &page_arg]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dom = String::from_utf8_lossy(&output.stdout);
    // scrollX, scrollY, pageYOffset, html.scrollTop (the viewport),
    // section scrollTop/Left, body.scrollTop (not a scroll container),
    // the yellow block's top (200 - 100 viewport - 150 section), the
    // clamped section maximum (180) and the clamped viewport minimum.
    assert!(
        dom.contains("local=\"data-info\" value=\"0,100,100,100,150,0,0,-50,180,0\""),
        "{dom}"
    );
    // One event per scrolled target at the rendering opportunity.
    assert!(
        dom.contains("local=\"data-seen\" value=\"doc,section\""),
        "{dom}"
    );
    // The render shows where the script left the page: the viewport at
    // 100 (the blue block) unless --scroll says otherwise.
    let png = dir.join("out.png");
    let png_arg = png.to_string_lossy().into_owned();
    let render = |extra: &[&str]| {
        let mut args = vec![
            "render",
            "--run-scripts",
            "--viewport",
            "50x50",
            "--output",
            &png_arg,
        ];
        args.extend_from_slice(extra);
        args.push(&page_arg);
        let output = rustic(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = std::fs::read(&png).unwrap();
        let image =
            decode_png(&bytes, PngLimits::default(), &CancellationToken::default()).unwrap();
        let i = (5 * 50 + 5) * 4;
        [
            image.pixels[i],
            image.pixels[i + 1],
            image.pixels[i + 2],
            image.pixels[i + 3],
        ]
    };
    assert_eq!(render(&[]), BLUE);
    assert_eq!(render(&["--scroll", "0,1"]), RED);
    let _ = std::fs::remove_dir_all(&dir);
}
