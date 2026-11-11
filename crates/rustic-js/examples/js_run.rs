//! Run a script file and print its completion value, the time it took and
//! what the heap holds afterwards:
//!
//! ```text
//! cargo run --release -p rustic-js --example js_run -- script.js
//! ```

use std::time::Instant;

use rustic_js::EvalError;
use rustic_js::runtime::Agent;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: js_run <script.js>");
        std::process::exit(2);
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("{path}: {error}");
            std::process::exit(2);
        }
    };
    let mut agent = Agent::new().expect("a default heap fits its intrinsics");
    let start = Instant::now();
    let result = agent.evaluate(&source);
    let elapsed = start.elapsed();
    match result {
        Ok(value) => println!("{}", agent.display(value)),
        Err(EvalError::Throw(value)) => {
            let text = agent.to_string(value).map_or_else(
                |_| "<unprintable>".to_owned(),
                |s| agent.heap.string(s).to_text(),
            );
            println!("uncaught {text}");
        }
        Err(other) => println!("error: {other}"),
    }
    eprintln!(
        "{elapsed:?}; heap {} bytes, {} objects, {} collections",
        agent.heap.bytes_used(),
        agent.heap.object_count(),
        agent.heap.collections()
    );
}
