//! `rustic automate`: the automation and DevTools session (ADR 0069).
//!
//! The session serves one page over the standard streams of the process
//! the user started: commands arrive one JSON object per line and each
//! gets one response line ([`rustic_devtools::protocol`]). In a split run
//! the renderer pulls commands from the broker, which reads them from its
//! standard input and admits only the methods the session was granted;
//! this module checks them again. There is no listening socket.
//!
//! Life of a session: before the page loads the client may set
//! breakpoints (`debugger.*`) and ask for capabilities; `page.load` (or
//! any page command) starts the load. While scripts run, a breakpoint,
//! step or `debugger` statement sends a `debugger.paused` event and the
//! session answers only `debugger.*` and `session.*` until resumed. After
//! the load settles the client inspects and drives the page until
//! `session.close` or end of input.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::rc::{Rc, Weak};
use std::time::Instant;

use rustic_devtools::protocol::{self, Command, ErrorCode, Refusal};
use rustic_devtools::{Grant, Grants, Value, base64, redact};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_engine::{ClassicScriptLoad, ScriptedDocument};
use rustic_js::Source;
use rustic_js::debug::{DebugHook, PausePoint, Resume, ScriptId};
use rustic_js::runtime::Agent;
use rustic_url::Url;

use crate::loader::LoadError;
use crate::network::{ResourceKind, Session as NetworkSession};

/// Most nodes one session hands ids to.
const MAX_NODES: usize = 200_000;
/// Most network entries kept; later ones are counted as dropped.
const MAX_NETWORK_ENTRIES: usize = 1024;
/// Most console messages kept between two `console.messages` calls.
const MAX_CONSOLE: usize = 4096;
/// Most text returned by `dom.text`, in bytes.
const MAX_TEXT_BYTES: usize = 1 << 20;
/// Longest `runtime.evaluate` expression, in UTF-16 code units.
const MAX_EXPRESSION_UNITS: usize = 1 << 20;
/// Most manual-clock time one `page.advance` may move.
const MAX_ADVANCE_MS: u64 = 60_000;

thread_local! {
    static ACTIVE: RefCell<Option<Rc<Session>>> = const { RefCell::new(None) };
}

/// Start a session on this thread's next load.
pub(crate) fn begin(grants: Grants) -> Rc<Session> {
    let session = Rc::new(Session {
        grants,
        renderer: crate::renderer::active(),
        state: RefCell::new(State::default()),
        started: Instant::now(),
        closed: Cell::new(false),
    });
    ACTIVE.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&session)));
    session
}

/// End this thread's session.
pub(crate) fn end() {
    ACTIVE.with(|slot| slot.borrow_mut().take());
}

/// The session attached to this thread's load, if any.
pub(crate) fn active() -> Option<Rc<Session>> {
    ACTIVE.with(|slot| slot.borrow().clone())
}

/// What the loader offers the session after the load settles.
pub(crate) trait PageHost {
    /// The topmost node at a point in CSS pixels.
    fn hit_test(
        &self,
        document: &ScriptedDocument<rustic_base::ManualClock>,
        x: f64,
        y: f64,
    ) -> Result<Option<NodeId>, LoadError>;
    /// The page as PNG bytes, with its width and height in pixels.
    fn screenshot(
        &self,
        document: &ScriptedDocument<rustic_base::ManualClock>,
    ) -> Result<(u32, u32, Vec<u8>), LoadError>;
    /// Run ready tasks and due timers (within the load's budgets);
    /// returns the turns run.
    fn settle(
        &self,
        document: &mut ScriptedDocument<rustic_base::ManualClock>,
    ) -> Result<u64, LoadError>;
    /// Move the manual clock and run what became due.
    fn advance(
        &self,
        document: &mut ScriptedDocument<rustic_base::ManualClock>,
        by: std::time::Duration,
    ) -> Result<u64, LoadError>;
}

/// One automation session.
pub(crate) struct Session {
    grants: Grants,
    /// Commands come through the broker (split run) or standard input.
    renderer: bool,
    state: RefCell<State>,
    started: Instant,
    closed: Cell<bool>,
}

#[derive(Default)]
struct State {
    document_url: Option<Url>,
    nodes: Vec<NodeId>,
    node_ids: HashMap<NodeId, u64>,
    breakpoints: Vec<Breakpoint>,
    next_breakpoint: u64,
    scripts: Vec<ScriptInfo>,
    /// External scripts fetched but not yet registered: URL and a hash of
    /// their text.
    fetched_scripts: Vec<(String, u64)>,
    network: Vec<Value>,
    network_dropped: u64,
    console: Vec<Value>,
    console_dropped: u64,
    stashed: Option<Command>,
    pause_on_start: bool,
    load_ms: Option<f64>,
    settle_turns: u64,
    layout_flushes: u64,
    paused: bool,
    /// The document's bytes as parsed, CR LF and CR as LF, to place inline
    /// scripts on document lines.
    document_text: Vec<u8>,
    /// Where the next inline script's text is searched from.
    inline_cursor: usize,
    /// A `runtime.evaluate` expression is compiling.
    evaluating: bool,
}

struct Breakpoint {
    id: u64,
    url: Option<String>,
    script: Option<ScriptId>,
    line: u32,
    resolved: Vec<ScriptId>,
}

struct ScriptInfo {
    id: ScriptId,
    url: String,
    inline: bool,
    lines: u32,
    /// Lines before the script's first line in its document (inline
    /// scripts): protocol lines are document lines.
    line_offset: u32,
}

/// A refusal of `command` with `code`.
fn refuse(command: &Command, code: ErrorCode, message: impl Into<String>) -> Refusal {
    Refusal::new(Some(command.id), code, message)
}

fn param_u64(command: &Command, name: &str) -> Result<u64, Refusal> {
    command
        .params
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            refuse(
                command,
                ErrorCode::InvalidParams,
                format!("{name} must be a non-negative integer"),
            )
        })
}

