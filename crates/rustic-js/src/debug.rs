//! Source-level debugging hooks for the bytecode VM (ADR 0069).
//!
//! Debugging costs nothing unless a host attaches a [`DebugHook`] before
//! scripts compile. With a hook attached, [`Agent::evaluate_source`]
//! registers each script's source under a [`ScriptId`] and the compiler
//! emits a step instruction before every statement. At a step the agent
//! decides on its own whether to pause (a breakpoint on that line, a step
//! request, or a `debugger` statement) and only then calls the hook, which
//! may inspect the stopped stack with [`Agent::debug_stack`] and
//! [`Agent::debug_scopes`] and returns how to continue.
//!
//! The hook runs with the VM stopped between two statements. It must not
//! run script: the inspection methods describe values without calling page
//! code (no getters, no `toString`), and steps are ignored while a pause
//! is in progress, so nothing the hook does can pause again.
//!
//! Scripts compiled before the hook was attached, `eval` code, `Function`
//! constructor bodies and module scripts carry no steps (TD-187).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::runtime::{Agent, JsResult, StringId, Value};
use crate::source::Source;

/// Most scripts one agent registers for debugging; later scripts run
/// without steps rather than grow the table without bound.
pub const MAX_DEBUG_SCRIPTS: usize = 4096;

/// Most breakpoints one agent holds.
pub const MAX_BREAKPOINTS: usize = 1024;

/// Most distinct lines the statement profile counts; later lines are not
/// counted.
pub const MAX_PROFILE_LINES: usize = 65_536;

/// Most frames [`Agent::debug_stack`] reports, innermost first.
pub const MAX_DEBUG_FRAMES: usize = 64;

/// Most bindings [`Agent::debug_scopes`] reports for one frame.
pub const MAX_DEBUG_BINDINGS: usize = 512;

/// A registered script: 1 for the first script compiled after the hook
/// was attached, then 2, 3 and so on in compilation order.
pub type ScriptId = u32;

/// Why execution stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseReason {
    /// A breakpoint on the statement's line.
    Breakpoint,
    /// The step the hook asked for.
    Step,
    /// A `debugger` statement.
    DebuggerStatement,
}

impl PauseReason {
    /// The protocol name of the reason.
    pub fn as_str(self) -> &'static str {
        match self {
            PauseReason::Breakpoint => "breakpoint",
            PauseReason::Step => "step",
            PauseReason::DebuggerStatement => "debugger-statement",
        }
    }
}

/// Where and why execution stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PausePoint {
    /// Why.
    pub reason: PauseReason,
    /// The script.
    pub script: ScriptId,
    /// One-based line of the statement about to run.
    pub line: u32,
    /// One-based UTF-16 column of that statement.
    pub column: u32,
}

/// How to continue after a pause.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Resume {
    /// Run until the next breakpoint or `debugger` statement.
    #[default]
    Continue,
    /// Stop at the next statement, entering calls.
    StepInto,
    /// Stop at the next statement of this frame or a caller.
    StepOver,
    /// Stop at the next statement of a caller.
    StepOut,
}

/// The host side of the debugger.
pub trait DebugHook {
    /// A script was registered and is about to compile. The host may set
    /// breakpoints in it with [`Agent::set_breakpoint`] before it runs.
    fn script_parsed(&self, agent: &mut Agent, script: ScriptId, source: &Source);

    /// Execution stopped at `point`. Inspect the stack, then return how to
    /// continue. Must not run script.
    fn paused(&self, agent: &mut Agent, point: &PausePoint) -> Resume;
}

/// One frame of a stopped stack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugFrame {
    /// The function's name; empty for a script's top level or an
    /// anonymous function.
    pub function: String,
    /// The frame's script, or 0 for code without steps.
    pub script: ScriptId,
    /// One-based line of the frame's current statement (0 when unknown).
    pub line: u32,
    /// One-based column of that statement (0 when unknown).
    pub column: u32,
}

/// One binding visible from a stopped frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugBinding {
    /// Which scope holds it: `local`, `closure` or `global`.
    pub scope: &'static str,
    /// The binding's name.
    pub name: String,
    /// A description of its value that ran no page code.
    pub value: String,
}

/// A statement start in compiled code: the source offset and the
/// register-allocated bindings visible there (an index into the code's
/// local tables).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DebugPoint {
    pub(crate) offset: u32,
    pub(crate) locals: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepMode {
    Run,
    Into,
    /// Stop at a statement whose stack is at most this deep.
    Over(usize),
    /// Stop at a statement whose stack is shallower than this.
    Out(usize),
}

