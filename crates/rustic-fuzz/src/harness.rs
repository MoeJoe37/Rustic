//! Isolated execution, the mutation loop, minimization and reproducers.

use std::any::Any;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::{Rng, Target, fingerprint, mutate};

/// Stack for each isolated execution; parsers are iterative, so this is ample.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound on candidate executions spent shrinking one failure.
const MAX_MINIMIZE_STEPS: usize = 4096;

/// How one execution failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FailureKind {
    /// The target panicked; carries the panic message when it is a string.
    Panic(String),
    /// The target returned an invariant violation.
    Violation(String),
    /// The target did not finish within the per-input time limit.
    Timeout(Duration),
}

impl FailureKind {
    /// Short label used in reproducer file names.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Panic(_) => "panic",
            Self::Violation(_) => "violation",
            Self::Timeout(_) => "timeout",
        }
    }

    fn same_class(&self, other: &Self) -> bool {
        self.label() == other.label()
    }
}

impl fmt::Display for FailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Panic(message) => write!(f, "panic: {message}"),
            Self::Violation(message) => write!(f, "invariant violation: {message}"),
            Self::Timeout(limit) => write!(f, "no result within {} ms", limit.as_millis()),
        }
    }
}

/// Run one input on a fresh worker thread, bounded by `timeout`.
///
/// A panic is caught at the thread boundary. Safe Rust cannot stop a running
/// thread, so after a timeout the worker is abandoned; callers report the
/// failure and exit, and CI's job timeout remains the outer bound.
pub fn execute(
    target: &Arc<dyn Target>,
    input: &[u8],
    timeout: Duration,
) -> Result<(), FailureKind> {
    let (sender, receiver) = mpsc::channel();
    let worker_target = Arc::clone(target);
    let worker_input = input.to_vec();
    let worker = thread::Builder::new()
        .name(format!("fuzz-{}", target.name()))
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            let result = worker_target.check(&worker_input);
            // The receiver may be gone after a timeout; nothing else to do.
            let _ = sender.send(result);
        })
        .map_err(|error| FailureKind::Panic(format!("could not start worker: {error}")))?;
    match receiver.recv_timeout(timeout) {
        Ok(result) => {
            let _ = worker.join();
            result.map_err(FailureKind::Violation)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
            Err(payload) => Err(FailureKind::Panic(panic_message(payload.as_ref()))),
            Ok(()) => Err(FailureKind::Panic("worker exited without a result".into())),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => Err(FailureKind::Timeout(timeout)),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".into()
    }
}

/// Shrink a deterministic failure by deleting chunks while it still fails
/// in the same class. Timeouts are returned unchanged: re-running a hang
/// repeatedly would multiply the abandoned workers.
pub fn minimize(
    target: &Arc<dyn Target>,
    input: &[u8],
    failure: &FailureKind,
    timeout: Duration,
) -> Vec<u8> {
    if matches!(failure, FailureKind::Timeout(_)) {
        return input.to_vec();
    }
    let mut best = input.to_vec();
    let mut steps = 0;
    let mut chunk = best.len().div_ceil(2).max(1);
    while steps < MAX_MINIMIZE_STEPS {
        let mut start = 0;
        let mut shrunk = false;
        while start < best.len() && steps < MAX_MINIMIZE_STEPS {
            let end = (start + chunk).min(best.len());
            let mut candidate = best[..start].to_vec();
            candidate.extend_from_slice(&best[end..]);
            steps += 1;
            match execute(target, &candidate, timeout) {
                Err(kind) if kind.same_class(failure) => {
                    best = candidate;
                    shrunk = true;
                }
                _ => start = end,
            }
        }
        if chunk == 1 && !shrunk {
            break;
        }
        if !shrunk {
            chunk = chunk.div_ceil(2).max(1);
        }
    }
    best
}