fn param_f64(command: &Command, name: &str) -> Result<f64, Refusal> {
    command
        .params
        .get(name)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            refuse(
                command,
                ErrorCode::InvalidParams,
                format!("{name} must be a number"),
            )
        })
}

fn param_str<'a>(command: &'a Command, name: &str) -> Result<&'a str, Refusal> {
    command
        .params
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            refuse(
                command,
                ErrorCode::InvalidParams,
                format!("{name} must be a string"),
            )
        })
}

fn failed(command: &Command, error: impl std::fmt::Display) -> Refusal {
    refuse(command, ErrorCode::Failed, error.to_string())
}

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// FNV-1a, to recognize a fetched script's text when it registers.
fn hash(bytes: &[u8]) -> u64 {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Session {
    fn granted(&self, grant: Grant) -> bool {
        self.grants.contains(grant)
    }

    /// Write one protocol line to the client.
    fn send(&self, line: &str) {
        if self.renderer {
            crate::renderer::automation_write(line);
            return;
        }
        write_stdout_line(line);
    }

    fn event(&self, name: &str, params: Value) {
        self.send(&protocol::event(name, params));
    }

    /// The next admitted command, or `None` at the end of the session.
    /// Refused lines are answered here and skipped.
    fn next_command(&self) -> Option<Command> {
        if self.closed.get() {
            return None;
        }
        loop {
            let line = if self.renderer {
                crate::renderer::automation_read()
            } else {
                read_stdin_line()
            };
            let Some(line) = line else {
                self.closed.set(true);
                return None;
            };
            if line.trim().is_empty() {
                continue;
            }
            match protocol::admit(&line, self.grants) {
                Ok(command) => return Some(command),
                Err(refusal) => self.send(&refusal.to_line()),
            }
        }
    }

    fn reply(&self, command: &Command, result: Result<Value, Refusal>) {
        match result {
            Ok(value) => self.send(&protocol::response(command.id, value)),
            Err(refusal) => self.send(&refusal.to_line()),
        }
    }

    fn capabilities(&self) -> Value {
        Value::object([
            ("protocol", Value::string(protocol::PROTOCOL)),
            (
                "grants",
                Value::Array(
                    self.grants
                        .iter()
                        .map(|g| Value::string(g.name()))
                        .collect(),
                ),
            ),
            (
                "methods",
                Value::Array(
                    rustic_devtools::authority::METHODS
                        .iter()
                        .filter(|(_, grant)| grant.is_none_or(|g| self.granted(g)))
                        .map(|(name, _)| Value::string(*name))
                        .collect(),
                ),
            ),
            ("remote", Value::Bool(false)),
        ])
    }

    // ---- before the load ------------------------------------------------

    /// Attach to a load before any HTML is fed: install the debugger and
    /// serve commands until the client starts the load.
    pub(crate) fn attach(
        self: &Rc<Self>,
        load: &mut ClassicScriptLoad<rustic_base::ManualClock>,
        document_url: &Url,
    ) -> Result<(), LoadError> {
        self.state.borrow_mut().document_url = Some(document_url.clone());
        if self.granted(Grant::Debug) || self.granted(Grant::Profile) {
            let hook: Rc<dyn DebugHook> = Rc::new(Hook(Rc::downgrade(self)));
            load.set_debug_hook(hook)?;
        }
        while let Some(command) = self.next_command() {
            let result = match command.method.as_str() {
                "session.capabilities" => Ok(self.capabilities()),
                "session.close" => {
                    self.closed.set(true);
                    self.reply(&command, Ok(Value::Null));
                    break;
                }
                "page.load" => {
                    self.reply(
                        &command,
                        Ok(Value::object([("loading", Value::Bool(true))])),
                    );
                    break;
                }
                "debugger.setBreakpoint" => self.set_breakpoint(None, &command),
                "debugger.removeBreakpoint" => self.remove_breakpoint(None, &command),
                "debugger.scripts" => Ok(self.scripts()),
                "debugger.pause" => {
                    self.state.borrow_mut().pause_on_start = true;
                    Ok(Value::Null)
                }
                _ => {
                    // Any page command starts the load and runs after it.
                    self.state.borrow_mut().stashed = Some(command);
                    break;
                }
            };
            self.reply(&command, result);
        }
        Ok(())
    }

    /// The first statement of the load stops when `debugger.pause` came
    /// before it.
    pub(crate) fn pause_requested(&self) -> bool {
        std::mem::take(&mut self.state.borrow_mut().pause_on_start)
    }

    /// Remember an external script's text so its registration can name it.
    pub(crate) fn note_script(&self, url: &Url, bytes: &[u8]) {
        let mut state = self.state.borrow_mut();
        if state.fetched_scripts.len() < 1024 {
            state.fetched_scripts.push((url.serialize(), hash(bytes)));
        }
    }

    // ---- debugger ---------------------------------------------------------

    /// Keep the document's bytes as the parser reads them, to place
    /// inline scripts on document lines (at most the load's input limit).
    pub(crate) fn note_document_bytes(&self, bytes: &[u8]) {
        let mut state = self.state.borrow_mut();
        let mut previous_cr = state.document_text.last() == Some(&b'\r');
        for &byte in bytes {
            match byte {
                b'\n' if previous_cr => {}
                b'\r' => state.document_text.push(b'\n'),
                _ => state.document_text.push(byte),
            }
            previous_cr = byte == b'\r';
        }
    }

    /// The document line offset of `script` (0 for external scripts).
    fn line_offset(&self, script: ScriptId) -> u32 {
        self.state
            .borrow()
            .scripts
            .iter()
            .find(|info| info.id == script)
            .map_or(0, |info| info.line_offset)
    }

    /// A script line as the protocol shows it.
    fn shown_line(&self, script: ScriptId, line: u32) -> Value {
        if line == 0 {
            return Value::int(0);
        }
        Value::int(line.saturating_add(self.line_offset(script)))
    }

    fn script_url(&self, script: ScriptId) -> Value {
        self.state
            .borrow()
            .scripts
            .iter()
            .find(|info| info.id == script)
            .map_or(Value::Null, |info| Value::string(redact::url(&info.url)))
    }

    fn scripts(&self) -> Value {
        Value::Array(
            self.state
                .borrow()
                .scripts
                .iter()
                .map(|info| {
                    Value::object([
                        ("script", Value::int(info.id)),
                        ("url", Value::string(redact::url(&info.url))),
                        ("inline", Value::Bool(info.inline)),
                        ("lines", Value::int(info.lines)),
                        ("lineOffset", Value::int(info.line_offset)),
                    ])
                })
                .collect(),
        )
    }

    fn script_parsed(&self, agent: &mut Agent, script: ScriptId, source: &Source) {
        if agent.has_debug_hook() && self.granted(Grant::Profile) && script == 1 {
            agent.set_step_profiling(true);
        }
        let script_text = String::from_utf16_lossy(source.units());
        let digest = hash(script_text.as_bytes());
        let lines = source.position(source.len() as u32).line;
        let info = {
            let mut state = self.state.borrow_mut();
            let found = state
                .fetched_scripts
                .iter()
                .position(|(_, h)| *h == digest)
                .map(|index| state.fetched_scripts.remove(index).0);
            let (url, inline, line_offset) = if let Some(url) = found {
                (url, false, 0)
            } else if state.evaluating {
                ("devtools:evaluate".to_owned(), false, 0)
            } else {
                let document_url = state
                    .document_url
                    .as_ref()
                    .map_or_else(String::new, Url::serialize);
                // Inline text appears in the document in order; find it
                // after the previous inline script.
                let needle = script_text.as_bytes();
                let from = state.inline_cursor.min(state.document_text.len());
                let at = (!needle.is_empty())
                    .then(|| {
                        state.document_text[from..]
                            .windows(needle.len())
                            .position(|window| window == needle)
                    })
                    .flatten()
                    .map(|position| from + position);
                let offset = at.map_or(0, |at| {
                    state.inline_cursor = at + needle.len();
                    state.document_text[..at]
                        .iter()
                        .filter(|&&b| b == b'\n')
                        .count() as u32
                });
                (document_url, true, offset)
            };
            state.scripts.push(ScriptInfo {
                id: script,
                url: url.clone(),
                inline,
                lines,
                line_offset,
            });
            (url, inline, line_offset)
        };
        // Breakpoints set by URL before the script existed.
        let mut resolved = Vec::new();
        for breakpoint in &mut self.state.borrow_mut().breakpoints {
            if breakpoint
                .url
                .as_deref()
                .is_some_and(|pattern| url_matches(pattern, &info.0))
                && breakpoint
                    .line
                    .checked_sub(info.2)
                    .is_some_and(|line| agent.set_breakpoint(script, line))
            {
                breakpoint.resolved.push(script);
                resolved.push(breakpoint.id);
            }
        }
        if self.granted(Grant::Debug) {
            self.event(
                "debugger.scriptParsed",
                Value::object([
                    ("script", Value::int(script)),
                    ("url", Value::string(redact::url(&info.0))),
                    ("inline", Value::Bool(info.1)),
                    ("lines", Value::int(lines)),
                    ("lineOffset", Value::int(info.2)),
                    (
                        "breakpoints",
                        Value::Array(
                            resolved
                                .into_iter()
                                .map(|id| Value::Number(id as f64))
                                .collect(),
                        ),
                    ),
                ]),
            );
        }
        if self.pause_requested() {
            agent.request_pause();
        }
    }

    fn set_breakpoint(
        &self,
        agent: Option<&mut Agent>,
        command: &Command,
    ) -> Result<Value, Refusal> {
        let line = u32::try_from(param_u64(command, "line")?)
            .ok()
            .filter(|&line| line > 0)
            .ok_or_else(|| refuse(command, ErrorCode::InvalidParams, "line must be 1 or more"))?;
        let url = command
            .params
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let script = match command.params.get("script") {
            Some(value) => Some(
                value
                    .as_u64()
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| {
                        refuse(command, ErrorCode::InvalidParams, "script must be an id")
                    })?,
            ),
            None => None,
        };
        if url.is_some() == script.is_some() {
            return Err(refuse(
                command,
                ErrorCode::InvalidParams,
                "give exactly one of url and script",
            ));
        }
        let mut state = self.state.borrow_mut();
        if state.breakpoints.len() >= rustic_js::debug::MAX_BREAKPOINTS {
            return Err(failed(command, "too many breakpoints"));
        }
        state.next_breakpoint += 1;
        let id = state.next_breakpoint;
        let mut resolved = Vec::new();
        if let Some(agent) = agent {
            let targets: Vec<(ScriptId, u32)> = state
                .scripts
                .iter()
                .filter(|info| match (&url, script) {
                    (_, Some(script)) => info.id == script,
                    (Some(pattern), None) => url_matches(pattern, &info.url),
                    (None, None) => false,
                })
                .map(|info| (info.id, info.line_offset))
                .collect();
            for (target, offset) in targets {
                if line
                    .checked_sub(offset)
                    .is_some_and(|line| agent.set_breakpoint(target, line))
                {
                    resolved.push(target);
                }
            }
        }
        let result = Value::object([
            ("breakpoint", Value::Number(id as f64)),
            (
                "resolved",
                Value::Array(resolved.iter().map(|&s| Value::int(s)).collect()),
            ),
        ]);
        state.breakpoints.push(Breakpoint {
            id,
            url,
            script,
            line,
            resolved,
        });
        Ok(result)
    }

    fn remove_breakpoint(
        &self,
        agent: Option<&mut Agent>,
        command: &Command,
    ) -> Result<Value, Refusal> {
        let id = param_u64(command, "breakpoint")?;
        let mut state = self.state.borrow_mut();
        let index = state
            .breakpoints
            .iter()
            .position(|b| b.id == id)
            .ok_or_else(|| refuse(command, ErrorCode::InvalidParams, "no such breakpoint"))?;
        let breakpoint = state.breakpoints.remove(index);
        if let Some(agent) = agent {
            let still = |script: ScriptId| {
                state
                    .breakpoints
                    .iter()
                    .any(|b| b.line == breakpoint.line && b.resolved.contains(&script))
            };
            for &script in breakpoint.resolved.iter().chain(breakpoint.script.iter()) {
                let offset = state
                    .scripts
                    .iter()
                    .find(|info| info.id == script)
                    .map_or(0, |info| info.line_offset);
                if !still(script) {
                    if let Some(line) = breakpoint.line.checked_sub(offset) {
                        agent.clear_breakpoint(script, line);
                    }
                }
            }
        }
        Ok(Value::Null)
    }

    fn stack(&self, agent: &Agent) -> Value {
        Value::Array(
            agent
                .debug_stack()
                .into_iter()
                .map(|frame| {
                    Value::object([
                        ("function", Value::string(frame.function)),
                        ("script", Value::int(frame.script)),
                        ("url", self.script_url(frame.script)),
                        ("line", self.shown_line(frame.script, frame.line)),
                        ("column", Value::int(frame.column)),
                    ])
                })
                .collect(),
        )
    }

    fn paused(&self, agent: &mut Agent, point: &PausePoint) -> Resume {
        if self.closed.get() || !self.granted(Grant::Debug) {
            return Resume::Continue;
        }
        self.state.borrow_mut().paused = true;
        self.event(
            "debugger.paused",
            Value::object([
                ("reason", Value::string(point.reason.as_str())),
                ("script", Value::int(point.script)),
                ("url", self.script_url(point.script)),
                ("line", self.shown_line(point.script, point.line)),
                ("column", Value::int(point.column)),
                ("stack", self.stack(agent)),
            ]),
        );
        let resume = loop {
            let Some(command) = self.next_command() else {
                break Resume::Continue;
            };
            let step = match command.method.as_str() {
                "debugger.resume" => Some(Resume::Continue),
                "debugger.stepInto" => Some(Resume::StepInto),
                "debugger.stepOver" => Some(Resume::StepOver),
                "debugger.stepOut" => Some(Resume::StepOut),
                _ => None,
            };
            if let Some(step) = step {
                self.reply(&command, Ok(Value::Null));
                break step;
            }
            let result = match command.method.as_str() {
                "session.capabilities" => Ok(self.capabilities()),
                "session.close" => {
                    self.closed.set(true);
                    self.reply(&command, Ok(Value::Null));
                    break Resume::Continue;
                }
                "debugger.stack" => Ok(self.stack(agent)),
                "debugger.scopes" => param_u64(&command, "frame").map(|frame| {
                    Value::Array(
                        agent
                            .debug_scopes(usize::try_from(frame).unwrap_or(usize::MAX))
                            .into_iter()
                            .map(|binding| {
                                Value::object([
                                    ("scope", Value::string(binding.scope)),
                                    ("name", Value::string(binding.name)),
                                    ("value", Value::string(binding.value)),
                                ])
                            })
                            .collect(),
                    )
                }),
                "debugger.scripts" => Ok(self.scripts()),
                "debugger.setBreakpoint" => self.set_breakpoint(Some(agent), &command),
                "debugger.removeBreakpoint" => self.remove_breakpoint(Some(agent), &command),
                "debugger.pause" => Ok(Value::Null),
                _ => Err(refuse(
                    &command,
                    ErrorCode::WrongState,
                    "paused in the debugger: only debugger and session methods run until resumed",
                )),
            };
            self.reply(&command, result);
        };
        self.state.borrow_mut().paused = false;
        self.event("debugger.resumed", Value::Null);
        resume
    }

    // ---- the loaded page ------------------------------------------------

    /// Record the load's end and keep the page's console messages for
    /// `console.messages`.
    pub(crate) fn loaded(&self, document: &mut ScriptedDocument<rustic_base::ManualClock>) {
        self.state.borrow_mut().load_ms = Some(self.started.elapsed().as_secs_f64() * 1000.0);
        self.collect_console(document);
    }

    fn collect_console(&self, document: &ScriptedDocument<rustic_base::ManualClock>) {
        let log = document.take_console_messages();
        let mut state = self.state.borrow_mut();
        state.console_dropped = state.console_dropped.saturating_add(log.dropped);
        for message in log.messages {
            if state.console.len() >= MAX_CONSOLE {
                state.console_dropped = state.console_dropped.saturating_add(1);
                continue;
            }
            let level = match message.level {
                rustic_engine::ConsoleLevel::Debug => "debug",
                rustic_engine::ConsoleLevel::Log => "log",
                rustic_engine::ConsoleLevel::Info => "info",
                rustic_engine::ConsoleLevel::Warn => "warn",
                rustic_engine::ConsoleLevel::Error => "error",
            };
            state.console.push(Value::object([
                ("level", Value::string(level)),
                ("text", Value::string(message.text)),
                ("truncated", Value::Bool(message.truncated)),
            ]));
        }
    }

    /// Serve the loaded page until the client closes the session.
    pub(crate) fn serve(
        &self,
        document: &mut ScriptedDocument<rustic_base::ManualClock>,
        page: &dyn PageHost,
    ) {
        let stashed = self.state.borrow_mut().stashed.take();
        let mut next = stashed.or_else(|| self.next_command());
        while let Some(command) = next {
            if command.method == "session.close" {
                self.closed.set(true);
                self.reply(&command, Ok(Value::Null));
                return;
            }
            let result = self.handle(document, page, &command);
            self.collect_console(document);
            self.reply(&command, result);
            next = self.next_command();
        }
    }

    fn node_ref(&self, id: NodeId) -> Value {
        let mut state = self.state.borrow_mut();
        if let Some(&number) = state.node_ids.get(&id) {
            return Value::Number(number as f64);
        }
        if state.nodes.len() >= MAX_NODES {
            return Value::Null;
        }
        state.nodes.push(id);
        let number = state.nodes.len() as u64;
        state.node_ids.insert(id, number);
        Value::Number(number as f64)
    }

    fn node_param(&self, command: &Command, document: &Document) -> Result<NodeId, Refusal> {
        let number = param_u64(command, "node")?;
        let id = number
            .checked_sub(1)
            .and_then(|index| self.state.borrow().nodes.get(index as usize).copied())
            .ok_or_else(|| refuse(command, ErrorCode::InvalidParams, "unknown node"))?;
        document
            .node(id)
            .map(|_| id)
            .map_err(|_| refuse(command, ErrorCode::InvalidParams, "the node was removed"))
    }

    fn handle(
        &self,
        document: &mut ScriptedDocument<rustic_base::ManualClock>,
        page: &dyn PageHost,
        command: &Command,
    ) -> Result<Value, Refusal> {
        match command.method.as_str() {
            "session.capabilities" => Ok(self.capabilities()),
            "page.load" => Ok(Value::object([("loading", Value::Bool(false))])),
            "page.info" => {
                let engine_document = document.document();
                let dom = engine_document.parsed().document();
                let url = self
                    .state
                    .borrow()
                    .document_url
                    .as_ref()
                    .map_or_else(String::new, Url::serialize);
                Ok(Value::object([
                    ("url", Value::string(redact::url(&url))),
                    ("title", Value::string(title(dom))),
                    ("loaded", Value::Bool(true)),
                ]))
            }
            "page.settle" => {
                let turns = page.settle(document).map_err(|e| failed(command, e))?;
                self.state.borrow_mut().settle_turns += turns;
                Ok(Value::object([("turns", Value::Number(turns as f64))]))
            }
            "page.advance" => {
                let ms = param_u64(command, "ms")?;
                if ms > MAX_ADVANCE_MS {
                    return Err(refuse(
                        command,
                        ErrorCode::InvalidParams,
                        "ms must be at most 60000",
                    ));
                }
                let turns = page
                    .advance(document, std::time::Duration::from_millis(ms))
                    .map_err(|e| failed(command, e))?;
                self.state.borrow_mut().settle_turns += turns;
                Ok(Value::object([("turns", Value::Number(turns as f64))]))
            }
            "page.screenshot" => {
                let (width, height, png) =
                    page.screenshot(document).map_err(|e| failed(command, e))?;
                Ok(Value::object([
                    ("width", Value::int(width)),
                    ("height", Value::int(height)),
                    ("png", Value::string(base64::encode(&png))),
                ]))
            }
            "dom.document" => self.dom_document(document, command),
            "dom.query" => {
                let selector = param_str(command, "selector")?;
                let engine_document = document.document();
                let dom = engine_document.parsed().document();
                let scope = match command.params.get("node") {
                    Some(_) => self.node_param(command, dom)?,
                    None => dom.root(),
                };
                let quirks = engine_document.parsed().mode() == rustic_html::DocumentMode::Quirks;
                let found = query(dom, scope, selector, quirks)
                    .map_err(|e| refuse(command, ErrorCode::InvalidParams, e))?;
                drop(engine_document);
                Ok(Value::object([(
                    "nodes",
                    Value::Array(found.into_iter().map(|id| self.node_ref(id)).collect()),
                )]))
            }
            "dom.attributes" => {
                let engine_document = document.document();
                let dom = engine_document.parsed().document();
                let id = self.node_param(command, dom)?;
                let node = dom
                    .node(id)
                    .map_err(|e| failed(command, format!("{e:?}")))?;
                let NodeKind::Element(element) = node.kind() else {
                    return Err(refuse(command, ErrorCode::InvalidParams, "not an element"));
                };
                Ok(Value::object([(
                    "attributes",
                    Value::Array(
                        element
                            .attributes()
                            .iter()
                            .map(|attribute| {
                                Value::Array(vec![
                                    Value::string(qualified(attribute.name())),
                                    Value::string(text(attribute.value().as_units())),
                                ])
                            })
                            .collect(),
                    ),
                )]))
            }
            "dom.text" => {
                let engine_document = document.document();
                let dom = engine_document.parsed().document();
                let id = self.node_param(command, dom)?;
                let mut out: Vec<u16> = Vec::new();
                let mut truncated = false;
                for (descendant, _) in dom
                    .walk(id)
                    .map_err(|e| failed(command, format!("{e:?}")))?
                {
                    if let Ok(NodeKind::Text(data) | NodeKind::CdataSection(data)) =
                        dom.node(descendant).map(rustic_dom::Node::kind)
                    {
                        if out.len() + data.as_units().len() > MAX_TEXT_BYTES / 2 {
                            truncated = true;
                            break;
                        }
                        out.extend_from_slice(data.as_units());
                    }
                }
                Ok(Value::object([
                    ("text", Value::string(text(&out))),
                    ("truncated", Value::Bool(truncated)),
                ]))
            }
            "css.computed" => {
                let id = {
                    let engine_document = document.document();
                    self.node_param(command, engine_document.parsed().document())?
                };
                let layout = document
                    .node_layout(id)
                    .map_err(|e| failed(command, format!("{e:?}")))?;
                let style = layout
                    .style
                    .ok_or_else(|| failed(command, "the node has no computed style"))?;
                let wanted: Option<Vec<&str>> = command
                    .params
                    .get("properties")
                    .map(|list| {
                        list.as_array()
                            .and_then(|items| items.iter().map(Value::as_str).collect())
                            .ok_or_else(|| {
                                refuse(
                                    command,
                                    ErrorCode::InvalidParams,
                                    "properties must be strings",
                                )
                            })
                    })
                    .transpose()?;
                let keep = |name: &str| wanted.as_ref().is_none_or(|list| list.contains(&name));
                Ok(Value::object([
                    (
                        "properties",
                        Value::Object(
                            style
                                .record()
                                .longhands
                                .iter()
                                .filter(|(name, _)| keep(name))
                                .map(|(name, value)| {
                                    ((*name).to_owned(), Value::string(value.clone()))
                                })
                                .collect(),
                        ),
                    ),
                    (
                        "custom",
                        Value::Object(
                            style
                                .record()
                                .custom_properties
                                .iter()
                                .filter(|(name, _)| keep(name))
                                .map(|(name, value)| (name.clone(), Value::string(value.clone())))
                                .collect(),
                        ),
                    ),
                ]))
            }
            "layout.box" => {
                let id = {
                    let engine_document = document.document();
                    self.node_param(command, engine_document.parsed().document())?
                };
                let layout = document
                    .node_layout(id)
                    .map_err(|e| failed(command, format!("{e:?}")))?;
                let pair = |[a, b]: [f64; 2], x: &str, y: &str| {
                    Value::object([(x, Value::Number(a)), (y, Value::Number(b))])
                };
                Ok(Value::object([
                    (
                        "rect",
                        layout.client_rect.map_or(Value::Null, |[x, y, w, h]| {
                            Value::object([
                                ("x", Value::Number(x)),
                                ("y", Value::Number(y)),
                                ("width", Value::Number(w)),
                                ("height", Value::Number(h)),
                            ])
                        }),
                    ),
                    (
                        "content",
                        layout
                            .box_sizes
                            .map_or(Value::Null, |b| pair(b.content, "width", "height")),
                    ),
                    (
                        "border",
                        layout
                            .box_sizes
                            .map_or(Value::Null, |b| pair(b.border, "width", "height")),
                    ),
                    (
                        "padding",
                        layout
                            .box_sizes
                            .map_or(Value::Null, |b| pair(b.padding, "left", "top")),
                    ),
                ]))
            }
            "layout.hitTest" => {
                let (x, y) = (param_f64(command, "x")?, param_f64(command, "y")?);
                let hit = page
                    .hit_test(document, x, y)
                    .map_err(|e| failed(command, e))?;
                Ok(Value::object([(
                    "node",
                    hit.map_or(Value::Null, |id| self.node_ref(id)),
                )]))
            }
            "console.messages" => {
                let mut state = self.state.borrow_mut();
                let messages = std::mem::take(&mut state.console);
                let dropped = std::mem::take(&mut state.console_dropped);
                Ok(Value::object([
                    ("messages", Value::Array(messages)),
                    ("dropped", Value::Number(dropped as f64)),
                ]))
            }
            "profile.metrics" => {
                let flushes = document.take_layout_flush_count();
                let mut state = self.state.borrow_mut();
                state.layout_flushes += flushes;
                Ok(Value::object([
                    (
                        "elapsedMs",
                        Value::Number(self.started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("loadMs", state.load_ms.map_or(Value::Null, Value::Number)),
                    ("settleTurns", Value::Number(state.settle_turns as f64)),
                    ("layoutFlushes", Value::Number(state.layout_flushes as f64)),
                    ("scripts", Value::Number(state.scripts.len() as f64)),
                    ("nodes", Value::Number(state.nodes.len() as f64)),
                ]))
            }
            "profile.statements" => {
                let counts = document
                    .devtools_agent()
                    .map(Agent::take_step_profile)
                    .unwrap_or_default();
                Ok(Value::object([(
                    "lines",
                    Value::Array(
                        counts
                            .into_iter()
                            .map(|(script, line, count)| {
                                Value::object([
                                    ("script", Value::int(script)),
                                    ("url", self.script_url(script)),
                                    ("line", self.shown_line(script, line)),
                                    ("count", Value::Number(count as f64)),
                                ])
                            })
                            .collect(),
                    ),
                )]))
            }
            "network.requests" => {
                let state = self.state.borrow();
                Ok(Value::object([
                    ("requests", Value::Array(state.network.clone())),
                    ("dropped", Value::Number(state.network_dropped as f64)),
                ]))
            }
            "input.click" => {
                let (x, y) = (param_f64(command, "x")?, param_f64(command, "y")?);
                let Some(hit) = page
                    .hit_test(document, x, y)
                    .map_err(|e| failed(command, e))?
                else {
                    return Ok(Value::object([
                        ("node", Value::Null),
                        ("delivered", Value::Bool(false)),
                    ]));
                };
                let event = rustic_engine::InputEvent::Click {
                    client_x: x as i32,
                    client_y: y as i32,
                    modifiers: rustic_engine::Modifiers::default(),
                };
                let delivered = document.queue_input(hit, event).is_ok();
                if delivered {
                    let turns = page.settle(document).map_err(|e| failed(command, e))?;
                    self.state.borrow_mut().settle_turns += turns;
                }
                Ok(Value::object([
                    ("node", self.node_ref(hit)),
                    ("delivered", Value::Bool(delivered)),
                ]))
            }
            "runtime.evaluate" => {
                let expression = param_str(command, "expression")?;
                let source = Source::from_str_limited(expression, MAX_EXPRESSION_UNITS)
                    .map_err(|e| refuse(command, ErrorCode::InvalidParams, e.to_string()))?;
                self.state.borrow_mut().evaluating = true;
                let outcome = document.evaluate_for_devtools(&source);
                self.state.borrow_mut().evaluating = false;
                match outcome {
                    Ok(Ok(value)) => Ok(Value::object([("value", Value::string(value))])),
                    Ok(Err(thrown)) => Ok(Value::object([("exception", Value::string(thrown))])),
                    Err(stop) => Err(failed(command, format!("the page stopped: {stop:?}"))),
                }
            }
            "debugger.scripts" => Ok(self.scripts()),
            "debugger.setBreakpoint" => {
                let agent = document.devtools_agent().ok();
                self.set_breakpoint(agent, command)
            }
            "debugger.removeBreakpoint" => {
                let agent = document.devtools_agent().ok();
                self.remove_breakpoint(agent, command)
            }
            "debugger.pause" => {
                if let Ok(agent) = document.devtools_agent() {
                    agent.request_pause();
                }
                Ok(Value::Null)
            }
            "debugger.resume" | "debugger.stepInto" | "debugger.stepOver" | "debugger.stepOut"
            | "debugger.stack" | "debugger.scopes" => {
                Err(refuse(command, ErrorCode::WrongState, "not paused"))
            }
            // `session.close` is handled by the loop; nothing else is admitted.
            _ => Err(refuse(command, ErrorCode::UnknownMethod, "unknown method")),
        }
    }

    fn dom_document(
        &self,
        document: &ScriptedDocument<rustic_base::ManualClock>,
        command: &Command,
    ) -> Result<Value, Refusal> {
        let limit = match command.params.get("maxNodes") {
            Some(_) => usize::try_from(param_u64(command, "maxNodes")?).unwrap_or(usize::MAX),
            None => 10_000,
        };
        let engine_document = document.document();
        let dom = engine_document.parsed().document();
        let mut nodes = Vec::new();
        let mut truncated = false;
        for (id, depth) in dom
            .walk(dom.root())
            .map_err(|e| failed(command, format!("{e:?}")))?
        {
            if nodes.len() >= limit {
                truncated = true;
                break;
            }
            let Ok(node) = dom.node(id) else { continue };
            let mut members = vec![
                ("node", self.node_ref(id)),
                (
                    "parent",
                    node.parent().map_or(Value::Null, |p| self.node_ref(p)),
                ),
                ("depth", Value::Number(depth as f64)),
            ];
            match node.kind() {
                NodeKind::Document => members.push(("type", Value::string("document"))),
                NodeKind::DocumentFragment => members.push(("type", Value::string("fragment"))),
                NodeKind::Element(element) => {
                    members.push(("type", Value::string("element")));
                    members.push(("name", Value::string(qualified(element.name()))));
                    members.push((
                        "attributes",
                        Value::Array(
                            element
                                .attributes()
                                .iter()
                                .map(|a| {
                                    Value::Array(vec![
                                        Value::string(qualified(a.name())),
                                        Value::string(text(a.value().as_units())),
                                    ])
                                })
                                .collect(),
                        ),
                    ));
                }
                NodeKind::Text(data) | NodeKind::CdataSection(data) => {
                    members.push(("type", Value::string("text")));
                    members.push(("value", Value::string(text(data.as_units()))));
                }
                NodeKind::Comment(data) => {
                    members.push(("type", Value::string("comment")));
                    members.push(("value", Value::string(text(data.as_units()))));
                }
                NodeKind::DocumentType(doctype) => {
                    members.push(("type", Value::string("doctype")));
                    members.push(("name", Value::string(text(doctype.name.as_units()))));
                }
                NodeKind::ProcessingInstruction(pi) => {
                    members.push(("type", Value::string("processing-instruction")));
                    members.push(("name", Value::string(text(pi.target().as_units()))));
                }
            }
            nodes.push(Value::object(members));
        }
        Ok(Value::object([
            ("nodes", Value::Array(nodes)),
            ("truncated", Value::Bool(truncated)),
        ]))
    }

    // ---- network diagnostics ----------------------------------------------

    fn record(&self, entry: Value) {
        let mut state = self.state.borrow_mut();
        if state.network.len() >= MAX_NETWORK_ENTRIES {
            state.network_dropped = state.network_dropped.saturating_add(1);
        } else {
            state.network.push(entry);
        }
    }
}

/// The debugger hook: forwards to the session while it lives.
struct Hook(Weak<Session>);

impl DebugHook for Hook {
    fn script_parsed(&self, agent: &mut Agent, script: ScriptId, source: &Source) {
        if let Some(session) = self.0.upgrade() {
            session.script_parsed(agent, script, source);
        }
    }

    fn paused(&self, agent: &mut Agent, point: &PausePoint) -> Resume {
        self.0
            .upgrade()
            .map_or(Resume::Continue, |session| session.paused(agent, point))
    }
}

/// Whether a breakpoint's `url` names a script URL: the whole URL, or,
/// for a pattern without a scheme, the end of its path.
fn url_matches(pattern: &str, url: &str) -> bool {
    if pattern.contains("://") {
        return pattern == url;
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    !pattern.is_empty() && path.ends_with(pattern)
}

fn qualified(name: &rustic_dom::QualifiedName) -> String {
    match name.prefix() {
        Some(prefix) => format!("{prefix}:{}", name.local()),
        None => name.local().to_owned(),
    }
}

/// The text of the document's first `title` element.
fn title(dom: &Document) -> String {
    let Ok(walk) = dom.walk(dom.root()) else {
        return String::new();
    };
    for (id, _) in walk {
        let Ok(node) = dom.node(id) else { continue };
        if let NodeKind::Element(element) = node.kind() {
            if element.name().local() == "title" {
                let mut out = Vec::new();
                let mut child = node.first_child();
                while let Some(id) = child {
                    let Ok(node) = dom.node(id) else { break };
                    if let NodeKind::Text(data) = node.kind() {
                        out.extend_from_slice(data.as_units());
                    }
                    child = node.next_sibling();
                }
                return text(&out).split_whitespace().collect::<Vec<_>>().join(" ");
            }
        }
    }
    String::new()
}

/// Every descendant element of `scope` that matches `selector`.
fn query(
    dom: &Document,
    scope: NodeId,
    selector: &str,
    quirks: bool,
) -> Result<Vec<NodeId>, String> {
    let limits = rustic_css::CssLimits {
        max_diagnostics: 0,
        ..rustic_css::CssLimits::default()
    };
    let parser = rustic_css::CssParser::new(limits, rustic_base::CancellationToken::default())
        .map_err(|_| "invalid CSS limits".to_owned())?;
    let parsed = parser
        .selector_list(selector, &rustic_css::NamespaceMap::new())
        .map_err(|error| format!("{error:?}"))?;
    let list = parsed.value.map_err(|_| "invalid selector".to_owned())?;
    let work = rustic_style::SelectorWork::new(10_000_000, None);
    rustic_style::ScopedSelector::new(dom, scope, &list, quirks, &work)
        .all()
        .map_err(|error| format!("{error:?}"))
}

/// One line from standard input, without its line break; `None` at the
/// end of input. A line longer than the command limit is cut, which the
/// gate then refuses.
pub(crate) fn read_stdin_line() -> Option<String> {
    let stdin = std::io::stdin();
    let mut bytes = Vec::new();
    let limit = (protocol::MAX_COMMAND_BYTES + 2) as u64;
    match stdin.lock().take(limit).read_until(b'\n', &mut bytes) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            } else if bytes.len() as u64 == limit {
                // Skip the rest of the oversized line.
                let mut rest = Vec::new();
                let _ = stdin.lock().read_until(b'\n', &mut rest);
            }
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}

/// Standard input lines, read on a thread of their own so a broker
/// waiting for the client can still notice its renderer timing out.
fn stdin_lines() -> &'static std::sync::Mutex<std::sync::mpsc::Receiver<String>> {
    static LINES: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Receiver<String>>> =
        std::sync::OnceLock::new();
    LINES.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Some(line) = read_stdin_line() {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        std::sync::Mutex::new(receive)
    })
}

