//! WEB-01 document-owned script tasks and microtask checkpoints.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use rustic_base::{CancellationToken, Clock, SystemClock};
use rustic_bindings::dom_exception;
use rustic_bindings::page::{self, PageTree};
use rustic_dom::{Document, NodeId};
use rustic_html::{DocumentMode, HtmlDocumentParser, ParseError as HtmlParseError};
use rustic_js::module::{GraphError, GraphLimits, ModuleEvalError, ModuleGraph};
use rustic_js::runtime::{
    Abrupt, Agent, DEFAULT_HEAP_BYTES, HostRoot, JsStr, ObjectId, OutOfMemory, Value,
};
use rustic_js::{CompileError, EvalError, ParseError, Source};

use crate::frames::{RenderFrame, Rendering};
use crate::lifecycle::DocumentScope;
use crate::local_modules::{LocalModules, ModuleKey};
use crate::tasks::{ScriptQueue, Task, TimerId};
use crate::window_tasks::WindowWork;
use crate::{EngineDocument, InputEvent};

/// Resource limits for one scripted document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskLimits {
    /// Maximum number of scripts, timers, input tasks and animation callbacks waiting to run.
    pub pending_scripts: usize,
    /// Maximum total UTF-16 units in waiting script sources and keyboard input.
    /// This also bounds each inline script evaluated directly by the parser host.
    pub pending_source_units: usize,
    /// VM safe points shared by a task and its microtask checkpoint.
    pub steps_per_turn: u64,
    /// Jobs that a single microtask checkpoint may run, including newly queued jobs.
    pub jobs_per_checkpoint: usize,
    /// JavaScript heap budget in bytes.
    pub heap_bytes: usize,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            pending_scripts: 128,
            pending_source_units: 512 * 1024,
            steps_per_turn: 1_000_000,
            jobs_per_checkpoint: 4096,
            heap_bytes: DEFAULT_HEAP_BYTES,
        }
    }
}

/// Why a document task could not be queued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptQueueError {
    /// Script execution has stopped.
    Stopped(ScriptStop),
    /// The pending task or source limit would be exceeded.
    Limit,
    /// The queue could not allocate a slot.
    Capacity,
    /// The requested delay exceeds the monotonic clock range.
    DeadlineOverflow,
    /// The input target is outside the active document tree.
    InvalidInputTarget,
    /// This document has exhausted its non-reusable timer identities.
    TimerIdsExhausted,
    /// This document has exhausted its animation callback identities.
    AnimationIdsExhausted,
}

impl fmt::Display for ScriptQueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped(reason) => reason.fmt(f),
            Self::Limit => f.write_str("pending document task limit reached"),
            Self::Capacity => f.write_str("cannot allocate a document task slot"),
            Self::InvalidInputTarget => {
                f.write_str("input target is outside the active document tree")
            }
            Self::DeadlineOverflow => f.write_str("script timer deadline exceeds the clock range"),
            Self::TimerIdsExhausted => f.write_str("script timer identity space exhausted"),
            Self::AnimationIdsExhausted => {
                f.write_str("animation callback identity space exhausted")
            }
        }
    }
}

impl std::error::Error for ScriptQueueError {}

/// Terminal reason for stopping a document's script execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptStop {
    /// The document's shared cancellation token was set.
    DocumentCancelled,
    /// The VM safe-point budget was exhausted.
    StepLimit,
    /// The checkpoint still had jobs after reaching its job limit.
    JobLimit,
    /// The JavaScript heap budget was exhausted.
    OutOfMemory,
    /// This document has exhausted its non-reusable frame sequence.
    FrameIdsExhausted,
}

impl fmt::Display for ScriptStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DocumentCancelled => "document cancelled",
            Self::StepLimit => "script turn exhausted its safe-point budget",
            Self::JobLimit => "microtask checkpoint exhausted its job budget",
            Self::OutOfMemory => "script agent exhausted its heap budget",
            Self::FrameIdsExhausted => "document frame identity space exhausted",
        })
    }
}

