//! Debugger hooks (ADR 0072): breakpoints, stepping, `debugger`
//! statements, stacks and scopes, and the guarantees that debugging is
//! off unless attached and that a pause cannot run page code.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use rustic_js::Source;
use rustic_js::debug::{DebugHook, PausePoint, PauseReason, Resume, ScriptId};
use rustic_js::runtime::Agent;

/// Records every pause and answers from a script of resumes.
#[derive(Default)]
struct Recorder {
    /// Breakpoints to set when a script is registered: `(script, line)`.
    breakpoints: Vec<(ScriptId, u32)>,
    resumes: RefCell<VecDeque<Resume>>,
    scripts: RefCell<Vec<(ScriptId, usize)>>,
    pauses: RefCell<Vec<Pause>>,
}

#[derive(Debug)]
struct Pause {
    point: PausePoint,
    stack: Vec<(String, u32)>,
    scopes: Vec<(String, String, String)>,
}

impl DebugHook for Recorder {
    fn script_parsed(&self, agent: &mut Agent, script: ScriptId, source: &Source) {
        self.scripts.borrow_mut().push((script, source.len()));
        for &(at, line) in &self.breakpoints {
            if at == script {
                assert!(agent.set_breakpoint(script, line), "line {line} exists");
            }
        }
    }

    fn paused(&self, agent: &mut Agent, point: &PausePoint) -> Resume {
        let stack = agent
            .debug_stack()
            .into_iter()
            .map(|frame| (frame.function, frame.line))
            .collect();
        let scopes = agent
            .debug_scopes(0)
            .into_iter()
            .map(|binding| (binding.scope.to_owned(), binding.name, binding.value))
            .collect();
        self.pauses.borrow_mut().push(Pause {
            point: *point,
            stack,
            scopes,
        });
        self.resumes.borrow_mut().pop_front().unwrap_or_default()
    }
}

fn agent_with(recorder: &Rc<Recorder>) -> Agent {
    let mut agent = Agent::new().unwrap();
    agent.set_debug_hook(Some(Rc::clone(recorder) as Rc<dyn DebugHook>));
    agent
}

const PROGRAM: &str = "\
function add(a, b) {
  let sum = a + b;
  return sum;
}
let total = 0;
for (let i = 0; i < 2; i++) {
  total = add(total, i + 1);
}
total;
";

#[test]
fn breakpoint_pauses_with_stack_and_scope() {
    let recorder = Rc::new(Recorder {
        breakpoints: vec![(1, 3)],
        ..Recorder::default()
    });
    let mut agent = agent_with(&recorder);
    let value = agent.evaluate(PROGRAM).unwrap();
    assert_eq!(agent.display(value), "3");
    let pauses = recorder.pauses.borrow();
    // Line 3 runs once per call: two pauses.
    assert_eq!(pauses.len(), 2, "{pauses:?}");
    let first = &pauses[0];
    assert_eq!(first.point.reason, PauseReason::Breakpoint);
    assert_eq!((first.point.script, first.point.line), (1, 3));
    assert_eq!(first.point.column, 3);
    assert_eq!(
        first.stack,
        vec![("add".to_owned(), 3), (String::new(), 7)],
        "innermost frame first"
    );
    let local = |name: &str| {
        first
            .scopes
            .iter()
            .find(|(_, n, _)| n == name)
            .map(|(scope, _, value)| (scope.as_str(), value.as_str()))
    };
    assert_eq!(local("a"), Some(("local", "0")));
    assert_eq!(local("b"), Some(("local", "1")));
    assert_eq!(local("sum"), Some(("local", "1")));
    assert_eq!(local("total"), Some(("global", "0")));
    let second = &pauses[1];
    let sum = second.scopes.iter().find(|(_, n, _)| n == "sum").unwrap();
    assert_eq!(sum.2, "3");
    assert_eq!(*recorder.scripts.borrow(), vec![(1, PROGRAM.len())]);
}

#[test]
fn stepping_into_over_and_out() {
    let recorder = Rc::new(Recorder {
        breakpoints: vec![(1, 7)],
        resumes: RefCell::new(VecDeque::from([
            Resume::StepInto, // 7 -> 2 (inside add)
            Resume::StepOver, // 2 -> 3
            Resume::StepOut,  // 3 -> the loop body's next run (line 7)
            Resume::StepOver, // 7 -> the statement after the loop
            Resume::Continue,
        ])),
        ..Recorder::default()
    });
    let mut agent = agent_with(&recorder);
    agent.evaluate(PROGRAM).unwrap();
    let lines: Vec<(PauseReason, u32, usize)> = recorder
        .pauses
        .borrow()
        .iter()
        .map(|pause| (pause.point.reason, pause.point.line, pause.stack.len()))
        .collect();
    assert_eq!(
        lines,
        vec![
            (PauseReason::Breakpoint, 7, 1),
            (PauseReason::Step, 2, 2),
            (PauseReason::Step, 3, 2),
            (PauseReason::Step, 7, 1),
            (PauseReason::Step, 9, 1),
        ]
    );
}

