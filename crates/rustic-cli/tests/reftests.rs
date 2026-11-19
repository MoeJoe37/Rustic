//! Runs every reftest in `tests/reftests` through `rustic reftest`.

use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn every_reftest_passes_at_scales_one_and_two() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reftests");
    let mut tests: Vec<_> = fs::read_dir(&directory)
        .expect("read reftests")
        .map(|entry| entry.expect("reftest entry").path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with(".html") && !name.ends_with("-ref.html")
        })
        .collect();
    tests.sort();
    assert!(tests.len() >= 5, "reftests are missing: {tests:?}");
    for test in &tests {
        for scale in ["1", "2"] {
            let output = Command::new(env!("CARGO_BIN_EXE_rustic"))
                .args(["reftest", "--scale", scale])
                .arg(test)
                .output()
                .expect("run rustic");
            assert!(
                output.status.success(),
                "{} at scale {scale}:\n{}{}",
                test.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn the_scripted_demos_match_static_references() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scripted");
    for name in [
        "counter.html",
        "event-bubbling.html",
        "form-validation.html",
        "style-mutation.html",
        "timer.html",
    ] {
        let path = directory.join(name);
        let reftest = |options: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_rustic"))
                .args(["reftest", "--viewport", "64x48"])
                .args(options)
                .arg(&path)
                .output()
                .expect("run rustic")
        };
        let inert = reftest(&[]);
        assert_eq!(inert.status.code(), Some(1), "{name}");
        for scale in ["1", "2"] {
            let output = reftest(&["--run-scripts", "--scale", scale]);
            assert!(
                output.status.success(),
                "{name} at scale {scale}:\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
