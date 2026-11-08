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
fn dump_dom_fails_cleanly_on_missing_files_directories_and_unsupported_markup() {
    let directory = env!("CARGO_TARGET_TMPDIR");
    let unsupported = Path::new(directory).join("rustic-cli-unsupported.html");
    fs::write(&unsupported, "<p>before<?target data?>after").expect("write fixture");
    let missing = Path::new(directory).join("rustic-cli-missing.html");
    let _ = fs::remove_file(&missing);
    for (path, message) in [
        (missing.as_path(), "cannot read file"),
        (Path::new(directory), "not a regular file"),
        (unsupported.as_path(), "unsupported HTML"),
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
    let output = rustic(&["dump-dom", deep]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let output = rustic(&["dump-dom", "--max-depth", "400", deep]);
    assert!(output.status.success(), "{}", stderr(&output));

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
    let output = rustic(&["dump-dom", "--max-nodes", "5", path]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
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
fn dump_layout_loads_local_css_and_rejects_unsupported_text_without_partial_stdout() {
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
    fs::write(
        &path,
        "<style>* {margin:0}</style><main>unshaped text</main>",
    )
    .unwrap();
    let output = rustic(&["dump-layout", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("TextLayout"));
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