#[test]
fn breakpoint_in_a_loop_stops_every_iteration_once_per_line() {
    // Two statements on the breakpoint's line: one stop per iteration.
    let source = "let n = 0;\nwhile (n < 3) {\n  n++; n++;\n}\nn;";
    let recorder = Rc::new(Recorder {
        breakpoints: vec![(1, 3)],
        ..Recorder::default()
    });
    let mut agent = agent_with(&recorder);
    let value = agent.evaluate(source).unwrap();
    assert_eq!(agent.display(value), "4");
    assert_eq!(recorder.pauses.borrow().len(), 2);
}

#[test]
fn debugger_statement_pauses_only_with_a_hook() {
    let source = "let x = 1;\ndebugger;\nx + 1;";
    let mut plain = Agent::new().unwrap();
    let value = plain.evaluate(source).unwrap();
    assert_eq!(
        plain.display(value),
        "2",
        "a debugger statement does nothing without a debugger"
    );
    let recorder = Rc::new(Recorder::default());
    let mut agent = agent_with(&recorder);
    agent.evaluate(source).unwrap();
    let pauses = recorder.pauses.borrow();
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].point.reason, PauseReason::DebuggerStatement);
    assert_eq!(pauses[0].point.line, 2);
}

#[test]
fn inspection_runs_no_page_code() {
    // A getter and a toString that would leave a mark if inspection ran them.
    let source = "\
globalThis.ran = 0;
let tricky = { get x() { ran++; return 1; }, toString() { ran++; return 'no'; } };
debugger;
ran;";
    let recorder = Rc::new(Recorder::default());
    let mut agent = agent_with(&recorder);
    let value = agent.evaluate(source).unwrap();
    assert_eq!(agent.display(value), "0");
    let pauses = recorder.pauses.borrow();
    let tricky = pauses[0]
        .scopes
        .iter()
        .find(|(_, name, _)| name == "tricky")
        .unwrap();
    assert!(!tricky.2.contains("no"), "{tricky:?}");
}

#[test]
fn scripts_before_the_hook_and_unknown_breakpoints() {
    let mut agent = Agent::new().unwrap();
    agent.evaluate("function f() { return 1; }").unwrap();
    assert!(!agent.set_breakpoint(1, 1), "no debugger attached");
    let recorder = Rc::new(Recorder::default());
    agent.set_debug_hook(Some(Rc::clone(&recorder) as Rc<dyn DebugHook>));
    agent.request_pause();
    // `f` was compiled without steps: only the new script's statement stops.
    agent.evaluate("f();").unwrap();
    let pauses = recorder.pauses.borrow();
    assert_eq!(pauses.len(), 1);
    assert_eq!(pauses[0].point.script, 1);
    drop(pauses);
    assert!(!agent.set_breakpoint(1, 2), "the script has one line");
    assert!(!agent.set_breakpoint(9, 1), "no such script");
    assert!(agent.set_breakpoint(1, 1));
    assert!(agent.clear_breakpoint(1, 1));
    assert!(!agent.clear_breakpoint(1, 1));
    agent.set_debug_hook(None);
    assert!(!agent.has_debug_hook());
}

#[test]
fn closures_show_captured_bindings() {
    let source = "\
function outer() {
  let captured = 'kept';
  return function inner() {
    debugger;
    return captured;
  };
}
outer()();";
    let recorder = Rc::new(Recorder::default());
    let mut agent = agent_with(&recorder);
    agent.evaluate(source).unwrap();
    let pauses = recorder.pauses.borrow();
    let captured = pauses[0]
        .scopes
        .iter()
        .find(|(_, name, _)| name == "captured")
        .unwrap();
    assert_eq!(captured.0, "closure");
    assert_eq!(captured.2, "kept");
    assert_eq!(pauses[0].stack[0].0, "inner");
}

#[test]
fn statement_profile_counts_lines() {
    let recorder = Rc::new(Recorder::default());
    let mut agent = agent_with(&recorder);
    agent.set_step_profiling(true);
    agent.evaluate(PROGRAM).unwrap();
    let profile = agent.take_step_profile();
    let count = |line: u32| {
        profile
            .iter()
            .find(|&&(script, l, _)| script == 1 && l == line)
            .map_or(0, |&(_, _, count)| count)
    };
    assert_eq!(
        (count(2), count(3), count(5), count(6), count(7)),
        (2, 2, 1, 1, 2)
    );
    assert!(
        recorder.pauses.borrow().is_empty(),
        "profiling never pauses"
    );
    assert!(agent.take_step_profile().is_empty(), "taken counts restart");
    agent.set_step_profiling(false);
}