/// The debugger state an agent keeps while a hook is attached.
pub(crate) struct DebugState {
    hook: Rc<dyn DebugHook>,
    sources: Vec<Rc<Source>>,
    breakpoints: HashSet<(ScriptId, u32)>,
    mode: StepMode,
    /// The script being compiled, or 0.
    pub(crate) compiling: ScriptId,
    pausing: bool,
    /// Statements run per script line, while profiling.
    profile: Option<HashMap<(ScriptId, u32), u64>>,
}

impl std::fmt::Debug for DebugState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugState")
            .field("scripts", &self.sources.len())
            .field("breakpoints", &self.breakpoints.len())
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Attach a debugger, or detach it with `None`. Only scripts compiled
    /// while a hook is attached can pause; detaching keeps their steps,
    /// which then cost one check each.
    pub fn set_debug_hook(&mut self, hook: Option<Rc<dyn DebugHook>>) {
        self.debug = hook.map(|hook| {
            Box::new(DebugState {
                hook,
                sources: Vec::new(),
                breakpoints: HashSet::new(),
                mode: StepMode::Run,
                compiling: 0,
                pausing: false,
                profile: None,
            })
        });
    }

    /// Whether a debugger is attached.
    pub fn has_debug_hook(&self) -> bool {
        self.debug.is_some()
    }

    /// Pause before the first statement on one-based `line` of `script`
    /// each time execution reaches it. Returns false without a debugger,
    /// for an unknown script or line, or past [`MAX_BREAKPOINTS`].
    pub fn set_breakpoint(&mut self, script: ScriptId, line: u32) -> bool {
        let Some(debug) = self.debug.as_mut() else {
            return false;
        };
        let Some(source) = script
            .checked_sub(1)
            .and_then(|index| debug.sources.get(index as usize))
        else {
            return false;
        };
        let lines = source.position(source.len() as u32).line;
        if line == 0 || line > lines {
            return false;
        }
        if debug.breakpoints.len() >= MAX_BREAKPOINTS
            && !debug.breakpoints.contains(&(script, line))
        {
            return false;
        }
        debug.breakpoints.insert((script, line));
        true
    }

    /// Remove a breakpoint; returns whether there was one.
    pub fn clear_breakpoint(&mut self, script: ScriptId, line: u32) -> bool {
        self.debug
            .as_mut()
            .is_some_and(|debug| debug.breakpoints.remove(&(script, line)))
    }

    /// Count the statements run on each line of registered scripts (or
    /// stop counting, discarding the counts).
    pub fn set_step_profiling(&mut self, on: bool) {
        if let Some(debug) = self.debug.as_mut() {
            debug.profile = on.then(HashMap::new);
        }
    }

    /// The statement counts so far, by script and line, in that order;
    /// counting continues from zero.
    pub fn take_step_profile(&mut self) -> Vec<(ScriptId, u32, u64)> {
        let Some(profile) = self.debug.as_mut().and_then(|debug| debug.profile.as_mut()) else {
            return Vec::new();
        };
        let mut counts: Vec<(ScriptId, u32, u64)> = profile
            .drain()
            .map(|((script, line), count)| (script, line, count))
            .collect();
        counts.sort_unstable();
        counts
    }

    /// Stop at the next statement that runs, as if stepping into it.
    pub fn request_pause(&mut self) {
        if let Some(debug) = self.debug.as_mut() {
            debug.mode = StepMode::Into;
        }
    }

    /// Register `source` before it compiles, so its code gets steps.
    /// Returns the script id, or 0 without a debugger or past
    /// [`MAX_DEBUG_SCRIPTS`].
    pub(crate) fn debug_register(&mut self, source: &Source) -> ScriptId {
        let Some(debug) = self.debug.as_mut() else {
            return 0;
        };
        if debug.sources.len() >= MAX_DEBUG_SCRIPTS || debug.sources.try_reserve(1).is_err() {
            return 0;
        }
        debug.sources.push(Rc::new(source.clone()));
        let script = debug.sources.len() as ScriptId;
        let hook = Rc::clone(&debug.hook);
        hook.script_parsed(self, script, source);
        script
    }

    /// The script whose code is being compiled (0 for none).
    pub(crate) fn debug_compiling(&self) -> ScriptId {
        self.debug.as_ref().map_or(0, |debug| debug.compiling)
    }

    pub(crate) fn set_debug_compiling(&mut self, script: ScriptId) {
        if let Some(debug) = self.debug.as_mut() {
            debug.compiling = script;
        }
    }

    fn debug_position(&self, script: ScriptId, offset: u32) -> (u32, u32) {
        self.debug
            .as_ref()
            .and_then(|debug| debug.sources.get(script.checked_sub(1)? as usize))
            .map_or((0, 0), |source| {
                let position = source.position(offset);
                (position.line, position.column)
            })
    }

    /// The step at `offset` of `script` with `depth` frames on the stack:
    /// pause if a breakpoint, step or `debugger` statement says so.
    /// `previous` is the offset of the code's statement before this one:
    /// a breakpoint stops only at the first statement of its line.
    pub(crate) fn debug_step(
        &mut self,
        script: ScriptId,
        offset: u32,
        previous: Option<u32>,
        depth: usize,
        statement: bool,
    ) -> JsResult<()> {
        let Some(debug) = self.debug.as_ref() else {
            return Ok(());
        };
        if debug.pausing || script == 0 {
            return Ok(());
        }
        let (line, column) = self.debug_position(script, offset);
        if let Some(profile) = self.debug.as_mut().and_then(|debug| debug.profile.as_mut()) {
            if let Some(count) = profile.get_mut(&(script, line)) {
                *count = count.saturating_add(1);
            } else if profile.len() < MAX_PROFILE_LINES {
                profile.insert((script, line), 1);
            }
        }
        let first_on_line =
            previous.is_none_or(|previous| self.debug_position(script, previous).0 != line);
        let debug = self.debug.as_mut().expect("checked above");
        let stepped = match debug.mode {
            StepMode::Run => false,
            StepMode::Into => true,
            StepMode::Over(at) => depth <= at,
            StepMode::Out(at) => depth < at,
        };
        let reason = if statement {
            PauseReason::DebuggerStatement
        } else if stepped {
            PauseReason::Step
        } else if first_on_line && debug.breakpoints.contains(&(script, line)) {
            PauseReason::Breakpoint
        } else {
            return Ok(());
        };
        debug.pausing = true;
        let hook = Rc::clone(&debug.hook);
        let point = PausePoint {
            reason,
            script,
            line,
            column,
        };
        let resume = hook.paused(self, &point);
        if let Some(debug) = self.debug.as_mut() {
            debug.pausing = false;
            debug.mode = match resume {
                Resume::Continue => StepMode::Run,
                Resume::StepInto => StepMode::Into,
                Resume::StepOver => StepMode::Over(depth),
                Resume::StepOut => StepMode::Out(depth),
            };
        }
        // The hook may have taken long; a cancelled page stops here.
        self.check_native_work()
    }

    /// The stopped stack, innermost frame first (at most
    /// [`MAX_DEBUG_FRAMES`]). Empty when no script is running.
    pub fn debug_stack(&self) -> Vec<DebugFrame> {
        let mut frames = Vec::new();
        for (code, pc) in self.vm_frames().take(MAX_DEBUG_FRAMES) {
            let point = code.debug_point_before(pc);
            let (line, column) = point.map_or((0, 0), |point| {
                self.debug_position(code.script, point.offset)
            });
            frames.push(DebugFrame {
                function: self.heap.string(code.name).to_text(),
                script: if point.is_some() { code.script } else { 0 },
                line,
                column,
            });
        }
        frames
    }

    /// The bindings visible from frame `index` of [`Self::debug_stack`]:
    /// its register locals, the environments on its scope chain, and the
    /// global lexical declarations (not the global object's properties).
    /// Values are described without running page code.
    pub fn debug_scopes(&mut self, index: usize) -> Vec<DebugBinding> {
        let Some((locals, env)) = self.debug_frame_view(index) else {
            return Vec::new();
        };
        let mut seen: HashSet<StringId> = HashSet::new();
        let mut found: Vec<(&'static str, StringId, Value)> = Vec::new();
        for (name, value) in locals {
            if seen.insert(name) {
                found.push(("local", name, value));
            }
        }
        let mut at = Some(env);
        while let Some(env) = at {
            let (bindings, outer, global) = self.debug_env_bindings(env);
            let scope = if global { "global" } else { "closure" };
            for (name, value) in bindings {
                if !visible_name(&self.heap.string(name).to_text()) {
                    continue;
                }
                if seen.insert(name) {
                    found.push((scope, name, value));
                }
            }
            at = outer;
        }
        found.truncate(MAX_DEBUG_BINDINGS);
        found
            .into_iter()
            .map(|(scope, name, value)| DebugBinding {
                scope,
                name: self.heap.string(name).to_text(),
                value: if value == Value::UNINITIALIZED {
                    "<uninitialized>".to_owned()
                } else {
                    self.describe_value(value)
                },
            })
            .collect()
    }
}

/// Whether a binding name is the page's own (not one of the compiler's
/// internal names such as `%default` or `new.target`).
pub(crate) fn visible_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['%', '.']) && name != "super" && name != "this"
}
