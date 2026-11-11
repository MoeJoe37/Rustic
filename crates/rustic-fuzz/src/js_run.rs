//! The JavaScript interpreter (`rustic-js` JS-04): crash-freedom, bounded
//! execution and differentials against its own slow paths.
//!
//! Each input is a script. It runs in three fresh agents with a small heap
//! budget, a small frame limit and a step budget: as shipped, with the
//! inline caches of property and global sites turned off, and with a
//! collection at every safe point. Invariants: no run panics, overflows
//! the Rust stack or outlives its step budget; the uncached run ends
//! exactly like the cached one; the collecting run ends like it too unless
//! one of them ran out of memory (collecting earlier changes what fits);
//! and afterwards every agent still evaluates script and stays within its
//! heap budget.
use crate::Target;
use rustic_js::runtime::{Agent, Kind, Roots, Value};
use rustic_js::{EvalError, VmLimits};

pub(crate) struct JsRunTarget;

const HEAP_LIMIT: usize = 2 * 1024 * 1024;
const STEPS: u64 = 3_000;

/// One program per line of `corpus/js-run/*.js`, plus the parser's seeds.
pub(crate) fn seeds(root: &std::path::Path) -> std::io::Result<Vec<Vec<u8>>> {
    let directory = root.join("crates/rustic-fuzz/corpus/js-run");
    let mut seeds = Vec::new();
    for (_, bytes) in crate::read_dir_inputs(&directory)? {
        let source = String::from_utf8_lossy(&bytes);
        seeds.extend(source.lines().map(|line| line.as_bytes().to_vec()));
    }
    seeds.extend(crate::js::seeds(root)?);
    Ok(seeds)
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Cached,
    Uncached,
    Collecting,
}

/// How a value looks from outside, without naming heap ids (collections
/// move those between modes).
fn show(agent: &Agent, value: Value) -> String {
    match value.kind() {
        Kind::Object(_) => "object".into(),
        _ => agent.display(value),
    }
}

/// Run `text` in a fresh agent and describe how it ended.
fn run(text: &str, mode: Mode) -> Result<String, String> {
    let mut agent =
        Agent::with_heap_limit(HEAP_LIMIT).map_err(|_| "agent does not fit its budget")?;
    agent.set_vm_limits(VmLimits {
        max_frames: 200,
        max_stack_values: 1 << 14,
        max_native_depth: 16,
        ..VmLimits::default()
    });
    agent.set_inline_caches(mode != Mode::Uncached);
    agent.heap.set_gc_stress(mode == Mode::Collecting);
    agent.set_step_budget(Some(STEPS));
    let outcome = match agent.evaluate(text) {
        Ok(value) => format!("value {}", show(&agent, value)),
        Err(EvalError::Throw(value)) => format!("throw {}", show(&agent, value)),
        Err(EvalError::Parse(_) | EvalError::SourceTooLong) => "parse".into(),
        Err(EvalError::Compile(_)) => "compile".into(),
        Err(EvalError::OutOfMemory) => "oom".into(),
        Err(EvalError::Cancelled) => "cancelled".into(),
    };
    // The agent must survive whatever the script did.
    agent.heap.set_gc_stress(false);
    agent.set_step_budget(Some(STEPS));
    agent.collect_garbage(Roots::default());
    if agent.heap.bytes_used() > HEAP_LIMIT {
        return Err(format!(
            "heap holds {} bytes past its budget",
            agent.heap.bytes_used()
        ));
    }
    match agent.evaluate("1 + 1") {
        Ok(v) if v.as_number() == Some(2.0) => {}
        Err(EvalError::OutOfMemory) if outcome == "oom" => {}
        other => {
            return Err(format!(
                "agent unusable after a run that ended with {outcome}: {other:?}"
            ));
        }
    }
    Ok(outcome)
}

impl Target for JsRunTarget {
    fn name(&self) -> &'static str {
        "js-run"
    }

    fn max_len(&self) -> usize {
        2048
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"function ",
            b"return ",
            b"var ",
            b"let ",
            b"const ",
            b"for (",
            b" of ",
            b" in ",
            b"while (",
            b"try {",
            b"} catch (e) {",
            b"} finally {",
            b"throw ",
            b"break",
            b"continue",
            b"new ",
            b"this.",
            b".prototype",
            b"__proto__: ",
            b"get x() {",
            b"set x(v) {",
            b"delete ",
            b"typeof ",
            b"=>",
            b"...",
            b"?.",
            b"??",
            b"`${",
            b"}`",
            b"[Symbol.iterator]",
            b".next()",
            b".length",
            b"globalThis.",
            b"arguments",
            b"with (",
            b"\"use strict\";",
            b"o.x = ",
            b"o.x",
            b"{x: 1}",
            b"[1, 2, 3]",
            b"1e308",
            b"-0",
            b"++",
            b"+= ",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let text = String::from_utf8_lossy(input);
        let cached = run(&text, Mode::Cached)?;
        let uncached = run(&text, Mode::Uncached)?;
        if cached != uncached {
            return Err(format!(
                "inline caches change the result: {cached:?} with, {uncached:?} without"
            ));
        }
        let collecting = run(&text, Mode::Collecting)?;
        if cached != collecting && cached != "oom" && collecting != "oom" {
            return Err(format!(
                "collecting at every safe point changes the result: {cached:?} normally, {collecting:?} collecting"
            ));
        }
        Ok(())
    }
}
