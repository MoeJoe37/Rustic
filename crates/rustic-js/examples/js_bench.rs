//! Run the JS-09 benchmark corpus (`benchmarks/*.js`) and print one line
//! per workload: median and 95th-percentile time, peak heap, collection
//! count and the total and longest collection pause.
//!
//! ```text
//! cargo run --release -p rustic-js --example js_bench -- [--size N] [--runs K] [name ...]
//! ```
//!
//! Each run uses a fresh agent with the default heap limit. Times come
//! from the host's monotonic clock, so they describe this machine only;
//! the deterministic checks of the corpus are in `tests/limits.rs`.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use rustic_js::runtime::Agent;

fn nanos() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
}

struct Sample {
    nanos: u64,
    peak: usize,
    collections: u64,
    pause: u64,
    longest: u64,
    result: String,
}

fn run_once(source: &str, size: u64) -> Result<Sample, String> {
    let mut agent = Agent::new().map_err(|_| "out of memory".to_owned())?;
    agent.heap.set_gc_clock(Some(nanos));
    agent.evaluate(source).map_err(|e| e.to_string())?;
    let before = agent.heap.collections();
    let start = nanos();
    let value = agent
        .evaluate(&format!("run({size})"))
        .map_err(|e| e.to_string())?;
    let elapsed = nanos().saturating_sub(start);
    Ok(Sample {
        nanos: elapsed,
        peak: agent.heap.peak_bytes(),
        collections: agent.heap.collections() - before,
        pause: agent.heap.gc_pause_nanos(),
        longest: agent.heap.gc_pause_max_nanos(),
        result: agent.display(value),
    })
}

/// The value at quantile `q` of sorted `values` (nearest rank).
fn quantile(values: &[u64], q: f64) -> u64 {
    let rank = (q * values.len() as f64).ceil() as usize;
    values[rank.clamp(1, values.len()) - 1]
}

fn millis(nanos: u64) -> f64 {
    nanos as f64 / 1e6
}

fn main() {
    let mut size = 200_000_u64;
    let mut runs = 21_usize;
    let mut names = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => size = args.next().and_then(|v| v.parse().ok()).unwrap_or(size),
            "--runs" => runs = args.next().and_then(|v| v.parse().ok()).unwrap_or(runs),
            _ => names.push(arg),
        }
    }
    let runs = runs.max(1);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("the benchmark corpus is in the crate")
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "js"))
        .collect();
    files.sort();
    println!("size {size}, {runs} runs each");
    println!(
        "{:<18} {:>10} {:>10} {:>10} {:>5} {:>10} {:>10}  result",
        "workload", "median ms", "p95 ms", "peak KiB", "GCs", "pause ms", "max ms"
    );
    let mut failed = false;
    for path in files {
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        if !names.is_empty() && !names.iter().any(|n| *n == name) {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("a readable workload");
        let mut samples = Vec::with_capacity(runs);
        let mut error = None;
        for _ in 0..runs {
            match run_once(&source, size) {
                Ok(sample) => samples.push(sample),
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        if let Some(error) = error {
            println!("{name:<18} not runnable: {error}");
            failed = true;
            continue;
        }
        let mut times: Vec<u64> = samples.iter().map(|s| s.nanos).collect();
        times.sort_unstable();
        let last = &samples[samples.len() - 1];
        println!(
            "{:<18} {:>10.2} {:>10.2} {:>10} {:>5} {:>10.2} {:>10.2}  {}",
            name,
            millis(quantile(&times, 0.5)),
            millis(quantile(&times, 0.95)),
            last.peak / 1024,
            last.collections,
            millis(last.pause),
            millis(last.longest),
            last.result
        );
    }
    if failed {
        std::process::exit(1);
    }
}
