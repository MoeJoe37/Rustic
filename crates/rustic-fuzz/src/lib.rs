//! Dependency-free deterministic mutation fuzzing for Rustic's parsers.
//!
//! This is a blind (not coverage-guided) mutation fuzzer written in safe,
//! portable Rust using only the standard library, so it builds on every host
//! the engine supports. Each campaign first replays its seed corpus, then
//! applies seeded byte mutations. A target fails when it panics, violates its
//! differential invariant (for example, feed boundaries changing a parse), or
//! exceeds the per-input time limit. The first failure is optionally
//! minimized and saved as a reproducer that replays from its bytes alone.
//!
//! Coverage-guided engines and sanitizers remain candidate tools behind the
//! dependency review in `docs/DEPENDENCIES.md`; nothing here claims to be one.

#![warn(missing_docs)]

mod corpus;
mod formatting;
mod harness;
mod images;
mod js;
mod js_heap;
mod js_run;
mod mutate;
mod paint;
mod png;
mod rng;
mod sfnt;
mod shaping;
mod targets;

pub use corpus::{seed_inputs, workspace_root};
pub use harness::{
    FailureKind, Finding, RunConfig, RunReport, execute, minimize, read_dir_inputs, run,
};
pub use mutate::mutate;
pub use rng::{Rng, fingerprint};
pub use targets::{HtmlDocumentTarget, HtmlTokenizerTarget, Target, UrlTarget, all, by_name};