/// The broker's gate (ADR 0069): the next command line from standard
/// input that `grants` admits, answering refused lines itself so they
/// never reach the renderer; `None` at the end of input or once `stop`
/// is set (the renderer's watchdog fired).
pub(crate) fn next_admitted(
    grants: Grants,
    stop: &std::sync::atomic::AtomicBool,
) -> Option<String> {
    let lines = stdin_lines().lock().ok()?;
    loop {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let line = match lines.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
        };
        if line.trim().is_empty() {
            continue;
        }
        match protocol::admit(&line, grants) {
            Ok(_) => return Some(line),
            Err(refusal) => write_stdout_line(&refusal.to_line()),
        }
    }
}

/// Write one protocol line to standard output.
pub(crate) fn write_stdout_line(line: &str) {
    let mut stdout = std::io::stdout().lock();
    // A client that went away ends the session at its next read.
    let _ = stdout
        .write_all(line.as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush());
}

/// A network session that records each request for `network.requests`
/// (granted sessions only), redacting credentials.
#[derive(Debug)]
pub(crate) struct ObservedSession {
    inner: Rc<dyn NetworkSession>,
}

/// Wrap `session` so the active session records its requests, when one
/// with the network grant is attached.
pub(crate) fn observe(session: Rc<dyn NetworkSession>) -> Rc<dyn NetworkSession> {
    match active() {
        Some(active) if active.granted(Grant::Network) => {
            active.record(Value::object([
                ("kind", Value::string("document")),
                ("method", Value::string("GET")),
                (
                    "url",
                    Value::string(redact::url(&session.document_url().serialize())),
                ),
                ("outcome", Value::string("ok")),
            ]));
            Rc::new(ObservedSession { inner: session })
        }
        _ => session,
    }
}

