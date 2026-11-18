use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn rustic(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args(args)
        .output()
        .expect("run rustic")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn reports_foundation_status_without_claiming_rendering() {
    let output = rustic(&["info"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not implemented"));
}

#[test]
fn rejects_navigation_and_extra_arguments() {
    for args in [vec!["https://example.com"], vec!["info", "unexpected"]] {
        let output = rustic(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(stderr(&output).contains("not implemented"));
    }
}

#[test]
fn scripts_are_opt_in_and_local_module_timers_render_deterministically() {
    let file = fixture("scripted/timer.html");
    let file = file.to_str().unwrap();
    let inert = rustic(&["dump-dom", file]);
    assert!(inert.status.success(), "{}", stderr(&inert));
    assert!(!String::from_utf8_lossy(&inert.stdout).contains("@class="));

    let whole = rustic(&["dump-dom", "--run-scripts", file]);
    let split = rustic(&["dump-dom", "--run-scripts", "--chunk-bytes", "1", file]);
    for output in [&whole, &split] {
        assert!(output.status.success(), "{}", stderr(output));
        assert_ne!(output.stdout, inert.stdout);
    }
    assert_eq!(whole.stdout, split.stdout);

    let output = rustic(&["reftest", "--viewport", "100x100", "--run-scripts", file]);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(&output)
    );
    let inert_pixels = rustic(&["reftest", "--viewport", "100x100", file]);
    assert_eq!(inert_pixels.status.code(), Some(1));
}

#[test]
fn dump_dom_matches_the_checked_snapshot_for_every_chunk_size() {
    let input = fixture("malformed.html");
    let expected = fs::read(fixture("malformed.dump")).expect("read snapshot");
    let input = input.to_str().expect("UTF-8 fixture path");
    let length = fs::metadata(input).expect("fixture metadata").len();
    let mut chunk_sizes: Vec<String> = (1..=17).map(|size| size.to_string()).collect();
    chunk_sizes.push(length.to_string());
    chunk_sizes.push("65536".into());
    for chunk in &chunk_sizes {
        let output = rustic(&["dump-dom", "--chunk-bytes", chunk, input]);
        assert!(
            output.status.success(),
            "chunk {chunk}: {}",
            stderr(&output)
        );
        assert_eq!(output.stdout, expected, "chunk {chunk}");
        assert!(output.stderr.is_empty());
    }
    let output = rustic(&["dump-dom", "--", input]);
    assert_eq!(output.stdout, expected);
}

#[test]
fn dump_dom_rejects_urls_without_touching_the_network() {
    for url in [
        "https://example.com/",
        "http://127.0.0.1/",
        "file:///etc/hostname",
        "data:text/html,<p>x",
    ] {
        let output = rustic(&["dump-dom", url]);
        assert_eq!(output.status.code(), Some(2), "{url}");
        assert!(stderr(&output).contains("URLs"), "{url}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn dump_dom_reports_usage_errors() {
    let path = fixture("malformed.html");
    let path = path.to_str().expect("UTF-8 fixture path");
    for args in [
        vec!["dump-dom"],
        vec!["dump-dom", path, path],
        vec!["dump-dom", "--chunk-bytes", "0", path],
        vec!["dump-dom", "--max-input-bytes", "lots", path],
        vec!["dump-dom", path, "--max-output-bytes"],
        vec!["dump-dom", "--verbose", path],
    ] {
        let output = rustic(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).contains("Usage"), "{args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn dump_dom_enforces_input_and_output_budgets() {
    let path = fixture("malformed.html");
    let path = path.to_str().expect("UTF-8 fixture path");
    let length = fs::metadata(path).expect("fixture metadata").len();
    let dump_length = fs::metadata(fixture("malformed.dump"))
        .expect("snapshot metadata")
        .len();

    let exact = length.to_string();
    assert!(
        rustic(&["dump-dom", "--max-input-bytes", &exact, path])
            .status
            .success()
    );
    let under = (length - 1).to_string();
    let output = rustic(&["dump-dom", "--max-input-bytes", &under, path]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("input limit"));
    assert!(output.stdout.is_empty());

    let exact = dump_length.to_string();
    assert!(
        rustic(&["dump-dom", "--max-output-bytes", &exact, path])
            .status
            .success()
    );
    let under = (dump_length - 1).to_string();
    let output = rustic(&["dump-dom", "--max-output-bytes", &under, path]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("output byte limit"));
    assert!(output.stdout.is_empty());
}

#[test]
fn dump_dom_fails_cleanly_on_missing_files_and_directories() {
    let directory = env!("CARGO_TARGET_TMPDIR");
    let missing = Path::new(directory).join("rustic-cli-missing.html");
    let _ = fs::remove_file(&missing);
    for (path, message) in [
        (missing.as_path(), "cannot read file"),
        (Path::new(directory), "not a regular file"),
    ] {
        let output = rustic(&["dump-dom", path.to_str().expect("UTF-8 path")]);
        assert_eq!(output.status.code(), Some(1), "{path:?}");
        assert!(
            stderr(&output).contains(message),
            "{path:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn dump_dom_parser_limits_are_configurable() {
    let directory = env!("CARGO_TARGET_TMPDIR");
    let deep = Path::new(directory).join("rustic-cli-deep.html");
    fs::write(&deep, "<div>".repeat(300)).expect("write fixture");
    let deep = deep.to_str().expect("UTF-8 path");
    // Past the depth limit, elements become siblings instead of failing
    // the load (ADR 0046).
    let capped = rustic(&["dump-dom", deep]);
    assert!(capped.status.success(), "{}", stderr(&capped));
    let output = rustic(&["dump-dom", "--max-depth", "400", deep]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_ne!(output.stdout, capped.stdout);

    let noisy = Path::new(directory).join("rustic-cli-noisy.html");
    fs::write(&noisy, "<!doctype html>".to_owned() + &"</span>".repeat(20)).expect("write fixture");
    let noisy = noisy.to_str().expect("UTF-8 path");
    let full = rustic(&["dump-dom", noisy]);
    assert!(full.status.success());
    // Parse errors beyond the retention limit are counted, never fatal.
    let output = rustic(&["dump-dom", "--max-diagnostics", "5", noisy]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(output.stdout, full.stdout);

    let path = fixture("malformed.html");
    let path = path.to_str().expect("UTF-8 fixture path");
    // Past the node limit the document is truncated, and says so.
    let output = rustic(&["dump-dom", "--max-nodes", "5", path]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!output.stdout.is_empty());
    assert!(stderr(&output).contains("warning: document truncated"));
}

#[test]
fn dump_dom_escapes_hostile_paths_and_options_in_errors() {
    let output = rustic(&["dump-dom", "--x\u{1b}[2J\nfake"]);
    assert_eq!(output.status.code(), Some(2));
    let message = stderr(&output);
    assert!(
        message.contains("unknown option \"--x\\u{1b}[2J\\nfake\""),
        "{message}"
    );
    assert!(!message.contains('\u{1b}'));

    let directory = env!("CARGO_TARGET_TMPDIR");
    let missing = Path::new(directory).join("missing\u{202e}lmth.\nerror");
    let output = rustic(&["dump-dom", missing.to_str().expect("UTF-8 path")]);
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.contains("missing\\u{202e}lmth.\\nerror\""),
        "{message}"
    );
    assert!(!message.contains('\u{202e}'));
    assert_eq!(message.lines().count(), 1, "{message}");
}

#[test]
fn dump_style_matches_the_checked_snapshot_and_budgets() {
    let input = fixture("styled.html");
    let input = input.to_str().expect("UTF-8 fixture path");
    let expected = fs::read(fixture("styled.dump")).expect("read snapshot");
    for chunk in ["1", "7", "65536"] {
        let output = rustic(&["dump-style", "--chunk-bytes", chunk, "--", input]);
        assert!(
            output.status.success(),
            "chunk {chunk}: {}",
            stderr(&output)
        );
        assert_eq!(output.stdout, expected, "chunk {chunk}");
        assert!(output.stderr.is_empty());
    }
    let exact = expected.len().to_string();
    assert!(
        rustic(&["dump-style", "--max-output-bytes", &exact, input])
            .status
            .success()
    );
    for under in [expected.len() - 1, 10] {
        let output = rustic(&[
            "dump-style",
            "--max-output-bytes",
            &under.to_string(),
            input,
        ]);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("output byte limit"));
        assert!(output.stdout.is_empty());
    }
    let output = rustic(&["dump-style", "https://example.com/"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("rustic dump-style:"));
}

#[test]
fn dump_style_loads_linked_and_imported_sheets_beside_the_document() {
    let input = fixture("linked/page.html");
    let input = input.to_str().expect("UTF-8 fixture path");
    let expected = fs::read(fixture("linked.dump")).expect("read snapshot");
    for chunk in ["1", "5", "65536"] {
        let output = rustic(&["dump-style", "--chunk-bytes", chunk, input]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&expected),
            "chunk {chunk}"
        );
    }
    let output = rustic(&[
        "dump-style",
        "--max-stylesheets",
        "2",
        "--max-import-depth",
        "1",
        input,
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let dump = String::from_utf8_lossy(&output.stdout);
    assert!(dump.contains("@import \"../../../outside.css\": skipped: import depth limit"));
    assert!(dump.contains("@link 5 \"missing.css\": skipped: stylesheet budget exhausted"));
    // 2 bytes cannot hold main.css, so nothing external applies.
    let output = rustic(&["dump-style", "--max-stylesheet-bytes", "2", input]);
    let dump = String::from_utf8_lossy(&output.stdout);
    assert!(dump.starts_with("@link 1 \"css/main.css\": skipped: stylesheet budget exceeded\n"));
    assert!(!dump.contains("rgb(0, 0, 128)"));
}

#[cfg(unix)]
#[test]
fn dump_style_never_follows_links_out_of_the_document_directory() {
    let base = std::env::temp_dir().join(format!("rustic-cli-escape-{}", std::process::id()));
    let page_dir = base.join("site");
    fs::create_dir_all(&page_dir).expect("create temp dirs");
    fs::write(base.join("secret.css"), "p { width: 1px }").expect("write secret");
    fs::write(page_dir.join("ok.css"), "p { height: 2px }").expect("write sheet");
    let _ = fs::remove_file(page_dir.join("escape.css"));
    std::os::unix::fs::symlink(base.join("secret.css"), page_dir.join("escape.css"))
        .expect("symlink");
    let _ = fs::remove_file(page_dir.join("inside.css"));
    std::os::unix::fs::symlink(page_dir.join("ok.css"), page_dir.join("inside.css"))
        .expect("symlink");
    fs::write(
        page_dir.join("page.html"),
        "<link rel=stylesheet href=escape.css><link rel=stylesheet href=inside.css>\
         <link rel=stylesheet href=file:///etc/hostname><link rel=stylesheet href=.>\
         <p>x",
    )
    .expect("write page");
    let page = page_dir.join("page.html");
    let output = rustic(&["dump-style", page.to_str().expect("UTF-8 temp path")]);
    let _ = fs::remove_dir_all(&base);
    assert!(output.status.success(), "{}", stderr(&output));
    let dump = String::from_utf8_lossy(&output.stdout);
    assert!(
        dump.contains("@link 1 \"escape.css\": skipped: blocked"),
        "{dump}"
    );
    assert!(dump.contains("@link 2 \"inside.css\": applied"), "{dump}");
    assert!(
        dump.contains("@link 3 \"file:///etc/hostname\": skipped: blocked"),
        "{dump}"
    );
    assert!(
        dump.contains("@link 4 \".\": skipped: unreadable"),
        "{dump}"
    );
    assert!(
        dump.contains("height: 2px") && !dump.contains("width: 1px"),
        "{dump}"
    );
}

#[test]
fn dump_boxes_matches_snapshot_across_chunks_and_enforces_output_limit() {
    let input = fixture("boxes.html");
    let input = input.to_str().unwrap();
    let expected = fs::read(fixture("boxes.dump")).unwrap();
    for chunk in ["1", "7", "65536"] {
        let output = rustic(&["dump-boxes", "--chunk-bytes", chunk, input]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(output.stdout, expected);
    }
    let exact = expected.len().to_string();
    let short = (expected.len() - 1).to_string();
    assert!(
        rustic(&["dump-boxes", "--max-output-bytes", &exact, input])
            .status
            .success()
    );
    let output = rustic(&["dump-boxes", "--max-output-bytes", &short, input]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let output = rustic(&["dump-boxes", "https://example.com/page"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn dump_layout_matches_used_geometry_at_every_fixture_chunk_and_enforces_exact_output_budget() {
    let input = fixture("layout.html");
    let input = input.to_str().unwrap();
    let expected = fs::read(fixture("layout.dump")).unwrap();
    for chunk in ["1", "7", "65536"] {
        let output = rustic(&["dump-layout", "--chunk-bytes", chunk, input]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(output.stdout, expected);
        assert!(output.stderr.is_empty());
    }
    let exact = expected.len().to_string();
    let short = (expected.len() - 1).to_string();
    assert!(
        rustic(&["dump-layout", "--max-output-bytes", &exact, input])
            .status
            .success()
    );
    let output = rustic(&["dump-layout", "--max-output-bytes", &short, input]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let output = rustic(&["dump-layout", "https://example.com/page"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn dump_layout_loads_local_css_lays_out_text_and_rejects_unsupported_without_partial_stdout() {
    let base = std::env::temp_dir().join(format!("rustic-cli-block-layout-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("block.css"), "main {height:15px;width:100px}").unwrap();
    let path = base.join("page.html");
    fs::write(
        &path,
        "<link rel=stylesheet href=block.css><style>* {margin:0}</style><main></main>",
    )
    .unwrap();
    let output = rustic(&["dump-layout", path.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains("content=(0,0,100,15)"));
    fs::write(&path, "<style>* {margin:0}</style><main>shaped text</main>").unwrap();
    // Text lays out with the bundled test fonts.
    let output = rustic(&["dump-layout", path.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\nline 0 block=") && stdout.contains("\ntext 0 line=0 "));
    // Atomic inlines lay out on their line.
    fs::write(
        &path,
        "<style>* {margin:0}</style><main>a <span style=display:inline-block>b</span></main>",
    )
    .unwrap();
    let output = rustic(&["dump-layout", path.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains(" line=0\n"));
    // Inline tables are not supported yet: no partial output.
    fs::write(
        &path,
        "<style>* {margin:0}</style><main>a <span style=display:inline-table>b</span></main>",
    )
    .unwrap();
    let output = rustic(&["dump-layout", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("FormattingContext"));
    assert!(output.stdout.is_empty());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn dump_layout_collapsed_margins_match_the_checked_fixture_at_each_chunk_size() {
    let input = fixture("margins.html");
    let expected = fs::read(fixture("margins.dump")).unwrap();
    for chunk in ["1", "7", "65536"] {
        let output = rustic(&[
            "dump-layout",
            "--chunk-bytes",
            chunk,
            input.to_str().unwrap(),
        ]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(output.stdout, expected);
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn dump_formatting_matches_reviewed_structure_across_chunks_and_output_boundaries() {
    let path = fixture("formatting.html");
    let input = path.to_str().unwrap();
    let expected = fs::read(fixture("formatting.dump")).unwrap();
    for chunk in ["1", "7", "65536"] {
        let output = rustic(&["dump-formatting", "--chunk-bytes", chunk, input]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(output.stdout, expected);
        assert!(output.stderr.is_empty());
    }
    assert!(
        rustic(&[
            "dump-formatting",
            "--max-output-bytes",
            &expected.len().to_string(),
            input
        ])
        .status
        .success()
    );
    let output = rustic(&[
        "dump-formatting",
        "--max-output-bytes",
        &(expected.len() - 1).to_string(),
        input,
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    for option in [
        "--max-format-boxes",
        "--max-format-depth",
        "--max-format-steps",
    ] {
        let output = rustic(&["dump-formatting", option, "1", input]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(stderr(&output).contains("formatting"));
        assert_eq!(
            rustic(&["dump-formatting", option, "0", input])
                .status
                .code(),
            Some(2)
        );
    }
    assert_eq!(
        rustic(&["dump-formatting", "https://example.com/page"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn dump_formatting_shares_local_stylesheet_policy_and_reports_unsupported_without_stdout() {
    let base = std::env::temp_dir().join(format!("rustic-cli-formatting-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("flow.css"), "span {display:block}").unwrap();
    let path = base.join("page.html");
    fs::write(
        &path,
        "<link rel=stylesheet href=flow.css><main>a<span></span>b</main>",
    )
    .unwrap();
    let output = rustic(&["dump-formatting", path.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .matches("kind=anonymous-block")
            .count(),
        2
    );
    fs::write(base.join("flow.css"), "span {display:flex}").unwrap();
    let output = rustic(&["dump-formatting", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("unsupported formatting"));
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn formatting_help_defaults_match_library_budgets() {
    let output = rustic(&["--help"]);
    let help = String::from_utf8(output.stdout).unwrap();
    let limits = rustic_layout::FormattingLimits::default();
    for (option, value) in [
        ("--max-format-boxes", limits.max_boxes),
        ("--max-format-depth", limits.max_depth),
        ("--max-format-steps", limits.max_steps),
    ] {
        assert!(
            help.lines()
                .any(|line| line.contains(option) && line.contains(&format!("(default {value})")))
        );
    }
}

#[test]
fn dump_paint_matches_the_reviewed_display_list_and_output_budget() {
    let input = fixture("paint.html");
    let input = input.to_str().unwrap();
    let expected = fs::read(fixture("paint.dump")).unwrap();
    for chunk in ["1", "65536"] {
        let output = rustic(&["dump-paint", "--chunk-bytes", chunk, input]);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(output.stdout, expected);
    }
    let short = (expected.len() - 1).to_string();
    let output = rustic(&["dump-paint", "--max-output-bytes", &short, input]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn render_writes_a_deterministic_png_at_the_requested_scale() {
    let input = fixture("paint.html");
    let input = input.to_str().unwrap();
    let base = std::env::temp_dir().join(format!("rustic-cli-render-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let out = base.join("out.png");
    let out_str = out.to_str().unwrap();
    let output = rustic(&["render", "--scale", "0.5", "--output", out_str, input]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    let png = fs::read(&out).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    // IHDR: 640x360 RGBA8.
    assert_eq!(&png[16..24], &[0, 0, 2, 128, 0, 0, 1, 104]);
    assert_eq!(&png[24..26], &[8, 6]);
    let again = base.join("again.png");
    let output = rustic(&[
        "render",
        input,
        "--output",
        again.to_str().unwrap(),
        "--scale",
        "0.5",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fs::read(&again).unwrap(), png);

    // The PNG counts against the output budget; nothing is written on failure.
    let limited = base.join("limited.png");
    let short = (png.len() - 1).to_string();
    let output = rustic(&[
        "render",
        "--scale",
        "0.5",
        "--max-output-bytes",
        &short,
        "--output",
        limited.to_str().unwrap(),
        input,
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!limited.exists());

    // Text paints; unsupported paint fails without creating a file.
    let text = base.join("text.html");
    fs::write(&text, "<p>hello</p>").unwrap();
    let painted = base.join("text.png");
    let output = rustic(&[
        "render",
        "--output",
        painted.to_str().unwrap(),
        text.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(painted.exists());
    fs::write(&text, "<p style='border: 1px dashed'>hello</p>").unwrap();
    let missing = base.join("missing.png");
    let output = rustic(&[
        "render",
        "--output",
        missing.to_str().unwrap(),
        text.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!missing.exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn render_rejects_missing_output_and_invalid_scales() {
    let input = fixture("paint.html");
    let input = input.to_str().unwrap();
    for args in [
        vec!["render", input],
        vec!["render", "--output"],
        vec!["render", "--output", "x.png", "--scale", "0", input],
        vec!["render", "--output", "x.png", "--scale", "65", input],
        vec!["render", "--output", "x.png", "--scale", "NaN", input],
        vec!["render", "--output", "x.png", "--scale", "inf", input],
        vec!["render", "--output", "x.png", "--scale"],
    ] {
        let output = rustic(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
    }
    assert!(!Path::new("x.png").exists());
}

#[test]
fn reftest_reports_each_reference_and_fails_when_one_does_not_hold() {
    let input = fixture("reftest/fails.html");
    let input = input.to_str().unwrap();
    let output = rustic(&["reftest", input]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 5, "{stdout}");
    assert!(
        lines[0].starts_with(
            "FAIL match \"fails-ref.html\": 100 pixels differ, max channel difference 5"
        )
    );
    assert!(lines[1].starts_with("PASS mismatch \"fails-ref.html\""));
    assert_eq!(
        lines[2],
        "FAIL match \"../paint.html\": outside the test's directory"
    );
    assert_eq!(lines[3], "FAIL match \"missing.html\": not found");
    assert!(lines[4].starts_with("FAIL "));
    // A page without references is an error, not a pass.
    let output = rustic(&["reftest", fixture("paint.html").to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no rel=match or rel=mismatch reference"));
}

#[test]
fn reftest_rejects_output_and_invalid_arguments() {
    let input = fixture("reftest/fails.html");
    let input = input.to_str().unwrap();
    for args in [
        vec!["reftest"],
        vec!["reftest", "--output", "x.png", input],
        vec!["reftest", "--scale", "0", input],
        vec!["reftest", "--bogus", input],
    ] {
        let output = rustic(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn hit_test_reports_the_topmost_box_at_each_point() {
    let input = fixture("paint.html");
    let input = input.to_str().unwrap();
    let output = rustic(&[
        "hit-test",
        "--at",
        "20,20",
        "--at",
        "30,32",
        "--at",
        "5,5",
        "--at",
        "1280,0",
        "--at",
        "0.5,719.9",
        input,
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hit 20,20 box=2 node=8:1 div\nhit 30,32 box=3 node=9:1 div\nhit 5,5 box=1 node=7:1 body\n\
         hit 1280,0 none\nhit 0.5,719.9 box=0 node=2:1 html\n"
    );
    let output = rustic(&["hit-test", "--at", "1,1", "--max-output-bytes", "5", input]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    for args in [
        vec!["hit-test", input],
        vec!["hit-test", "--at", "1", input],
        vec!["hit-test", "--at", "NaN,1", input],
        vec!["hit-test", "--at"],
        vec!["hit-test", "--at", "1,1"],
    ] {
        let output = rustic(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn images_load_locally_within_budgets_and_break_cleanly() {
    let input = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reftests/image-scaled.html");
    let input = input.to_str().unwrap();
    let dump = |extra: &[&str]| {
        let mut args = vec!["dump-paint"];
        args.extend_from_slice(extra);
        args.push(input);
        let output = rustic(&args);
        assert!(output.status.success(), "{}", stderr(&output));
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let loaded = dump(&[]);
    assert!(
        loaded.contains(" image image=0 rect=(7,7,50,30) size=1x1 "),
        "{loaded}"
    );
    // Any exhausted budget breaks the image: no content, layout continues.
    for limit in [
        ["--max-image-bytes", "10"],
        ["--max-image-decoded-bytes", "3"],
    ] {
        let broken = dump(&limit);
        assert!(!broken.contains(" image "), "{limit:?}: {broken}");
        assert!(broken.contains(" border "), "{limit:?}");
    }
    assert!(dump(&["--max-images", "1"]).contains(" image image=0 "));
    let output = rustic(&["dump-paint", "--max-images", "0", input]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn render_lays_the_page_out_again_for_the_window_and_zoom() {
    let base = std::env::temp_dir().join(format!("rustic-cli-view-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let write = |name: &str, html: &str| {
        let path = base.join(name);
        fs::write(&path, html).unwrap();
        path
    };
    // Viewport units and media queries follow the layout viewport.
    let page = write(
        "page.html",
        "<style>html, body { margin: 0 } div { width: 50vw; height: 10px; background: blue }
         @media (max-width: 150px) { div { background: red } }</style><div></div>",
    );
    let narrow = write(
        "narrow.html",
        "<style>html, body { margin: 0 } div { width: 50px; height: 10px; background: red }</style><div></div>",
    );
    let wide = write(
        "wide.html",
        "<style>html, body { margin: 0 } div { width: 150px; height: 10px; background: blue }</style><div></div>",
    );
    let render = |input: &Path, name: &str, args: &[&str]| {
        let out = base.join(name);
        let mut all = vec!["render", "--output", out.to_str().unwrap()];
        all.extend_from_slice(args);
        all.push(input.to_str().unwrap());
        let output = rustic(&all);
        assert!(output.status.success(), "{}", stderr(&output));
        fs::read(out).unwrap()
    };
    // A 200x100 window at 200% zoom lays out in a 100x50 viewport painted
    // at scale 2: exactly the narrow page in that viewport at scale 2.
    let zoomed = render(
        &page,
        "zoomed.png",
        &["--viewport", "200x100", "--zoom", "2"],
    );
    let expected = render(
        &narrow,
        "narrow.png",
        &["--viewport", "100x50", "--scale", "2"],
    );
    assert_eq!(zoomed, expected);
    // IHDR: the PNG is the window size, 200x100.
    assert_eq!(&zoomed[16..24], &[0, 0, 0, 200, 0, 0, 0, 100]);
    // Resizing the same page to a 300-pixel window reflows it again.
    let resized = render(&page, "resized.png", &["--viewport", "300x100"]);
    assert_eq!(
        resized,
        render(&wide, "wide.png", &["--viewport", "300x100"])
    );
    // Out-of-range views are usage errors.
    for args in [
        ["--zoom", "6"],
        ["--zoom", "x"],
        ["--viewport", "0x10"],
        ["--viewport", "100"],
    ] {
        let out = base.join("bad.png");
        let mut all = vec!["render", "--output", out.to_str().unwrap()];
        all.extend_from_slice(&args);
        all.push(page.to_str().unwrap());
        assert_eq!(rustic(&all).status.code(), Some(2), "{args:?}");
        assert!(!out.exists());
    }
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn explicit_font_formats_feed_computed_metrics_and_identical_pixels() {
    let dir = std::env::temp_dir().join(format!("rustic-cli-fonts-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let input = dir.join("fonts.html");
    fs::write(
        &input,
        "<!doctype html><style>*{margin:0;font-size:10px}p{width:2ex}</style><p>fiAVx</p>",
    )
    .unwrap();
    let font_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../rustic-text/tests/fonts/shaping");
    let input = input.to_str().unwrap();
    let mut pngs = Vec::new();
    for name in ["RusticCFFTest.otf", "RusticCFFTest.woff2"] {
        let font = font_root.join(name);
        let font = font.to_str().unwrap();
        let styles = rustic(&["dump-style", "--font", font, input]);
        assert!(styles.status.success(), "{}", stderr(&styles));
        assert!(String::from_utf8_lossy(&styles.stdout).contains("width: 10px"));
        let output = dir.join(format!("{name}.png"));
        let rendered = rustic(&[
            "render",
            "--font",
            font,
            "--viewport",
            "40x30",
            "--output",
            output.to_str().unwrap(),
            input,
        ]);
        assert!(rendered.status.success(), "{}", stderr(&rendered));
        pngs.push(fs::read(output).unwrap());
    }
    assert_eq!(pngs[0], pngs[1]);
    for args in [
        vec!["render", "--font"],
        vec!["dump-style", "--font", input, input],
    ] {
        let failed = rustic(&args);
        assert!(!failed.status.success());
        assert!(failed.stdout.is_empty());
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stylesheets_decode_with_their_charset_rule() {
    let base = std::env::temp_dir().join(format!("rustic-cli-charset-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    // "é" in ISO-8859-1 (windows-1252) is the single byte 0xE9.
    fs::write(
        base.join("latin1.css"),
        b"@charset \"iso-8859-1\"; main.\xE9 {height:15px;width:100px}",
    )
    .unwrap();
    fs::write(
        base.join("plain.css"),
        b"main.\xE9 {height:15px;width:100px}",
    )
    .unwrap();
    let path = base.join("page.html");
    let page = |sheet: &str| {
        format!(
            "<meta charset=utf-8><link rel=stylesheet href={sheet}><style>* {{margin:0}}</style><main class=\u{E9}></main>"
        )
    };
    fs::write(&path, page("latin1.css")).unwrap();
    let output = rustic(&["dump-layout", path.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains("content=(0,0,100,15)"));
    // Without the rule the sheet decodes as UTF-8, 0xE9 becomes U+FFFD and
    // the selector no longer matches.
    fs::write(&path, page("plain.css")).unwrap();
    let output = rustic(&["dump-layout", path.to_str().unwrap()]);
    let _ = fs::remove_dir_all(&base);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("content=(0,0,100,15)"));
}