impl std::error::Error for ScriptStop {}

/// A script failure that still permits the microtask checkpoint and later turns.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptFailure {
    /// The source did not parse.
    Parse(ParseError),
    /// The parsed script could not be compiled.
    Compile(CompileError),
    /// A thrown value reached the top level. The host receives the failure category.
    UncaughtException,
}

/// The result of one script or input task and its completed microtask checkpoint.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptTurn {
    /// The task completed normally.
    Completed,
    /// The task failed and the checkpoint still ran.
    Failed(ScriptFailure),
}

/// Work selected by the document pump when the host supplies a renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum DocumentTurn<T> {
    /// A script, timer or input task ran and completed its microtask checkpoint.
    Script(ScriptTurn),
    /// The host renderer ran for an offered rendering opportunity.
    Rendered {
        /// Ticket to validate before presenting the output.
        frame: RenderFrame,
        /// The renderer's result, including any host-defined error.
        output: T,
    },
}

#[derive(Debug)]
enum ScriptTreeState {
    Parsing(Box<HtmlDocumentParser>),
    Ready(EngineDocument),
}

#[derive(Debug)]
struct ScriptTree {
    state: RefCell<ScriptTreeState>,
    current_script: Cell<Option<NodeId>>,
    pending_classics: RefCell<HashSet<NodeId>>,
}

impl PageTree for ScriptTree {
    fn read(&self) -> Option<Ref<'_, Document>> {
        Ref::filter_map(self.state.try_borrow().ok()?, |state| match state {
            ScriptTreeState::Parsing(parser) => parser.live_document(),
            ScriptTreeState::Ready(owner) => Some(owner.parsed().document()),
        })
        .ok()
    }

    fn write(&self) -> Option<RefMut<'_, Document>> {
        RefMut::filter_map(self.state.try_borrow_mut().ok()?, |state| match state {
            ScriptTreeState::Parsing(parser) => parser.live_document_mut(),
            ScriptTreeState::Ready(owner) => Some(owner.parsed_mut().document_mut()),
        })
        .ok()
    }

    fn quirks_mode(&self) -> bool {
        let mode = match &*self.state.borrow() {
            ScriptTreeState::Parsing(parser) => parser.live_document_mode(),
            ScriptTreeState::Ready(owner) => Some(owner.parsed().mode()),
        };
        mode == Some(DocumentMode::Quirks)
    }

    fn retained_nodes(&self) -> Result<Vec<NodeId>, OutOfMemory> {
        let mut roots = Vec::new();
        if let ScriptTreeState::Parsing(parser) =
            &*self.state.try_borrow().map_err(|_| OutOfMemory)?
        {
            let pending = self
                .pending_classics
                .try_borrow()
                .map_err(|_| OutOfMemory)?;
            for node in self
                .current_script
                .get()
                .into_iter()
                .chain(parser.retained_nodes())
                .chain(pending.iter().copied())
            {
                roots.try_reserve(1).map_err(|_| OutOfMemory)?;
                roots.push(node);
            }
        }
        Ok(roots)
    }

    fn current_script(&self) -> Option<NodeId> {
        self.current_script.get()
    }

    fn allows_adoption_from_main(&self) -> bool {
        matches!(&*self.state.borrow(), ScriptTreeState::Ready(_))
    }
}

/// A completed document with a single script agent and a host-driven turn scheduler.
///
/// The host explicitly queues scripts and pumps turns. The document and agent
/// move together, with the document's existing token shared by the VM.
pub struct ScriptedDocument<C = SystemClock> {
    document: Rc<ScriptTree>,
    cancellation: CancellationToken,
    agent: Result<Agent, ScriptStop>,
    pending: Rc<WindowWork>,
    animation_errors: u64,
    inline_script_errors: u64,
    external_script_errors: u64,
    module_script_errors: u64,
    module_map: RefCell<HashMap<ModuleKey, ModuleRoots>>,
    pending_modules: Vec<HostRoot>,
    rendering: Rendering,
    clock: C,
    limits: TaskLimits,
}