fn kind_name(kind: &ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Stylesheet => "stylesheet",
        ResourceKind::Image => "image",
        ResourceKind::Script { module: true, .. } => "module",
        ResourceKind::Script { .. } => "script",
    }
}

impl NetworkSession for ObservedSession {
    fn document_url(&self) -> &Url {
        self.inner.document_url()
    }

    fn csp(&self) -> &rustic_net::csp::CspList {
        self.inner.csp()
    }

    fn is_local(&self) -> bool {
        self.inner.is_local()
    }

    fn allows_eval(&self) -> bool {
        self.inner.allows_eval()
    }

    fn fetch(
        &self,
        url: &Url,
        kind: &ResourceKind,
        max_bytes: usize,
    ) -> Result<Vec<u8>, rustic_style::FetchError> {
        let result = self.inner.fetch(url, kind, max_bytes);
        if let Some(session) = active() {
            let outcome = match &result {
                Ok(_) => "ok",
                Err(rustic_style::FetchError::Blocked) => "blocked",
                Err(rustic_style::FetchError::NotFound) => "not-found",
                Err(rustic_style::FetchError::TooLarge) => "too-large",
                Err(rustic_style::FetchError::Failed) => "failed",
                Err(rustic_style::FetchError::Cancelled) => "cancelled",
            };
            let mut members = vec![
                ("kind", Value::string(kind_name(kind))),
                ("method", Value::string("GET")),
                ("url", Value::string(redact::url(&url.serialize()))),
                ("outcome", Value::string(outcome)),
            ];
            if let Ok(bytes) = &result {
                members.push(("bytes", Value::Number(bytes.len() as f64)));
            }
            session.record(Value::object(members));
        }
        result
    }

