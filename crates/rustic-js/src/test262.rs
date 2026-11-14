//! Parse-phase classification of Test262 files (JS-01 evidence).
//!
//! Test262 (<https://github.com/tc39/test262>) files carry YAML front matter
//! between `/*---` and `---*/`. This module reads the parts that decide how
//! a file is parsed (`flags`) and whether parsing must fail (`negative`
//! with `phase: parse`), and runs the parser in each required mode. It
//! never executes code. [`run_tree`] runs a whole checkout;
//! `tests/test262.rs` compares it with the committed expected failures of
//! the pinned revision.
//!
//! # Execution (JS-07)
//!
//! [`execute`] runs one file the way Test262's `INTERPRETING.md`
//! specifies: a fresh agent per mode, the harness files `assert.js` and
//! `sta.js` plus the file's `includes`, then the test, in sloppy and
//! strict mode unless its flags say otherwise. A `negative` expectation
//! must fail in the named phase with the named error. Every run has a
//! heap limit and a step budget ([`RunLimits`]), so a runaway test fails
//! the same way on every machine instead of timing out. After the test the
//! promise jobs run; an `async` test passes when it prints
//! `Test262:AsyncTestComplete` through `$DONE`. [`execute_tree`] runs script
//! tests from a checkout. [`execute_module_tree`] selects module tests and
//! loads their static dependencies within the checkout.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::module::{GraphError, GraphLimits, ModuleEvalError, ModuleGraph, ModuleRequest};
use crate::runtime::{Agent, PropertyKey, Value};
use crate::{EvalError, ParseError, ParseLimits, Source, parse_module, parse_script};

/// Proposal features the parser does not implement yet (TD-89). Files
/// using them are skipped and counted separately, never passed.
pub const UNSUPPORTED_FEATURES: &[&str] = &[
    "await-dictionary",
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
    /// `flags: [async]`: the test finishes by calling `$DONE`.
    pub is_async: bool,
    /// `includes: [...]`: harness files to load before the test.
    pub includes: Vec<String>,
    /// `negative:`: the phase that must fail and the error's name.
    pub negative: Option<Negative>,
    /// `flags: [CanBlockIsFalse]`: the agent may not block in
    /// `Atomics.wait`.
    pub cannot_block: bool,
}

/// When a negative test must fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// While parsing (early errors included).
    Parse,
    /// While resolving module imports.
    Resolution,
    /// While running.
    Runtime,
}

/// A `negative:` expectation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negative {
    /// The phase that must fail.
    pub phase: Phase,
    /// The name of the error's constructor, such as `SyntaxError`.
    pub error: String,
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
    let mut in_includes_block = false;
    let (mut phase, mut error) = (None, None);
    for line in yaml.lines() {
        let trimmed = line.trim();
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            in_negative = trimmed.starts_with("negative:");
            in_features_block = false;
            in_includes_block = false;
        }
        if let Some(rest) = trimmed.strip_prefix("flags:") {
            for flag in list_items(rest) {
                match flag {
                    "module" => meta.module = true,
                    "onlyStrict" => meta.only_strict = true,
                    "noStrict" => meta.no_strict = true,
                    "raw" => meta.raw = true,
                    "async" => meta.is_async = true,
                    "CanBlockIsFalse" => meta.cannot_block = true,
                    _ => {}
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("features:") {
            meta.features.extend(list_items(rest).map(str::to_owned));
            in_features_block = rest.trim().is_empty();
        } else if in_features_block && trimmed.starts_with("- ") {
            meta.features.push(trimmed[2..].trim().to_owned());
        } else if let Some(rest) = trimmed.strip_prefix("includes:") {
            meta.includes.extend(list_items(rest).map(str::to_owned));
            in_includes_block = rest.trim().is_empty();
        } else if in_includes_block && trimmed.starts_with("- ") {
            meta.includes.push(trimmed[2..].trim().to_owned());
        } else if in_negative && trimmed.starts_with("phase:") {
            phase = match trimmed["phase:".len()..].trim() {
                "parse" => Some(Phase::Parse),
                "resolution" => Some(Phase::Resolution),
                "runtime" => Some(Phase::Runtime),
                _ => None,
            };
            meta.negative_parse = phase == Some(Phase::Parse);
        } else if in_negative && trimmed.starts_with("type:") {
            error = Some(trimmed["type:".len()..].trim().to_owned());
        }
    }
    if let (Some(phase), Some(error)) = (phase, error) {
        meta.negative = Some(Negative { phase, error });
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

/// The harness files of a Test262 checkout, by file name.
#[derive(Clone, Debug, Default)]
pub struct Harness {
    files: std::collections::BTreeMap<String, String>,
}

impl Harness {
    /// Read `harness/*.js` of the checkout at `root`.
    pub fn load(root: &Path) -> io::Result<Self> {
        let mut files = std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(root.join("harness"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "js") {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                files.insert(name, std::fs::read_to_string(&path)?);
            }
        }
        Ok(Self { files })
    }

    /// A harness made of the given `(name, source)` files.
    pub fn from_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            files: files
                .into_iter()
                .map(|(name, text)| (name.to_owned(), text.to_owned()))
                .collect(),
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.files.get(name).map(String::as_str)
    }
}

/// What one test may use.
#[derive(Clone, Copy, Debug)]
pub struct RunLimits {
    /// The agent's heap, in bytes.
    pub heap_bytes: usize,
    /// Safe points (loop iterations and calls) the harness and test may
    /// pass together before the run is cancelled.
    pub steps: u64,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            heap_bytes: 64 << 20,
            steps: 5_000_000,
        }
    }
}

