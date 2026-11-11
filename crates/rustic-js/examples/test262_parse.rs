//! Run the parse phase of a Test262 checkout's `test/language` tree:
//! `cargo run --release -p rustic-js --example test262_parse -- TEST262_DIR
//! [FAILURES_OUT]`. Prints totals and writes failing paths (relative to
//! the checkout) with reasons to FAILURES_OUT when given, in the format of
//! `tests/fixtures/test262_expected_failures.txt`.

use std::path::PathBuf;
use std::process::ExitCode;

use rustic_js::ParseLimits;
use rustic_js::test262::run_tree;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first().map(PathBuf::from) else {
        eprintln!("usage: test262_parse TEST262_DIR [FAILURES_OUT]");
        return ExitCode::FAILURE;
    };
    let report = match run_tree(&root, &ParseLimits::default()) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let failed = report.failures.len();
    println!(
        "test262 parse: {} pass, {failed} fail of {}; {} skipped (unsupported proposals)",
        report.passed,
        report.passed + failed,
        report.skipped
    );
    if let Some(out) = args.get(1) {
        let lines: String = report
            .failures
            .iter()
            .map(|(path, reason)| format!("{path}\t{reason}\n"))
            .collect();
        if let Err(e) = std::fs::write(out, lines) {
            eprintln!("{out}: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
