//! Harness behavior with injected faulty targets, and seed corpus replay.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rustic_fuzz::{
    FailureKind, RunConfig, Target, all, execute, minimize, read_dir_inputs, run, seed_inputs,
    workspace_root,
};

/// Fails (in the configured way) whenever the input contains `needle`.
struct Faulty {
    needle: &'static [u8],
    mode: Mode,
}

#[derive(Clone, Copy)]
enum Mode {
    Panic,
    Violation,
    Hang,
}

impl Target for Faulty {
    fn name(&self) -> &'static str {
        "faulty"
    }

    fn max_len(&self) -> usize {
        64
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[b"BUG"]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        if !input.windows(self.needle.len()).any(|w| w == self.needle) {
            return Ok(());
        }
        match self.mode {
            Mode::Panic => panic!("found the bug"),
            Mode::Violation => Err("needle present".into()),
            Mode::Hang => {
                std::thread::sleep(Duration::from_secs(2));
                Ok(())
            }
        }
    }
}

fn faulty(mode: Mode) -> Arc<dyn Target> {
    Arc::new(Faulty {
        needle: b"BUG",
        mode,
    })
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustic-fuzz-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn config(artifacts: PathBuf) -> RunConfig {
    RunConfig {
        seed: 9,
        iterations: 20_000,
        max_duration: None,
        timeout: Duration::from_secs(5),
        artifacts,
        minimize: true,
    }
}

#[test]
fn execute_classifies_pass_panic_violation_and_timeout() {
    let short = Duration::from_secs(5);
    assert_eq!(execute(&faulty(Mode::Panic), b"fine", short), Ok(()));
    assert_eq!(
        execute(&faulty(Mode::Panic), b"xBUGx", short),
        Err(FailureKind::Panic("found the bug".into()))
    );
    assert_eq!(
        execute(&faulty(Mode::Violation), b"BUG", short),
        Err(FailureKind::Violation("needle present".into()))
    );
    let limit = Duration::from_millis(50);
    assert_eq!(
        execute(&faulty(Mode::Hang), b"BUG", limit),
        Err(FailureKind::Timeout(limit))
    );
}

#[test]
fn minimization_keeps_the_failure_and_drops_unrelated_bytes() {
    let input = b"<p>lots of unrelated text BUG and more trailing text</p>";
    let failure = FailureKind::Violation("needle present".into());
    let target = faulty(Mode::Violation);
    let small = minimize(&target, input, &failure, Duration::from_secs(5));
    assert_eq!(small, b"BUG");
    // Timeouts are never re-run during minimization.
    let hang = FailureKind::Timeout(Duration::from_millis(1));
    assert_eq!(
        minimize(&faulty(Mode::Hang), input, &hang, Duration::from_millis(1)),
        input
    );
}

#[test]
fn campaign_finds_mutated_failure_and_saves_a_replayable_reproducer() {
    let artifacts = scratch("campaign");
    let target = faulty(Mode::Panic);
    let report = run(
        &target,
        &[b"BU".to_vec(), b"G!".to_vec()],
        &config(artifacts.clone()),
    )
    .expect("artifacts are writable");
    let finding = report
        .finding
        .expect("dictionary mutation inserts the needle");
    assert!(finding.origin.starts_with("seed 9 iteration "));
    assert_eq!(fs::read(&finding.reproducer).unwrap(), b"BUG");
    assert_eq!(
        execute(
            &target,
            &fs::read(&finding.reproducer).unwrap(),
            Duration::from_secs(5)
        ),
        Err(FailureKind::Panic("found the bug".into()))
    );
    let note = fs::read_to_string(finding.reproducer.with_extension("txt")).unwrap();
    assert!(note.contains("replay: cargo run --locked --release -p rustic-fuzz -- replay faulty"));
    // Notes are skipped when the directory is read back as a corpus.
    let saved = read_dir_inputs(&artifacts.join("faulty")).unwrap();
    assert_eq!(saved.len(), 1);
    let _ = fs::remove_dir_all(artifacts);
}

#[test]
fn failing_seed_stops_before_mutation_and_same_seed_repeats_exactly() {
    let artifacts = scratch("seed");
    let target = faulty(Mode::Violation);
    let report = run(
        &target,
        &[b"ok".to_vec(), b"BUG".to_vec()],
        &config(artifacts.clone()),
    )
    .unwrap();
    assert_eq!(report.seeds, 2);
    assert_eq!(report.mutations, 0);
    assert_eq!(report.finding.unwrap().origin, "seed #1");

    let clean: Arc<dyn Target> = Arc::new(Faulty {
        needle: b"never-generated-needle",
        mode: Mode::Panic,
    });
    let mut short = config(artifacts.clone());
    short.iterations = 300;
    let first = run(&clean, &[b"<p>".to_vec()], &short).unwrap();
    assert!(first.finding.is_none());
    assert_eq!(first.mutations, 300);
    let _ = fs::remove_dir_all(artifacts);
}

#[test]
fn every_target_replays_its_seed_corpus_cleanly() {
    let root = workspace_root();
    for target in all() {
        let target: Arc<dyn Target> = Arc::from(target);
        let seeds = seed_inputs(&root, target.name()).expect("seed corpus is readable");
        assert!(
            seeds.len() > 100,
            "{} has only {} seeds",
            target.name(),
            seeds.len()
        );
        for (index, seed) in seeds.iter().enumerate() {
            if let Err(kind) = execute(&target, seed, Duration::from_secs(60)) {
                panic!("{} seed #{index} failed: {kind}", target.name());
            }
        }
    }
}