/// The result of running one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every mode behaved as the file expects.
    Pass,
    /// A mode did not; with the mode and what happened.
    Fail(String),
    /// The file was not run; with the reason.
    Skip(&'static str),
}

/// Why a file cannot be run yet, if it cannot.
pub fn skip_reason(meta: &Metadata) -> Option<&'static str> {
    if meta.unsupported() {
        Some("proposal")
    } else if meta.module {
        Some("module")
    } else {
        None
    }
}

/// Run `text` in every mode its metadata requires.
pub fn execute(text: &str, meta: &Metadata, harness: &Harness, limits: RunLimits) -> Verdict {
    if let Some(reason) = skip_reason(meta) {
        return Verdict::Skip(reason);
    }
    let mut modes: Vec<(&'static str, bool)> = Vec::new();
    if meta.raw {
        modes.push(("raw", false));
    } else {
        if !meta.only_strict {
            modes.push(("sloppy", false));
        }
        if !meta.no_strict {
            modes.push(("strict", true));
        }
    }
    for (mode, strict) in modes {
        let run = std::panic::catch_unwind(|| {
            execute_mode(text, meta, harness, limits, TestMode::Script { strict })
        });
        match run {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => return Verdict::Fail(format!("{mode}: {reason}")),
            Err(_) => return Verdict::Fail(format!("{mode}: the engine panicked")),
        }
    }
    Verdict::Pass
}

#[derive(Clone, Copy)]
enum TestMode<'a> {
    Script { strict: bool },
    Module { path: &'a Path, root: &'a Path },
}

#[derive(Debug)]
enum TestError {
    Runtime(EvalError),
    Resolution(String),
    Host(String),
}

impl From<EvalError> for TestError {
    fn from(error: EvalError) -> Self {
        Self::Runtime(error)
    }
}

fn execute_module(
    agent: &mut Agent,
    text: &str,
    path: &Path,
    root: &Path,
) -> Result<Value, TestError> {
    let limits = GraphLimits::default();
    let parse_source = |text: &str| {
        Source::from_str_limited(text, limits.max_source_units).map_err(|_| "module is too large")
    };
    let entry = parse_source(text).map_err(|error| TestError::Host(error.to_owned()))?;
    // Distinguish a syntax error in the test itself from a failed import.
    parse_module(&entry, &limits.parse).map_err(EvalError::Parse)?;
    let graph = ModuleGraph::load(
        path.to_owned(),
        &limits,
        |referrer, request| resolve_fixture(root, referrer, request),
        |key| {
            if key == path {
                return Ok(entry.clone());
            }
            // Graph limits count code units. Each code unit takes at most
            // three UTF-8 bytes in a well-formed file.
            let max_bytes = 3 * limits.max_source_units;
            let mut source = String::new();
            std::fs::File::open(key)
                .and_then(|file| file.take(max_bytes as u64 + 1).read_to_string(&mut source))
                .map_err(|error| format!("read module fixture: {error}"))?;
            if source.len() > max_bytes {
                return Err("module is too large".into());
            }
            parse_source(&source).map_err(str::to_owned)
        },
    )
    .map_err(|error| match error {
        GraphError::UnresolvedExport { .. } | GraphError::Parse(ParseError::Syntax(_)) => {
            TestError::Resolution(format!("{error:?}"))
        }
        other => TestError::Host(format!("{other:?}")),
    })?;
    agent
        .evaluate_module_graph(&graph, &limits)
        .map(Value::object)
        .map_err(|error| match error {
            ModuleEvalError::Runtime(abrupt) => TestError::Runtime(abrupt.into()),
            other => TestError::Host(other.to_string()),
        })
}

