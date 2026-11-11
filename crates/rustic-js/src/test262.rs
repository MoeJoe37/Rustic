//! Parse-phase classification of Test262 files (JS-01 evidence).
//!
//! Test262 (<https://github.com/tc39/test262>) files carry YAML front matter
//! between `/*---` and `---*/`. This module reads the parts that decide how
//! a file is parsed (`flags`) and whether parsing must fail (`negative`
//! with `phase: parse`), and runs the parser in each required mode. It
//! never executes code: runtime and resolution expectations are out of
//! scope until the interpreter exists (JS-07 adds the full harness).
//! [`run_tree`] runs a whole checkout; `tests/test262.rs` compares it with
//! the committed expected failures of the pinned revision.

use std::io;
use std::path::{Path, PathBuf};

use crate::{ParseError, ParseLimits, Source, parse_module, parse_script};

/// Proposal features the parser does not implement yet (TD-89). Files
/// using them are skipped and counted separately, never passed.
pub const UNSUPPORTED_FEATURES: &[&str] = &[
    "decorators",
    "explicit-resource-management",
    "import-defer",
    "source-phase-imports",
    "source-phase-imports-module-source",
];

/// What a Test262 file's front matter says about parsing it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    /// `flags: [module]`.
    pub module: bool,
    /// `flags: [onlyStrict]`.
    pub only_strict: bool,
    /// `flags: [noStrict]`.
    pub no_strict: bool,
    /// `flags: [raw]`: run the source exactly as written, sloppy only.
    pub raw: bool,
    /// `negative: { phase: parse }`: every mode must fail to parse.
    pub negative_parse: bool,
    /// `features: [...]`.
    pub features: Vec<String>,
}

impl Metadata {
    /// Whether the file needs a feature in [`UNSUPPORTED_FEATURES`].
    pub fn unsupported(&self) -> bool {
        self.features
            .iter()
            .any(|f| UNSUPPORTED_FEATURES.contains(&f.as_str()))
    }
}

