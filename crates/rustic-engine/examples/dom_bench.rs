//! Run the PERF-01 DOM microbenchmarks in a scripted document and print
//! `sample <workload> <ms> ok` for every run, like `js_bench --samples`.
//!
//! ```text
//! cargo run --release -p rustic-engine --example dom_bench -- [--size N] [--runs K]
//! ```
//!
//! Each run parses a fresh document, defines the workload, then times one
//! script turn (including its microtask checkpoint) with the host clock.
//! The workload checks its own result and throws on a mismatch. JavaScript
//! shells have no DOM, so these have no cross-engine comparison yet.

use std::time::Instant;

use rustic_engine::{Engine, ScriptTurn, TaskLimits};
use rustic_html::ParserLimits;
use rustic_js::Source;

/// `(name, workload)`: each defines `run(n)` and returns nothing; it throws
/// if its checksum is wrong.
const WORKLOADS: [(&str, &str); 4] = [
    (
        "dom_create_append",
        "function run(n) {
           const list = document.createElement('ul');
           for (let i = 0; i < n; i++) {
             const item = document.createElement('li');
             item.appendChild(document.createTextNode('item ' + i));
             list.appendChild(item);
           }
           document.body.appendChild(list);
           if (list.childNodes.length !== n) throw 'count';
         }",
    ),
    (
        "dom_query",
        "function run(n) {
           const root = document.createElement('div');
           for (let i = 0; i < n; i++) {
             const child = document.createElement('span');
             child.setAttribute('class', 'c' + (i % 8));
             root.appendChild(child);
           }
           document.body.appendChild(root);
           let found = 0;
           for (let i = 0; i < 64; i++) found += root.querySelectorAll('.c' + (i % 8)).length;
           if (found !== 8 * n) throw 'found ' + found;
         }",
    ),
    (
        "dom_attributes",
        "function run(n) {
           const node = document.createElement('div');
           let total = 0;
           for (let i = 0; i < n; i++) {
             node.setAttribute('data-k' + (i & 15), String(i));
             total += node.getAttribute('data-k' + (i & 15)).length;
           }
           if (total <= 0) throw 'total';
         }",
    ),
    (
        "dom_dispatch",
        "function run(n) {
           const outer = document.createElement('div');
           const inner = document.createElement('span');
           outer.appendChild(inner);
           document.body.appendChild(outer);
           let count = 0;
           outer.addEventListener('ping', () => count++);
           inner.addEventListener('ping', () => count++, true);
           for (let i = 0; i < n; i++) inner.dispatchEvent(new Event('ping', { bubbles: true }));
           if (count !== 2 * n) throw 'count ' + count;
         }",
    ),
];

fn source(text: &str) -> Source {
    Source::from_str_limited(text, 64 * 1024).expect("a short workload")
}

fn main() {
    let mut size = 2_000_u32;
    let mut runs = 11_usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => size = args.next().and_then(|v| v.parse().ok()).unwrap_or(size),
            "--runs" => runs = args.next().and_then(|v| v.parse().ok()).unwrap_or(runs),
            _ => {}
        }
    }
    let limits = TaskLimits {
        steps_per_turn: u64::MAX / 2,
        ..TaskLimits::default()
    };
    let mut failed = false;
    for (name, workload) in WORKLOADS {
        for _ in 0..runs.max(1) {
            let mut engine = Engine::new(1);
            let mut load = engine
                .begin_document(ParserLimits::default())
                .expect("admit a document");
            load.feed(b"<!doctype html><body>").expect("parse");
            let mut page = load
                .finish()
                .expect("parse")
                .into_scripted(limits)
                .expect("script the document");
            page.queue_script(source(workload)).expect("queue");
            assert_eq!(page.run_next(), Ok(Some(ScriptTurn::Completed)));
            page.queue_script(source(&format!("run({size})")))
                .expect("queue");
            let start = Instant::now();
            let turn = page.run_next();
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if turn != Ok(Some(ScriptTurn::Completed)) {
                println!("{name} not runnable: {turn:?}");
                failed = true;
                break;
            }
            println!("sample {name} {elapsed} ok");
        }
    }
    if failed {
        std::process::exit(1);
    }
}