impl EngineDocument {
    /// Give this document a script agent with the requested limits.
    ///
    /// The conversion consumes the inert document. If initialization fails, its
    /// scope is closed as when dropping any document owner.
    pub fn into_scripted(self, limits: TaskLimits) -> Result<ScriptedDocument, ScriptStop> {
        self.into_scripted_with_clock(limits, SystemClock::default())
    }

    /// Give this document a script agent and a host-selected monotonic clock.
    /// The clock governs timer eligibility and rendering timestamps. JavaScript
    /// reads civil time separately.
    pub fn into_scripted_with_clock<C: Clock>(
        self,
        limits: TaskLimits,
        clock: C,
    ) -> Result<ScriptedDocument<C>, ScriptStop> {
        let cancellation = self.cancellation().clone();
        ScriptedDocument::new(ScriptTreeState::Ready(self), cancellation, limits, clock)
    }
}

impl<C: Clock> ScriptedDocument<C> {
    pub(crate) fn with_parser(
        mut parser: HtmlDocumentParser,
        cancellation: CancellationToken,
        limits: TaskLimits,
        clock: C,
    ) -> Result<Self, ScriptStop> {
        // Publish the document to the bindings before scripts can acquire wrappers.
        parser
            .live_document_mut()
            .expect("new parser owns a document");
        Self::new(
            ScriptTreeState::Parsing(Box::new(parser)),
            cancellation,
            limits,
            clock,
        )
    }

