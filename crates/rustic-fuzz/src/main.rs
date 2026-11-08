//! Command line for deterministic parser fuzz campaigns and reproducer replay.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use rustic_fuzz::{RunConfig, Target, all, by_name, execute, run, seed_inputs, workspace_root};

const USAGE: &str = "\
usage:
  rustic-fuzz list
  rustic-fuzz run [--target NAME | --all] [--seed N] [--iterations N]
                  [--seconds N] [--timeout-ms N] [--artifacts DIR] [--no-minimize]
  rustic-fuzz replay [--timeout-ms N] TARGET FILE...

run replays every seed input and then executes seeded mutations, stopping at the
first panic, invariant violation or timeout and saving a reproducer. Exit status:
0 clean, 1 failure found, 2 usage or I/O error.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("rustic-fuzz: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(args: &[String]) -> Result<bool, String> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            for target in all() {
                println!("{}", target.name());
            }
            Ok(true)
        }
        Some("run") => campaign(&args[1..]),
        Some("replay") => replay(&args[1..]),
        Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            Ok(true)
        }
        _ => Err("expected a command".into()),
    }
}

fn number(flag: &str, value: Option<&String>) -> Result<u64, String> {
    value
        .ok_or_else(|| format!("{flag} needs a value"))?
        .parse()
        .map_err(|_| format!("{flag} needs a non-negative integer"))
}

fn campaign(args: &[String]) -> Result<bool, String> {
    let mut targets: Vec<Box<dyn Target>> = Vec::new();
    let mut config = RunConfig {
        seed: 0,
        iterations: 10_000,
        max_duration: None,
        timeout: Duration::from_secs(10),
        artifacts: PathBuf::from("fuzz-artifacts"),
        minimize: true,
    };
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--all" => targets = all(),
            "--target" => {
                let name = iter.next().ok_or("--target needs a value")?;
                targets.push(by_name(name).ok_or_else(|| format!("unknown target `{name}`"))?);
            }
            "--seed" => config.seed = number(flag, iter.next())?,
            "--iterations" => config.iterations = number(flag, iter.next())?,
            "--seconds" => {
                config.max_duration = Some(Duration::from_secs(number(flag, iter.next())?));
            }
            "--timeout-ms" => {
                config.timeout = Duration::from_millis(number(flag, iter.next())?.max(1));
            }
            "--artifacts" => {
                config.artifacts = PathBuf::from(iter.next().ok_or("--artifacts needs a value")?);
            }
            "--no-minimize" => config.minimize = false,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    if targets.is_empty() {
        return Err("choose --target NAME or --all".into());
    }
    let root = workspace_root();
    let mut clean = true;
    for target in targets {
        let target: Arc<dyn Target> = Arc::from(target);
        let seeds = seed_inputs(&root, target.name())
            .map_err(|error| format!("reading seeds for {}: {error}", target.name()))?;
        let report =
            run(&target, &seeds, &config).map_err(|error| format!("saving reproducer: {error}"))?;
        println!(
            "{}: seed {} replayed {} seeds and {} mutations in {:.1}s",
            target.name(),
            config.seed,
            report.seeds,
            report.mutations,
            report.elapsed.as_secs_f64()
        );
        if let Some(finding) = report.finding {
            clean = false;
            println!(
                "{}: FAILED at {}: {}\n  reproducer: {}",
                target.name(),
                finding.origin,
                finding.kind,
                finding.reproducer.display()
            );
            if matches!(finding.kind, rustic_fuzz::FailureKind::Timeout(_)) {
                // An abandoned worker may still be running; stop immediately.
                return Ok(false);
            }
        }
    }
    Ok(clean)
}

fn replay(args: &[String]) -> Result<bool, String> {
    let mut timeout = Duration::from_secs(10);
    let mut rest = args;
    if rest.first().map(String::as_str) == Some("--timeout-ms") {
        timeout = Duration::from_millis(number("--timeout-ms", rest.get(1))?.max(1));
        rest = &rest[2..];
    }
    let (name, files) = rest.split_first().ok_or("replay needs a target")?;
    if files.is_empty() {
        return Err("replay needs at least one file".into());
    }
    let target: Arc<dyn Target> =
        Arc::from(by_name(name).ok_or_else(|| format!("unknown target `{name}`"))?);
    let mut clean = true;
    for file in files {
        let input = std::fs::read(file).map_err(|error| format!("reading {file}: {error}"))?;
        match execute(&target, &input, timeout) {
            Ok(()) => println!("{file}: ok"),
            Err(kind) => {
                clean = false;
                println!("{file}: {kind}");
            }
        }
    }
    Ok(clean)
}
