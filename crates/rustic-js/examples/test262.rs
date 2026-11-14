//! Run a directory of a Test262 checkout and print the totals:
//!
//! ```text
//! cargo run --release -p rustic-js --example test262 -- CHECKOUT [DIRECTORY] [--modules] [--list FILE]
//! ```
//!
//! `DIRECTORY` defaults to `test/language`. `--list FILE` writes one line
//! per file: `pass`, `skip REASON` or `fail REASON`, then the path.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use rustic_js::test262::{RunLimits, execute_module_tree, execute_tree};

fn main() {
    let mut positional = Vec::new();
    let mut list = None;
    let mut modules = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--list" {
            list = args.next();
        } else if arg == "--modules" {
            modules = true;
        } else {
            positional.push(arg);
        }
    }
    let Some(root) = positional.first() else {
        eprintln!("usage: test262 CHECKOUT [DIRECTORY] [--modules] [--list FILE]");
        std::process::exit(2);
    };
    let directory = positional.get(1).map_or("test/language", String::as_str);
    let run = if modules {
        execute_module_tree
    } else {
        execute_tree
    };
    let report = match run(&PathBuf::from(root), directory, RunLimits::default()) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{root}: {error}");
            std::process::exit(2);
        }
    };
    // Totals per directory one level below the one that was run.
    let depth = directory.split('/').count() + 1;
    let group = |path: &str| path.split('/').take(depth).collect::<Vec<_>>().join("/");
    let mut groups: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    for path in &report.passed {
        groups.entry(group(path)).or_default()[0] += 1;
    }
    for (path, _) in &report.failed {
        groups.entry(group(path)).or_default()[1] += 1;
    }
    for (path, _) in &report.skipped {
        groups.entry(group(path)).or_default()[2] += 1;
    }
    println!(
        "{:<44} {:>7} {:>7} {:>7}",
        "directory", "pass", "fail", "skip"
    );
    for (name, [pass, fail, skip]) in &groups {
        println!("{name:<44} {pass:>7} {fail:>7} {skip:>7}");
    }
    let (pass, fail, skip) = (
        report.passed.len(),
        report.failed.len(),
        report.skipped.len(),
    );
    println!("{:<44} {pass:>7} {fail:>7} {skip:>7}", "total");
    let run = pass + fail;
    if run > 0 {
        println!(
            "{:.2}% of the {run} files that were run pass",
            100.0 * pass as f64 / run as f64
        );
    }
    if let Some(file) = list {
        let mut lines: Vec<(String, String)> = Vec::new();
        lines.extend(report.passed.iter().map(|p| (p.clone(), "pass".to_owned())));
        lines.extend(
            report
                .failed
                .iter()
                .map(|(p, r)| (p.clone(), format!("fail {r}"))),
        );
        lines.extend(
            report
                .skipped
                .iter()
                .map(|(p, r)| (p.clone(), format!("skip {r}"))),
        );
        lines.sort();
        let mut text = String::new();
        for (path, verdict) in lines {
            let _ = writeln!(text, "{path}\t{verdict}");
        }
        if let Err(error) = std::fs::write(&file, text) {
            eprintln!("{file}: {error}");
            std::process::exit(2);
        }
    }
}