    fn new(
        state: ScriptTreeState,
        cancellation: CancellationToken,
        limits: TaskLimits,
        clock: C,
    ) -> Result<Self, ScriptStop> {
        if cancellation.is_cancelled() {
            return Err(ScriptStop::DocumentCancelled);
        }
        let mut agent =
            Agent::with_heap_limit(limits.heap_bytes).map_err(|_| ScriptStop::OutOfMemory)?;
        agent.set_cancellation(Some(cancellation.clone()));
        agent.set_can_block(false);
        dom_exception::install(&mut agent).map_err(|_| ScriptStop::OutOfMemory)?;
        let document = Rc::new(ScriptTree {
            state: RefCell::new(state),
            current_script: Cell::new(None),
            pending_classics: RefCell::default(),
        });
        let owner = document
            .read()
            .expect("new script tree owns its document")
            .id();
        let pending = Rc::new(WindowWork {
            queue: RefCell::new(ScriptQueue::new(owner, limits)),
        });
        page::install_with_scheduler(&mut agent, document.clone(), pending.clone())
            .map_err(|_| ScriptStop::OutOfMemory)?;
        Ok(Self {
            document,
            cancellation,
            agent: Ok(agent),
            pending,
            animation_errors: 0,
            inline_script_errors: 0,
            external_script_errors: 0,
            module_script_errors: 0,
            module_map: RefCell::default(),
            pending_modules: Vec::new(),
            rendering: Rendering::new(owner),
            clock,
            limits,
        })
    }
    /// Inspect the parsed document and its lifetime token.
    pub fn document(&self) -> Ref<'_, EngineDocument> {
        Ref::map(self.document.state.borrow(), |state| match state {
            ScriptTreeState::Ready(document) => document,
            ScriptTreeState::Parsing(_) => {
                unreachable!("the host receives only finished documents")
            }
        })
    }

    pub(crate) fn with_active_parser<T>(
        &self,
        operation: impl FnOnce(&mut HtmlDocumentParser) -> T,
    ) -> T {
        let mut state = self.document.state.borrow_mut();
        let ScriptTreeState::Parsing(parser) = &mut *state else {
            unreachable!("only pending loads can drive a parser");
        };
        operation(parser)
    }

    pub(crate) fn finish_parsing(&mut self, scope: DocumentScope) -> Result<(), HtmlParseError> {
        let parsed = self.with_active_parser(HtmlDocumentParser::finish)?;
        *self.document.state.borrow_mut() =
            ScriptTreeState::Ready(EngineDocument::new(scope, parsed));
        Ok(())
    }

    pub(crate) fn run_inline_script(
        &mut self,
        element: NodeId,
        source: &Source,
    ) -> Result<(), ScriptStop> {
        if matches!(
            self.run_parser_classic(element, source)?,
            ScriptTurn::Failed(_)
        ) {
            self.inline_script_errors = self.inline_script_errors.saturating_add(1);
        }
        Ok(())
    }

    pub(crate) fn retain_external_classic(&self, element: NodeId) -> Result<(), ScriptStop> {
        let mut roots = self.document.pending_classics.borrow_mut();
        roots.try_reserve(1).map_err(|_| ScriptStop::OutOfMemory)?;
        let inserted = roots.insert(element);
        debug_assert!(inserted, "one request per parser-inserted element");
        Ok(())
    }

    pub(crate) fn run_external_classic(
        &mut self,
        element: NodeId,
        source: Option<&Source>,
    ) -> Result<(), ScriptStop> {
        let turn = source
            .map(|source| self.run_parser_classic(element, source))
            .transpose();
        self.document.pending_classics.borrow_mut().remove(&element);
        if !matches!(turn?, Some(ScriptTurn::Completed)) {
            self.note_external_script_error();
        }
        Ok(())
    }

    fn run_parser_classic(
        &mut self,
        element: NodeId,
        source: &Source,
    ) -> Result<ScriptTurn, ScriptStop> {
        self.check_live()?;
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        agent.set_step_budget(Some(self.limits.steps_per_turn));
        self.document.current_script.set(Some(element));
        let result = agent.evaluate_source(source).map(|_| ());
        self.document.current_script.set(None);
        self.finish_script_turn(result)
    }

    /// Read and clear recoverable errors from parser-inserted inline classics.
    pub fn take_inline_script_error_count(&mut self) -> u64 {
        std::mem::take(&mut self.inline_script_errors)
    }

    pub(crate) fn note_external_script_error(&mut self) {
        self.external_script_errors = self.external_script_errors.saturating_add(1);
    }

    /// Read and clear recoverable preparation, fetch or evaluation errors from external classics.
    pub fn take_external_script_error_count(&mut self) -> u64 {
        std::mem::take(&mut self.external_script_errors)
    }

    pub(crate) fn run_module_graph(
        &mut self,
        graph: &ModuleGraph<ModuleKey>,
        limits: &GraphLimits,
        sources: &LocalModules,
    ) -> Result<(), ScriptStop> {
        self.check_live()?;
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        agent.set_step_budget(Some(self.limits.steps_per_turn));
        let cache = &self.module_map;
        let (entry, _) = graph.module(0).expect("module graph has an entry");
        let cached = cache.borrow().get(entry).copied();
        let evaluation = if let Some(root) = cached {
            Ok(root.objects(agent))
        } else {
            agent.start_cached_module_evaluation(
                graph,
                limits,
                |key| {
                    sources
                        .base(key)
                        .map(|url| JsStr::from_text(&url.serialize()))
                },
                |agent, key| cache.borrow().get(key).map(|root| root.objects(agent)),
                |agent, index, namespace, completion| {
                    let (key, _) = graph.module(index).expect("evaluated graph module");
                    store_module(agent, &mut cache.borrow_mut(), key, namespace, completion)?;
                    Ok(())
                },
            )
        };
        match evaluation {
            Ok((namespace, completion)) => {
                let root = store_module(
                    agent,
                    &mut self.module_map.borrow_mut(),
                    entry,
                    namespace,
                    completion,
                )
                .map_err(|_| ScriptStop::OutOfMemory)?;
                self.pending_modules
                    .try_reserve(1)
                    .map_err(|_| ScriptStop::OutOfMemory)?;
                self.pending_modules.push(root.completion);
            }
            Err(ModuleEvalError::Runtime(Abrupt::OutOfMemory)) => {
                return Err(self.stop(ScriptStop::OutOfMemory));
            }
            Err(ModuleEvalError::Runtime(Abrupt::Cancelled)) => {
                return Err(self.stop_after_cancellation());
            }
            Err(ModuleEvalError::Graph(GraphError::Cancelled)) => {
                return Err(self.stop(ScriptStop::DocumentCancelled));
            }
            Err(_) => self.note_module_script_error(),
        }
        self.finish_script_turn(Ok(()))?;
        Ok(())
    }

    pub(crate) fn has_pending_modules(&self) -> bool {
        !self.pending_modules.is_empty()
    }

    pub(crate) fn note_module_script_error(&mut self) {
        self.module_script_errors = self.module_script_errors.saturating_add(1);
    }

    /// Read and clear recoverable module graph or evaluation errors.
    pub fn take_module_script_error_count(&mut self) -> u64 {
        std::mem::take(&mut self.module_script_errors)
    }

    /// Read and clear the number of listener exceptions reported by the page.
    pub fn take_listener_error_count(&self) -> u64 {
        self.agent
            .as_ref()
            .map_or(0, page::take_listener_error_count)
    }

    /// Read and clear uncaught exceptions from animation frame callbacks.
    pub fn take_animation_error_count(&mut self) -> u64 {
        std::mem::take(&mut self.animation_errors)
    }

    /// Queue a host-supplied script after checking both pending-work limits.
    pub fn queue_script(&mut self, source: Source) -> Result<(), ScriptQueueError> {
        self.check_live().map_err(ScriptQueueError::Stopped)?;
        self.pending.queue.borrow_mut().queue(Task::Script(source))
    }

    /// Queue host input for a connected target in this document.
    ///
    /// The host supplies the hit-tested pointer or selected keyboard target.
    /// Targets detached or adopted before delivery are skipped.
    pub fn queue_input(
        &mut self,
        target: NodeId,
        event: InputEvent,
    ) -> Result<(), ScriptQueueError> {
        self.check_live().map_err(ScriptQueueError::Stopped)?;
        if !page::connected_input_target(self.document().parsed().document(), target) {
            return Err(ScriptQueueError::InvalidInputTarget);
        }
        self.pending
            .queue
            .borrow_mut()
            .queue(Task::Input { target, event })
    }

    /// Access the host-selected timer clock, for example to advance a manual clock.
    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Queue a one-shot script timer that becomes eligible after `delay`.
    /// A zero delay makes it eligible on a later host turn.
    pub fn schedule_script(
        &mut self,
        delay: Duration,
        source: Source,
    ) -> Result<TimerId, ScriptQueueError> {
        self.check_live().map_err(ScriptQueueError::Stopped)?;
        let deadline = self
            .clock
            .now()
            .checked_add(delay)
            .ok_or(ScriptQueueError::DeadlineOverflow)?;
        self.pending.queue.borrow_mut().schedule(deadline, source)
    }

    /// Remove a pending timer and release its queue budget.
    /// Returns `false` for a foreign timer or one that has already run or been removed.
    pub fn cancel_timer(&mut self, timer: TimerId) -> Result<bool, ScriptStop> {
        self.check_live()?;
        Ok(self.pending.queue.borrow_mut().cancel(timer))
    }

    /// Delay until the earliest pending timer, or zero if already eligible.
    /// Returns `None` if no timers are pending.
    pub fn time_until_next_timer(&mut self) -> Result<Option<Duration>, ScriptStop> {
        self.check_live()?;
        Ok(self.pending.queue.borrow().delay(self.clock.now()))
    }

    /// Offer this document a rendering opportunity at the current clock time.
    /// Offers made before the next rendering turn coalesce to the latest time.
    pub fn offer_rendering_opportunity(&mut self) -> Result<(), ScriptStop> {
        self.check_live()?;
        self.rendering.offer(self.clock.now());
        Ok(())
    }

    /// Pump one script, timer, input or rendering turn.
    ///
    /// A ready task and an offered frame alternate when both can
    /// run. Each task completes its checkpoint before a later rendering turn.
    /// Animation callbacks and their checkpoints run before the renderer.
    /// The renderer receives the completed document and a ticket for its output.
    pub fn run_next_with_render<T>(
        &mut self,
        render: impl FnOnce(&EngineDocument, RenderFrame) -> T,
    ) -> Result<Option<DocumentTurn<T>>, ScriptStop> {
        self.check_live()?;
        if !self.rendering.has_priority() {
            if let Some(turn) = self.run_script()? {
                return Ok(Some(DocumentTurn::Script(turn)));
            }
        }
        let Some(frame) = self.rendering.begin().map_err(|reason| self.stop(reason))? else {
            return Ok(None);
        };
        self.run_animation_callbacks(frame)?;
        let output = render(&self.document(), frame);
        self.check_live()?;
        Ok(Some(DocumentTurn::Rendered { frame, output }))
    }

    /// Accept the current frame once before presenting its output.
    /// A later rendering turn invalidates earlier tickets.
    pub fn accept_rendered_frame(&mut self, frame: RenderFrame) -> Result<bool, ScriptStop> {
        self.check_live()?;
        Ok(self.rendering.accept(frame))
    }

    /// Run one eligible task and drain its promise jobs before a later task.
    ///
    /// A terminal failure discards pending tasks and releases the JS agent.
    /// Returns `Ok(None)` when there is no eligible script, timer or input. Use
    /// [`Self::run_next_with_render`] to service rendering offers as well.
    pub fn run_next(&mut self) -> Result<Option<ScriptTurn>, ScriptStop> {
        self.check_live()?;
        self.run_script()
    }

    fn run_script(&mut self) -> Result<Option<ScriptTurn>, ScriptStop> {
        let Some(task) = self.pending.queue.borrow_mut().pop_ready(self.clock.now()) else {
            return Ok(None);
        };

        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        agent.set_step_budget(Some(self.limits.steps_per_turn));
        let result = match task {
            Task::Script(source) => agent.evaluate_source(&source).map(|_| ()),
            Task::Timer(timer) => timer.run(agent, &self.pending),
            Task::Input { target, event } => {
                page::deliver_input(agent, target, &event).map_err(EvalError::from)
            }
        };
        self.finish_script_turn(result).map(Some)
    }

    fn finish_script_turn(
        &mut self,
        result: Result<(), EvalError>,
    ) -> Result<ScriptTurn, ScriptStop> {
        let turn = match result {
            Ok(_) => ScriptTurn::Completed,
            Err(EvalError::Parse(error)) => ScriptTurn::Failed(ScriptFailure::Parse(error)),
            Err(EvalError::Compile(error)) => ScriptTurn::Failed(ScriptFailure::Compile(error)),
            Err(EvalError::Throw(_)) => ScriptTurn::Failed(ScriptFailure::UncaughtException),
            Err(EvalError::OutOfMemory) => return Err(self.stop(ScriptStop::OutOfMemory)),
            Err(EvalError::Cancelled) => return Err(self.stop_after_cancellation()),
            Err(EvalError::SourceTooLong) => unreachable!("Source already bounds its length"),
        };
        self.checkpoint()?;
        self.pending
            .queue
            .borrow_mut()
            .publish_timers(self.clock.now());
        self.rendering.after_script();
        Ok(turn)
    }

    fn run_animation_callbacks(&mut self, frame: RenderFrame) -> Result<(), ScriptStop> {
        let cursor = self.pending.queue.borrow().frame_cursor();
        let timestamp = Value::number(frame.timestamp().as_secs_f64() * 1_000.0);
        self.agent
            .as_mut()
            .map_err(|reason| *reason)?
            .set_step_budget(Some(self.limits.steps_per_turn));
        loop {
            let root = self.pending.queue.borrow_mut().pop_frame(cursor);
            let Some(root) = root else {
                break;
            };
            let agent = self.agent.as_mut().map_err(|reason| *reason)?;
            let callback = agent.rooted(root).expect("retained animation callback");
            let result = agent
                .check_native_work()
                .and_then(|()| agent.call(callback, Value::UNDEFINED, &[timestamp]));
            agent.unroot(root);
            match result {
                Ok(_) => {}
                Err(Abrupt::Throw(_)) => {
                    self.animation_errors = self.animation_errors.saturating_add(1)
                }
                Err(Abrupt::OutOfMemory) => return Err(self.stop(ScriptStop::OutOfMemory)),
                Err(Abrupt::Cancelled) => return Err(self.stop_after_cancellation()),
            }
            self.checkpoint()?;
        }
        self.pending
            .queue
            .borrow_mut()
            .publish_timers(self.clock.now());
        Ok(())
    }

    fn checkpoint(&mut self) -> Result<(), ScriptStop> {
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        match agent.run_jobs_limited(self.limits.jobs_per_checkpoint) {
            Ok(true) => {
                // Release the rejection report's roots at the checkpoint.
                // Web rejection-event dispatch belongs to the later event layer.
                agent.take_rejection_events();
                self.pending_modules.retain(|&root| {
                    let promise = agent
                        .rooted(root)
                        .and_then(Value::as_object)
                        .expect("retained module completion");
                    match agent.promise_rejected(promise) {
                        Some(rejected) => {
                            self.module_script_errors = self
                                .module_script_errors
                                .saturating_add(u64::from(rejected));
                            false
                        }
                        None => true,
                    }
                });
                self.check_live()
            }
            Ok(false) => Err(self.stop(ScriptStop::JobLimit)),
            Err(Abrupt::OutOfMemory) => Err(self.stop(ScriptStop::OutOfMemory)),
            Err(Abrupt::Cancelled) => Err(self.stop_after_cancellation()),
            Err(Abrupt::Throw(_)) => unreachable!("the job runner handles thrown values"),
        }
    }

    fn check_live(&mut self) -> Result<(), ScriptStop> {
        if self.cancellation.is_cancelled() {
            return Err(self.stop(ScriptStop::DocumentCancelled));
        }
        self.agent.as_ref().map(|_| ()).map_err(|reason| *reason)
    }

    fn stop_after_cancellation(&mut self) -> ScriptStop {
        if self.cancellation.is_cancelled() {
            self.stop(ScriptStop::DocumentCancelled)
        } else {
            self.stop(ScriptStop::StepLimit)
        }
    }

    fn stop(&mut self, reason: ScriptStop) -> ScriptStop {
        self.agent = Err(reason);
        self.pending.queue.borrow_mut().clear();
        self.module_map.get_mut().clear();
        self.pending_modules.clear();
        self.rendering.clear();
        reason
    }
}

#[derive(Clone, Copy)]
struct ModuleRoots {
    namespace: HostRoot,
    completion: HostRoot,
}

impl ModuleRoots {
    fn objects(self, agent: &Agent) -> (ObjectId, ObjectId) {
        let object = |root| {
            agent
                .rooted(root)
                .and_then(Value::as_object)
                .expect("module map root")
        };
        (object(self.namespace), object(self.completion))
    }
}

fn store_module(
    agent: &mut Agent,
    cache: &mut HashMap<ModuleKey, ModuleRoots>,
    key: &ModuleKey,
    namespace: ObjectId,
    completion: ObjectId,
) -> Result<ModuleRoots, OutOfMemory> {
    if let Some(&existing) = cache.get(key) {
        return Ok(existing);
    }
    cache.try_reserve(1).map_err(|_| OutOfMemory)?;
    let namespace = agent.root(Value::object(namespace))?;
    match agent.root(Value::object(completion)) {
        Ok(completion) => {
            let roots = ModuleRoots {
                namespace,
                completion,
            };
            cache.insert(key.clone(), roots);
            Ok(roots)
        }
        Err(error) => {
            agent.unroot(namespace);
            Err(error)
        }
    }
}