/// Read the front matter of a Test262 file. Returns `None` without one.
pub fn metadata(text: &str) -> Option<Metadata> {
    let start = text.find("/*---")? + 5;
    let end = start + text[start..].find("---*/")?;
    let yaml = &text[start..end];
    let mut meta = Metadata::default();
    let mut in_negative = false;
    let mut in_features_block = false;
    for line in yaml.lines() {
        let trimmed = line.trim();
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            in_negative = trimmed.starts_with("negative:");
            in_features_block = false;
        }
        if let Some(rest) = trimmed.strip_prefix("flags:") {
            for flag in list_items(rest) {
                match flag {
                    "module" => meta.module = true,
                    "onlyStrict" => meta.only_strict = true,
                    "noStrict" => meta.no_strict = true,
                    "raw" => meta.raw = true,
                    _ => {}
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("features:") {
            meta.features.extend(list_items(rest).map(str::to_owned));
            in_features_block = rest.trim().is_empty();
        } else if in_features_block && trimmed.starts_with("- ") {
            meta.features.push(trimmed[2..].trim().to_owned());
        } else if in_negative && trimmed.starts_with("phase:") {
            meta.negative_parse = trimmed["phase:".len()..].trim() == "parse";
        }
    }
    Some(meta)
}

fn list_items(text: &str) -> impl Iterator<Item = &str> {
    text.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The outcome of parsing one file in every mode it requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every mode parsed (or failed) as expected.
    Pass,
    /// A mode parsed although the file is a negative parse test.
    UnexpectedSuccess(&'static str),
    /// A mode failed although the file should parse; with the error text.
    UnexpectedFailure(&'static str, String),
}

/// Parse `text` in the modes its metadata requires: module, or sloppy
/// and/or strict script (strict mode by prefixing `"use strict";`, as the
/// Test262 interpreting guide specifies).
pub fn run(text: &str, meta: &Metadata, limits: &ParseLimits) -> Outcome {
    let mut modes: Vec<(&'static str, bool, bool)> = Vec::new();
    if meta.module {
        modes.push(("module", true, false));
    } else {
        if !meta.only_strict {
            modes.push(("sloppy", false, false));
        }
        if !meta.no_strict && !meta.raw {
            modes.push(("strict", false, true));
        }
    }
    for (name, module, strict) in modes {
        let full;
        let source_text = if strict {
            full = format!("\"use strict\";\n{text}");
            &full
        } else {
            text
        };
        let result = match Source::from_str_limited(source_text, usize::MAX) {
            Ok(source) if module => parse_module(&source, limits),
            Ok(source) => parse_script(&source, limits),
            Err(e) => return Outcome::UnexpectedFailure(name, e.to_string()),
        };
        match (result, meta.negative_parse) {
            (Ok(_), true) => return Outcome::UnexpectedSuccess(name),
            (Err(e), false) => {
                let message = match e {
                    ParseError::Syntax(s) => s.to_string(),
                    other => other.to_string(),
                };
                return Outcome::UnexpectedFailure(name, message);
            }
            _ => {}
        }
    }
    Outcome::Pass
}

/// Totals of [`run_tree`].
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Files whose every mode behaved as expected.
    pub passed: usize,
    /// Files skipped for [`UNSUPPORTED_FEATURES`].
    pub skipped: usize,
    /// Failing files, as paths relative to the checkout with `/`
    /// separators, and the reason, sorted by path.
    pub failures: Vec<(String, String)>,
}

/// Run the parse phase over every test file of a Test262 checkout's
/// `test/language` tree, in path order. `_FIXTURE` files are imported by
/// other tests and are not tests themselves.
pub fn run_tree(root: &Path, limits: &ParseLimits) -> io::Result<Report> {
    let mut files = Vec::new();
    walk(&root.join("test/language"), &mut files)?;
    let mut report = Report::default();
    for path in &files {
        let text = std::fs::read_to_string(path)?;
        let Some(meta) = metadata(&text) else {
            continue;
        };
        if meta.unsupported() {
            report.skipped += 1;
            continue;
        }
        let relative: Vec<String> = path
            .strip_prefix(root)
            .unwrap_or(path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let reason = match run(&text, &meta, limits) {
            Outcome::Pass => {
                report.passed += 1;
                continue;
            }
            Outcome::UnexpectedSuccess(mode) => format!("parsed in {mode} mode"),
            Outcome::UnexpectedFailure(mode, message) => format!("{mode}: {message}"),
        };
        report.failures.push((relative.join("/"), reason));
    }
    Ok(report)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<_>>()?;
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "js")
            && !path.to_string_lossy().contains("_FIXTURE")
        {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_flags_and_negative_phase_are_read() {
        let text = "/*---\nflags: [onlyStrict, module]\nnegative:\n  phase: parse\n  type: SyntaxError\nfeatures: [class-fields-private]\n---*/\n$DONOTEVALUATE();\n";
        let meta = metadata(text).unwrap();
        assert!(meta.module && meta.only_strict && meta.negative_parse && !meta.raw);
        assert_eq!(meta.features, ["class-fields-private"]);
        let runtime = "/*---\nnegative:\n  phase: runtime\n  type: Test262Error\nfeatures:\n  - BigInt\n---*/\n";
        let meta = metadata(runtime).unwrap();
        assert!(!meta.negative_parse);
        assert_eq!(meta.features, ["BigInt"]);
        assert!(metadata("no front matter").is_none());
    }

    #[test]
    fn modes_follow_the_flags() {
        let limits = ParseLimits::default();
        let both = Metadata::default();
        assert_eq!(run("var x = 1;", &both, &limits), Outcome::Pass);
        // `with` parses sloppy but not strict.
        assert!(matches!(
            run("with (a) b;", &both, &limits),
            Outcome::UnexpectedFailure("strict", _)
        ));
        let sloppy = Metadata {
            no_strict: true,
            ..Metadata::default()
        };
        assert_eq!(run("with (a) b;", &sloppy, &limits), Outcome::Pass);
        let negative = Metadata {
            negative_parse: true,
            ..Metadata::default()
        };
        assert_eq!(run("var 1;", &negative, &limits), Outcome::Pass);
        assert_eq!(
            run("var x;", &negative, &limits),
            Outcome::UnexpectedSuccess("sloppy")
        );
    }
}