/// Parameters for one deterministic campaign against one target.
#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Generator seed; print it with the target to reproduce a run.
    pub seed: u64,
    /// Mutated executions after every seed input has been replayed.
    pub iterations: u64,
    /// Optional wall-clock cap for the mutation phase.
    pub max_duration: Option<Duration>,
    /// Per-input time limit.
    pub timeout: Duration,
    /// Directory receiving `<target>/<kind>-<fingerprint>.bin` reproducers.
    pub artifacts: PathBuf,
    /// Shrink deterministic failures before saving them.
    pub minimize: bool,
}

/// A failure found by a campaign and the reproducer saved for it.
#[derive(Debug)]
pub struct Finding {
    /// What went wrong.
    pub kind: FailureKind,
    /// Seed replay index or mutation iteration that found it.
    pub origin: String,
    /// Saved (possibly minimized) reproducer.
    pub reproducer: PathBuf,
}

/// Totals for a finished campaign.
#[derive(Debug, Default)]
pub struct RunReport {
    /// Seed inputs replayed before mutation.
    pub seeds: usize,
    /// Mutated inputs executed.
    pub mutations: u64,
    /// The first failure, if any; campaigns stop at the first failure.
    pub finding: Option<Finding>,
    /// Wall-clock duration of the whole campaign.
    pub elapsed: Duration,
}

/// Replay every seed, then mutate them; stop at and save the first failure.
pub fn run(
    target: &Arc<dyn Target>,
    seeds: &[Vec<u8>],
    config: &RunConfig,
) -> io::Result<RunReport> {
    let started = Instant::now();
    let mut report = RunReport::default();
    for (index, seed) in seeds.iter().enumerate() {
        report.seeds += 1;
        if let Err(kind) = execute(target, seed, config.timeout) {
            report.finding = Some(save(target, seed, kind, format!("seed #{index}"), config)?);
            report.elapsed = started.elapsed();
            return Ok(report);
        }
    }
    let empty = [Vec::new()];
    let pool = if seeds.is_empty() { &empty[..] } else { seeds };
    let mut rng = Rng::new(config.seed);
    for iteration in 0..config.iterations {
        if config
            .max_duration
            .is_some_and(|limit| started.elapsed() >= limit)
        {
            break;
        }
        let base = &pool[rng.below(pool.len())];
        let other = &pool[rng.below(pool.len())];
        let input = mutate(&mut rng, base, other, target.dictionary(), target.max_len());
        report.mutations += 1;
        if let Err(kind) = execute(target, &input, config.timeout) {
            let origin = format!("seed {} iteration {iteration}", config.seed);
            report.finding = Some(save(target, &input, kind, origin, config)?);
            break;
        }
    }
    report.elapsed = started.elapsed();
    Ok(report)
}

fn save(
    target: &Arc<dyn Target>,
    input: &[u8],
    kind: FailureKind,
    origin: String,
    config: &RunConfig,
) -> io::Result<Finding> {
    let input = if config.minimize {
        minimize(target, input, &kind, config.timeout)
    } else {
        input.to_vec()
    };
    let directory = config.artifacts.join(target.name());
    fs::create_dir_all(&directory)?;
    let stem = format!("{}-{:016x}", kind.label(), fingerprint(&input));
    let reproducer = directory.join(format!("{stem}.bin"));
    fs::write(&reproducer, &input)?;
    let note = format!(
        "target: {}\norigin: {origin}\nfailure: {kind}\nbytes: {}\nreplay: cargo run --locked --release -p rustic-fuzz -- replay {} {}\n",
        target.name(),
        input.len(),
        target.name(),
        reproducer.display()
    );
    fs::write(directory.join(format!("{stem}.txt")), note)?;
    Ok(Finding {
        kind,
        origin,
        reproducer,
    })
}

/// Read every regular file in `directory` (sorted by name) as one input.
/// A missing directory is an empty corpus.
pub fn read_dir_inputs(directory: &Path) -> io::Result<Vec<(PathBuf, Vec<u8>)>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let path = entry.path();
            // Notes beside reproducers describe them; they are not inputs.
            if path
                .extension()
                .is_some_and(|ext| ext == "txt" || ext == "md")
            {
                continue;
            }
            paths.push(path);
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| fs::read(&path).map(|bytes| (path, bytes)))
        .collect()
}