fn resolve_fixture(
    root: &Path,
    referrer: &Path,
    request: &ModuleRequest,
) -> Result<PathBuf, String> {
    if !request.attributes().is_empty() {
        return Err("this Test262 host loads source text module fixtures".into());
    }
    let specifier = String::from_utf16(request.specifier())
        .map_err(|_| "module specifier is not valid UTF-16")?;
    let relative = specifier
        .strip_prefix("./")
        .ok_or("expected a relative module specifier")?;
    let relative = Path::new(relative);
    if relative
        .extension()
        .is_none_or(|extension| extension != "js")
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("expected a JavaScript fixture inside the checkout".into());
    }
    let path = referrer
        .parent()
        .ok_or("module has no containing directory")?
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("locate module fixture: {error}"))?;
    if path.strip_prefix(root).is_err() || !path.is_file() {
        return Err("module fixture is outside the checkout or is not a regular file".into());
    }
    Ok(path)
}

fn execute_mode(
    text: &str,
    meta: &Metadata,
    harness: &Harness,
    limits: RunLimits,
    mode: TestMode<'_>,
) -> Result<(), String> {
    let mut agent =
        Agent::with_heap_limit(limits.heap_bytes).map_err(|_| "no memory for an agent")?;
    agent.set_step_budget(Some(limits.steps));
    // The main agent of a Test262 host may block in `Atomics.wait`
    // unless the test asks for one that may not.
    agent.set_can_block(!meta.cannot_block);
    install_host_object(&mut agent).map_err(|_| "no memory for $262")?;
    if meta.is_async {
        // `doneprintHandle.js` reports through the host's `print`; keep
        // what it prints on the function itself.
        // A string, not an array: tests may put setters on
        // `Array.prototype`.
        let print = "var print = function (message) { print.out += String(message) + '\\n' }; \
             print.out = '';";
        if let Err(error) = agent.evaluate(print) {
            return Err(format!("print: {}", describe(&mut agent, &error)));
        }
    }
    if !meta.raw {
        let done = meta.is_async.then_some("doneprintHandle.js");
        let names = ["assert.js", "sta.js"]
            .into_iter()
            .chain(done)
            .chain(meta.includes.iter().map(String::as_str));
        for name in names {
            let source = harness
                .get(name)
                .ok_or_else(|| format!("harness file {name} is missing"))?;
            if let Err(error) = agent.evaluate(source) {
                return Err(format!("harness {name}: {}", describe(&mut agent, &error)));
            }
        }
    }
    let result = match mode {
        TestMode::Script { strict: true } => agent
            .evaluate(&format!("\"use strict\";\n{text}"))
            .map_err(TestError::from),
        TestMode::Script { strict: false } => agent.evaluate(text).map_err(TestError::from),
        TestMode::Module { path, root } => execute_module(&mut agent, text, path, root),
    };
    // A microtask checkpoint after evaluation, as a host runs one.
    let result = result.and_then(|v| {
        settle(&mut agent)
            .map(|()| v)
            .map_err(|error| TestError::Runtime(error.into()))
    });
    let reason = |agent: &mut Agent, error: &TestError| match error {
        TestError::Runtime(error) => describe(agent, error),
        TestError::Resolution(error) | TestError::Host(error) => error.clone(),
    };
    match (&meta.negative, result) {
        (None, Ok(_)) if meta.is_async => async_outcome(&mut agent),
        (None, Ok(_)) => Ok(()),
        (None, Err(error)) => Err(reason(&mut agent, &error)),
        (Some(negative), Ok(_)) => Err(format!("ran, but must throw {}", negative.error)),
        (Some(negative), Err(error)) => {
            let (phase, name) = match &error {
                TestError::Runtime(EvalError::Parse(ParseError::Syntax(_))) => {
                    (Phase::Parse, "SyntaxError".to_owned())
                }
                TestError::Runtime(EvalError::Throw(value)) => {
                    (Phase::Runtime, error_name(&mut agent, *value))
                }
                TestError::Resolution(_) => (Phase::Resolution, "SyntaxError".to_owned()),
                _ => return Err(reason(&mut agent, &error)),
            };
            if phase == negative.phase && name == negative.error {
                Ok(())
            } else {
                Err(format!(
                    "{}, but must throw {} in the {:?} phase",
                    reason(&mut agent, &error),
                    negative.error,
                    negative.phase
                ))
            }
        }
    }
}