    fn charset_of(&self, url: &Url) -> Option<String> {
        self.inner.charset_of(url)
    }

    fn script_fetch(
        &self,
        request: rustic_engine::ScriptRequest,
    ) -> Result<rustic_engine::ScriptResponse, rustic_engine::NetworkFailure> {
        let method = request.method.clone();
        let url = request.url.serialize();
        let request_headers: Vec<Value> = request
            .headers
            .iter()
            .map(|(name, value)| header(name, value))
            .collect();
        let result = self.inner.script_fetch(request);
        if let Some(session) = active() {
            let mut members = vec![
                ("kind", Value::string("fetch")),
                ("method", Value::string(method)),
                ("url", Value::string(redact::url(&url))),
                ("requestHeaders", Value::Array(request_headers)),
            ];
            match &result {
                Ok(response) => {
                    members.push(("outcome", Value::string("ok")));
                    members.push(("status", Value::int(response.status)));
                    members.push((
                        "responseHeaders",
                        Value::Array(
                            response
                                .headers
                                .iter()
                                .map(|(name, value)| header(name, value))
                                .collect(),
                        ),
                    ));
                    members.push(("bytes", Value::Number(response.body.len() as f64)));
                }
                Err(_) => members.push(("outcome", Value::string("network-error"))),
            }
            session.record(Value::object(members));
        }
        result
    }

    fn storage_host(&self) -> Result<Option<Rc<dyn rustic_engine::StorageHost>>, String> {
        self.inner.storage_host()
    }

    fn finish(&self) -> Result<(), String> {
        self.inner.finish()
    }

    fn open_popup(&self, popup: &rustic_engine::PopupRequest) -> Result<(), String> {
        self.inner.open_popup(popup)
    }
}

fn header(name: &str, value: &[u8]) -> Value {
    Value::Array(vec![
        Value::string(name),
        Value::string(redact::header_value(name, &String::from_utf8_lossy(value))),
    ])
}
