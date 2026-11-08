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