/// Run the promise jobs, then let time pass for each pending
/// `Atomics.waitAsync` timeout in turn, as a host's timer tasks would.
fn settle(agent: &mut Agent) -> crate::runtime::JsResult<()> {
    agent.run_jobs()?;
    while let Some(deadline) = agent.next_waiter_deadline() {
        agent.expire_waiters(deadline)?;
        agent.run_jobs()?;
    }
    Ok(())
}

/// The part of the `$262` host object (INTERPRETING.md) the engine can
/// provide so far: `global` and `detachArrayBuffer`. Realms, `evalScript`,
/// `gc` and agents are missing (TD-105), so tests that use them fail.
fn install_host_object(agent: &mut Agent) -> crate::runtime::JsResult<()> {
    fn detach(agent: &mut Agent, _: Value, args: &[Value]) -> crate::runtime::JsResult<Value> {
        let detached = args
            .first()
            .and_then(|v| v.as_object())
            .is_some_and(|buffer| agent.detach_array_buffer(buffer));
        if !detached {
            return agent.type_error("$262.detachArrayBuffer: not an ArrayBuffer");
        }
        Ok(Value::NULL)
    }
    let host = agent.new_object()?;
    let key = agent.heap.key("detachArrayBuffer")?;
    agent.define_method(host, key, detach, "detachArrayBuffer", 1)?;
    let global = agent.intrinsics.global;
    let key = agent.heap.key("global")?;
    agent.create_data_property_or_throw(host, key, Value::object(global))?;
    let key = agent.heap.key("$262")?;
    agent.define_property_or_throw(
        global,
        key,
        crate::runtime::PropertyDescriptor::data_with(Value::object(host), true, false, true),
    )
}

/// What an async test printed through `$DONE`.
fn async_outcome(agent: &mut Agent) -> Result<(), String> {
    let printed = match agent.evaluate("print.out") {
        Ok(v) => agent.display(v),
        Err(error) => return Err(describe(agent, &error)),
    };
    if printed.lines().any(|l| l == "Test262:AsyncTestComplete") {
        return Ok(());
    }
    match printed
        .lines()
        .find_map(|l| l.strip_prefix("Test262:AsyncTestFailure:"))
    {
        Some(failure) => Err(format!("async failure: {failure}")),
        None => Err("the async test never called $DONE".to_owned()),
    }
}

/// The name of a thrown value's constructor, or of the error itself.
fn error_name(agent: &mut Agent, value: Value) -> String {
    let name_of = |agent: &mut Agent, holder: Value| -> Option<String> {
        let key = PropertyKey::String(agent.names.name);
        let name = agent.get_v(holder, key).ok()?;
        let id = name.as_string()?;
        Some(agent.heap.string(id).to_text())
    };
    if value.as_object().is_none() {
        return String::new();
    }
    let constructor = agent
        .heap
        .intern_str("constructor")
        .ok()
        .and_then(|key| agent.get_v(value, PropertyKey::String(key)).ok())
        .filter(|c| c.as_object().is_some());
    constructor
        .and_then(|c| name_of(agent, c))
        .or_else(|| name_of(agent, value))
        .unwrap_or_default()
}

fn describe(agent: &mut Agent, error: &EvalError) -> String {
    let text = match error {
        EvalError::Throw(value) => {
            let text = agent.to_string(*value).map_or_else(
                |_| "<unprintable>".to_owned(),
                |s| agent.heap.string(s).to_text(),
            );
            format!("threw {text}")
        }
        EvalError::Parse(ParseError::Syntax(e)) => format!("SyntaxError: {e}"),
        EvalError::Cancelled => "ran out of steps".to_owned(),
        other => other.to_string(),
    };
    // One line, bounded: reasons go into reports.
    let line = text.lines().next().unwrap_or_default();
    line.chars().take(160).collect()
}

/// Totals of [`execute_tree`].
#[derive(Clone, Debug, Default)]
pub struct ExecutionReport {
    /// Paths (relative to the checkout, `/` separators) that passed,
    /// sorted.
    pub passed: Vec<String>,
    /// Failing paths and their reasons, sorted by path.
    pub failed: Vec<(String, String)>,
    /// Paths that were not run and why, sorted by path.
    pub skipped: Vec<(String, &'static str)>,
}

/// Run every test file under `directory` (relative to the checkout at
/// `root`, such as `test/language`), in path order.
pub fn execute_tree(
    root: &Path,
    directory: &str,
    limits: RunLimits,
) -> io::Result<ExecutionReport> {
    execute_tree_with(root, directory, limits, false)
}

/// Run only tests flagged `module`, with a local static-module host.
pub fn execute_module_tree(
    root: &Path,
    directory: &str,
    limits: RunLimits,
) -> io::Result<ExecutionReport> {
    execute_tree_with(root, directory, limits, true)
}

fn execute_tree_with(
    root: &Path,
    directory: &str,
    limits: RunLimits,
    modules_only: bool,
) -> io::Result<ExecutionReport> {
    let root = root.canonicalize()?;
    let harness = Harness::load(&root)?;
    let mut files = Vec::new();
    walk(&root.join(directory), &mut files)?;
    let mut report = ExecutionReport::default();
    for path in &files {
        let canonical = if modules_only {
            let canonical = path.canonicalize()?;
            if canonical.strip_prefix(&root).is_err() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "module test is outside the checkout",
                ));
            }
            Some(canonical)
        } else {
            None
        };
        let text = std::fs::read_to_string(canonical.as_ref().unwrap_or(path))?;
        let Some(meta) = metadata(&text) else {
            continue;
        };
        if modules_only && !meta.module {
            continue;
        }
        let relative: Vec<String> = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let relative = relative.join("/");
        let verdict = if !modules_only {
            execute(&text, &meta, &harness, limits)
        } else if meta.unsupported() {
            Verdict::Skip("proposal")
        } else {
            let mode = TestMode::Module {
                path: canonical
                    .as_deref()
                    .expect("module paths were canonicalized"),
                root: &root,
            };
            match std::panic::catch_unwind(|| execute_mode(&text, &meta, &harness, limits, mode)) {
                Ok(Ok(())) => Verdict::Pass,
                Ok(Err(reason)) => Verdict::Fail(format!("module: {reason}")),
                Err(_) => Verdict::Fail("module: the engine panicked".into()),
            }
        };
        match verdict {
            Verdict::Pass => report.passed.push(relative),
            Verdict::Fail(reason) => report.failed.push((relative, reason)),
            Verdict::Skip(reason) => report.skipped.push((relative, reason)),
        }
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

    const HARNESS: &[(&str, &str)] = &[
        (
            "assert.js",
            "var assert = { sameValue: function (a, b) { if (a !== b) throw new Test262Error(a + ' !== ' + b) } };",
        ),
        (
            "sta.js",
            "function Test262Error(message) { this.message = message } \
             Test262Error.prototype.toString = function () { return 'Test262Error: ' + this.message };",
        ),
        ("helper.js", "var helped = 7;"),
        (
            "doneprintHandle.js",
            "function $DONE(error) { print(error ? 'Test262:AsyncTestFailure:' + error : 'Test262:AsyncTestComplete') }",
        ),
    ];

    fn verdict(text: &str) -> Verdict {
        let meta = metadata(text).expect("front matter");
        let harness = Harness::from_files(HARNESS.iter().copied());
        let limits = RunLimits {
            heap_bytes: 8 << 20,
            steps: 10_000,
        };
        execute(text, &meta, &harness, limits)
    }

    #[test]
    fn execution_follows_the_metadata() {
        assert_eq!(
            verdict("/*---\n---*/\nassert.sameValue(1 + 1, 2);"),
            Verdict::Pass
        );
        assert_eq!(
            verdict("/*---\n---*/\nassert.sameValue(1, 2);"),
            Verdict::Fail("sloppy: threw Test262Error: 1 !== 2".into())
        );
        // Both modes run: `this` is the global object only in sloppy code.
        assert_eq!(
            verdict("/*---\n---*/\nassert.sameValue((function () { return this })(), undefined);"),
            Verdict::Fail("sloppy: threw Test262Error: [object Object] !== undefined".into())
        );
        assert_eq!(
            verdict(
                "/*---\nflags: [onlyStrict]\n---*/\nassert.sameValue((function () { return this })(), undefined);"
            ),
            Verdict::Pass
        );
        assert!(matches!(
            verdict("/*---\nflags: [noStrict]\n---*/\nwith ({}) {}"),
            Verdict::Pass
        ));
        // Includes are loaded; a missing one fails the test.
        assert_eq!(
            verdict("/*---\nincludes: [helper.js]\n---*/\nassert.sameValue(helped, 7);"),
            Verdict::Pass
        );
        assert_eq!(
            verdict("/*---\nincludes:\n  - nope.js\n---*/\n1;"),
            Verdict::Fail("sloppy: harness file nope.js is missing".into())
        );
        // Raw tests run without the harness.
        assert!(matches!(
            verdict("/*---\nflags: [raw]\n---*/\nassert.sameValue(1, 1);"),
            Verdict::Fail(reason) if reason.starts_with("raw: threw ReferenceError")
        ));
    }

    #[test]
    fn negative_tests_need_the_named_error_in_the_named_phase() {
        assert_eq!(
            verdict("/*---\nnegative:\n  phase: parse\n  type: SyntaxError\n---*/\nvar 1;"),
            Verdict::Pass
        );
        assert!(matches!(
            verdict("/*---\nnegative:\n  phase: parse\n  type: SyntaxError\n---*/\nvar x;"),
            Verdict::Fail(reason) if reason.contains("must throw SyntaxError")
        ));
        assert_eq!(
            verdict(
                "/*---\nnegative:\n  phase: runtime\n  type: Test262Error\n---*/\nthrow new Test262Error('x');"
            ),
            Verdict::Pass
        );
        assert_eq!(
            verdict("/*---\nnegative:\n  phase: runtime\n  type: TypeError\n---*/\nnull.x;"),
            Verdict::Pass
        );
        assert!(matches!(
            verdict("/*---\nnegative:\n  phase: runtime\n  type: RangeError\n---*/\nnull.x;"),
            Verdict::Fail(reason) if reason.contains("must throw RangeError in the Runtime phase")
        ));
        // A parse error is not a runtime error.
        assert!(matches!(
            verdict("/*---\nnegative:\n  phase: runtime\n  type: SyntaxError\n---*/\nvar 1;"),
            Verdict::Fail(_)
        ));
    }

    #[test]
    fn limits_and_skips_are_reported() {
        assert_eq!(
            verdict("/*---\n---*/\nfor (;;) {}"),
            Verdict::Fail("sloppy: ran out of steps".into())
        );
        // The heap limit ends a test that allocates without bound.
        assert_eq!(
            verdict("/*---\n---*/\nvar s = 'x'; for (var i = 0; i < 60; i++) s += s;"),
            Verdict::Fail("sloppy: out of memory".into())
        );
        assert_eq!(
            verdict("/*---\nflags: [module]\n---*/\n1;"),
            Verdict::Skip("module")
        );
        // Async tests finish through `$DONE` once the jobs have run.
        assert_eq!(
            verdict(
                "/*---\nflags: [async]\n---*/\nPromise.resolve(1).then(function () { $DONE() });"
            ),
            Verdict::Pass
        );
        assert_eq!(
            verdict("/*---\nflags: [async]\n---*/\nPromise.reject('no').catch($DONE);"),
            Verdict::Fail("sloppy: async failure: no".into())
        );
        assert_eq!(
            verdict("/*---\nflags: [async]\n---*/\n1;"),
            Verdict::Fail("sloppy: the async test never called $DONE".into())
        );
        assert_eq!(
            verdict("/*---\nfeatures: [decorators]\n---*/\n1;"),
            Verdict::Skip("proposal")
        );
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
