//! WEB-01 document-owned script tasks and microtask checkpoints.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::Duration;

use rustic_base::{CancellationToken, Clock, SystemClock};
use rustic_bindings::dom_exception;
use rustic_bindings::page::{
    self, ComputedStyleSnapshot, LayoutReadError, LifecycleEvent, PageTree,
};
use rustic_dom::{Document, NodeId};
use rustic_html::{DocumentMode, HtmlDocumentParser, ParseError as HtmlParseError};
use rustic_js::module::{GraphError, GraphLimits, ModuleEvalError, ModuleGraph};
use rustic_js::runtime::{
    Abrupt, Agent, DEFAULT_HEAP_BYTES, HostRoot, JsResult, JsStr, ObjectId, OutOfMemory,
    RejectionEvent, Value,
};
use rustic_js::{CompileError, EvalError, ParseError, Source};

use crate::frames::{RenderFrame, Rendering};
use crate::layout_reads::{LayoutGeometry, LayoutReads, LayoutSource};
use crate::lifecycle::DocumentScope;
use crate::local_modules::{LocalModules, ModuleKey};
use crate::tasks::{ResourceCompletion, ScriptQueue, Task, TimerId};
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
    /// A prevalidated host resource was delivered at the document boundary.
    /// On failure, the applied-input revision and frame ticket remain intact.
    ResourceCompletion {
        /// Whether the staged completion was committed.
        applied: bool,
    },
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
    Ready(Box<EngineDocument>),
}

#[derive(Debug)]
struct ScriptTree {
    state: RefCell<ScriptTreeState>,
    current_script: Cell<Option<NodeId>>,
    // Keep root checks in a deterministic order under the native work budget.
    pending_classics: RefCell<Vec<NodeId>>,
    layout: RefCell<LayoutReads>,
    /// Scroll positions (ADR 0064): the viewport's, and scroll containers'
    /// by element.
    scroll: RefCell<ScrollState>,
}

/// The document's scroll positions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollState {
    /// The viewport's position in CSS pixels.
    pub viewport: (f64, f64),
    /// Scroll containers' positions, by element.
    pub elements: HashMap<NodeId, (f64, f64)>,
}

impl ScriptTree {
    /// `None` for the viewport: the document element in no-quirks mode
    /// stands for it (CSSOM View `scrollTop`); other targets stay.
    fn scroll_target(&self, node: Option<NodeId>) -> Option<NodeId> {
        let node = node?;
        let document = self.read()?;
        let root = document.root();
        let document_element = document.children(root).ok()?.find(|&child| {
            document
                .node(child)
                .is_ok_and(|n| matches!(n.kind(), rustic_dom::NodeKind::Element(_)))
        });
        (document_element != Some(node) || self.quirks_mode()).then_some(node)
    }
    fn read_layout<T>(
        &self,
        empty: T,
        read: impl FnOnce(&LayoutGeometry) -> Result<T, LayoutReadError>,
    ) -> Result<T, LayoutReadError> {
        let document = self.read().ok_or(LayoutReadError::Unavailable)?;
        let mode = self.mode().ok_or(LayoutReadError::Unavailable)?;
        self.layout.borrow_mut().read(&document, mode, empty, read)
    }

    fn mode(&self) -> Option<DocumentMode> {
        match &*self.state.borrow() {
            ScriptTreeState::Parsing(parser) => parser.live_document_mode(),
            ScriptTreeState::Ready(owner) => Some(owner.parsed().mode()),
        }
    }

    fn parser_roots<'a>(
        &'a self,
        parser: &'a HtmlDocumentParser,
        pending: &'a [NodeId],
    ) -> impl Iterator<Item = NodeId> + 'a {
        self.current_script
            .get()
            .into_iter()
            .chain(parser.retained_nodes())
            .chain(pending.iter().copied())
    }
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
        self.mode() == Some(DocumentMode::Quirks)
    }

    fn encoding_name(&self) -> &'static str {
        let encoding = match &*self.state.borrow() {
            ScriptTreeState::Parsing(parser) => parser.encoding(),
            ScriptTreeState::Ready(owner) => owner.parsed().encoding(),
        };
        encoding.name()
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
            for node in self.parser_roots(parser, &pending) {
                roots.try_reserve(1).map_err(|_| OutOfMemory)?;
                roots.push(node);
            }
        }
        Ok(roots)
    }

    fn current_script(&self) -> Option<NodeId> {
        self.current_script.get()
    }

    fn offset_width(&self, id: NodeId) -> Result<i32, LayoutReadError> {
        self.read_layout(0, |geometry| {
            Ok(geometry.offset_widths.get(&id).copied().unwrap_or(0))
        })
    }

    fn client_rect(&self, id: NodeId) -> Result<[f64; 4], LayoutReadError> {
        let scroll = self.scroll.borrow().clone();
        self.read_layout([0.0; 4], |geometry| {
            let mut rect = geometry
                .client_rects
                .get(&id)
                .copied()
                .unwrap_or(Some([0.0; 4]))
                .ok_or(LayoutReadError::Unavailable)?;
            // Client rectangles are relative to the scrolled viewport.
            if let Some((viewport, containers)) = geometry.scroll_chains.get(&id) {
                if *viewport {
                    rect[0] -= scroll.viewport.0;
                    rect[1] -= scroll.viewport.1;
                }
                for container in containers {
                    let (x, y) = scroll.elements.get(container).copied().unwrap_or_default();
                    rect[0] -= x;
                    rect[1] -= y;
                }
            }
            Ok(rect)
        })
    }

    fn scroll_position(&self, node: Option<NodeId>) -> (f64, f64) {
        let scroll = self.scroll.borrow();
        match self.scroll_target(node) {
            None => scroll.viewport,
            Some(node) => scroll.elements.get(&node).copied().unwrap_or_default(),
        }
    }

    fn scroll_to(&self, node: Option<NodeId>, x: f64, y: f64) -> Result<bool, LayoutReadError> {
        let target = self.scroll_target(node);
        let range = self.read_layout(None, |geometry| {
            Ok(match target {
                None => Some(geometry.viewport_scroll_range),
                Some(node) => geometry.scroll_ranges.get(&node).copied(),
            })
        })?;
        // Not a scroll container (or no layout): nothing scrolls.
        let Some([min_x, min_y, max_x, max_y]) = range else {
            return Ok(false);
        };
        let clamp = |value: f64, min: f64, max: f64| {
            if value.is_finite() {
                value.clamp(min, max.max(min))
            } else {
                0.0
            }
        };
        let next = (clamp(x, min_x, max_x), clamp(y, min_y, max_y));
        let mut scroll = self.scroll.borrow_mut();
        let slot = match target {
            None => &mut scroll.viewport,
            Some(node) => scroll.elements.entry(node).or_default(),
        };
        let changed = *slot != next;
        *slot = next;
        Ok(changed)
    }

    fn box_sizes(&self, id: NodeId) -> Result<Option<page::BoxSizes>, LayoutReadError> {
        self.read_layout(None, |geometry| Ok(geometry.box_sizes.get(&id).copied()))
    }

    fn computed_style(&self, id: NodeId) -> Result<ComputedStyleSnapshot, LayoutReadError> {
        if !self.layout.borrow().has_source() {
            return Err(LayoutReadError::Unavailable);
        }
        self.read_layout(ComputedStyleSnapshot::default(), |geometry| {
            geometry
                .computed_styles
                .get(&id)
                .cloned()
                .ok_or(LayoutReadError::Unavailable)
        })
    }

    fn allows_adoption_from_main(&self, agent: &mut Agent, subtree: NodeId) -> JsResult<bool> {
        let state = self.state.try_borrow().map_err(|_| OutOfMemory)?;
        let ScriptTreeState::Parsing(parser) = &*state else {
            return Ok(true);
        };
        let Some(document) = parser.live_document() else {
            return Ok(false);
        };
        let pending = self
            .pending_classics
            .try_borrow()
            .map_err(|_| OutOfMemory)?;
        for root in self.parser_roots(parser, &pending) {
            let mut current = Some(root);
            while let Some(id) = current {
                agent.check_native_work()?;
                if id == subtree {
                    return Ok(false);
                }
                current = document.node(id).ok().and_then(|node| {
                    node.parent()
                        .or(node.template_host())
                        .or(node.shadow_host())
                });
            }
        }
        Ok(true)
    }
}

/// A completed document with a single script agent and a host-driven turn scheduler.
///
/// The host explicitly queues scripts and pumps turns. The document and agent
/// move together, with the document's existing token shared by the VM.
/// An element's layout as developer tools show it ([`ScriptedDocument::node_layout`]).
#[derive(Clone, Debug, Default)]
pub struct NodeLayout {
    /// Computed style, for a styled element.
    pub style: Option<ComputedStyleSnapshot>,
    /// Border-box bounds `[x, y, width, height]` in CSS pixels, for an
    /// element with a box and a projectable transform.
    pub client_rect: Option<[f64; 4]>,
    /// Content and border box sizes, for an element with a block or
    /// atomic box.
    pub box_sizes: Option<page::BoxSizes>,
}

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
        ScriptedDocument::new(
            ScriptTreeState::Ready(Box::new(self)),
            cancellation,
            limits,
            clock,
        )
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
            layout: RefCell::default(),
            scroll: RefCell::default(),
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
            ScriptTreeState::Ready(document) => document.as_ref(),
            ScriptTreeState::Parsing(_) => {
                unreachable!("the host receives only finished documents")
            }
        })
    }

    /// HostEnsureCanCompileStrings for this document's agent (CSP
    /// `'unsafe-eval'`).
    /// The tree parsed so far (the whole tree once parsing has ended), for
    /// a host discovering resources ahead of the parser. `None` while a
    /// script holds the tree.
    pub fn live_document(&self) -> Option<Ref<'_, Document>> {
        self.document.read()
    }

    /// The document's mode, once the parser has decided it.
    pub fn live_document_mode(&self) -> Option<DocumentMode> {
        self.document.mode()
    }

    pub fn set_string_compilation_allowed(&mut self, allowed: bool) -> Result<(), ScriptStop> {
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        agent.set_string_compilation_allowed(allowed);
        Ok(())
    }

    /// Give the document `fetch()`, backed by the host's network session.
    pub fn set_fetch_host(
        &mut self,
        host: std::rc::Rc<dyn crate::FetchHost>,
    ) -> Result<(), ScriptStop> {
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        page::attach_fetch(agent, host).map_err(|_| ScriptStop::OutOfMemory)
    }

    /// Give the document `localStorage` and `sessionStorage` (HTML §12.2)
    /// backed by the host's areas for its storage key.
    pub fn set_storage_host(
        &mut self,
        host: std::rc::Rc<dyn crate::StorageHost>,
    ) -> Result<(), ScriptStop> {
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        page::attach_storage(agent, host).map_err(|_| ScriptStop::OutOfMemory)
    }

    /// Supply the bounded layout pipeline serving synchronous script reads.
    /// Replacing it also invalidates any outstanding render ticket.
    pub fn set_layout_source(&mut self, source: Box<dyn LayoutSource>) -> Result<(), ScriptStop> {
        self.check_live()?;
        self.document.layout.borrow_mut().set_source(source);
        Ok(())
    }

    /// Publish a change to the renderer's viewport or applied resource data.
    /// Call at a host turn boundary once the new inputs are available.
    pub fn rendering_inputs_changed(&mut self) -> Result<(), ScriptStop> {
        self.check_live()?;
        self.document.layout.borrow_mut().inputs_changed();
        Ok(())
    }

    /// Commit a completed stylesheet, image or font result into this document.
    ///
    /// The host must stage the result before entering this method. `apply`
    /// publishes only a successfully validated resource into the renderer's
    /// applied state; it must not run JavaScript, perform a fetch, or mutate
    /// state before returning an error. A successful commit invalidates the
    /// shared style/layout snapshot and any outstanding frame ticket, then
    /// offers a rendering opportunity. A failed commit does none of those.
    ///
    /// The outer error is a stopped document; the inner error comes from the
    /// host's resource application. Neither the host nor scripts can observe
    /// the new resource under an old rendering-input revision after success.
    pub fn apply_resource_completion<T, E>(
        &mut self,
        apply: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ScriptStop> {
        self.check_live()?;
        let result = apply();
        if result.is_ok() {
            self.check_live()?;
            self.document.layout.borrow_mut().inputs_changed();
            self.rendering.offer(self.clock.now());
        }
        Ok(result)
    }

    /// Read and clear the number of layout flushes attempted by script.
    pub fn take_layout_flush_count(&mut self) -> u64 {
        self.document.layout.borrow_mut().take_flush_count()
    }

    /// Drop the realm and its pending work, keeping the modified document.
    pub fn into_document(self) -> EngineDocument {
        let Self {
            document, agent, ..
        } = self;
        drop(agent);
        let tree = Rc::try_unwrap(document)
            .unwrap_or_else(|_| unreachable!("only the realm shares the document tree"));
        match tree.state.into_inner() {
            ScriptTreeState::Ready(document) => *document,
            ScriptTreeState::Parsing(_) => {
                unreachable!("the host receives only finished documents")
            }
        }
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
            ScriptTreeState::Ready(Box::new(EngineDocument::new(scope, parsed)));
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
        let index = roots.binary_search(&element);
        debug_assert!(index.is_err(), "one request per parser-inserted element");
        if let Err(index) = index {
            roots.try_reserve(1).map_err(|_| ScriptStop::OutOfMemory)?;
            roots.insert(index, element);
        }
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
        if !matches!(turn?, Some(ScriptTurn::Completed)) {
            self.note_external_script_error();
        }
        self.finish_element_script(element, Some(source.is_some()))
    }

    pub(crate) fn finish_element_script(
        &mut self,
        element: NodeId,
        loaded: Option<bool>,
    ) -> Result<(), ScriptStop> {
        let result = match loaded {
            Some(true) => self.deliver_lifecycle(LifecycleEvent::ScriptLoad(element)),
            Some(false) => self.deliver_lifecycle(LifecycleEvent::ScriptError(element)),
            None => Ok(()),
        };
        let mut roots = self.document.pending_classics.borrow_mut();
        if let Ok(index) = roots.binary_search(&element) {
            roots.remove(index);
        }
        result
    }

    pub(crate) fn deliver_lifecycle(&mut self, event: LifecycleEvent) -> Result<(), ScriptStop> {
        self.check_live()?;
        self.begin_turn()?;
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        let result = page::deliver_lifecycle(agent, event).map_err(EvalError::from);
        self.finish_script_turn(result).map(|_| ())
    }

    fn run_parser_classic(
        &mut self,
        element: NodeId,
        source: &Source,
    ) -> Result<ScriptTurn, ScriptStop> {
        self.check_live()?;
        self.begin_turn()?;
        // HTML "</script>" step: a microtask checkpoint before the script,
        // which delivers mutation records for the parser's insertions.
        self.checkpoint()?;
        self.document.current_script.set(Some(element));
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
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
        self.begin_turn()?;
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
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

    /// Whether the window has transient user activation (HTML §6.4.3).
    pub fn has_transient_activation(&self) -> bool {
        self.agent
            .as_ref()
            .is_ok_and(page::has_transient_activation)
    }

    /// HTML "consume user activation" for a host API that needs a gesture
    /// (a download, a popup, a permission prompt); returns whether there
    /// was transient activation to spend.
    pub fn consume_user_activation(&self) -> bool {
        self.agent.as_ref().is_ok_and(page::consume_user_activation)
    }

    /// Read and clear the number of listener exceptions reported by the page.
    pub fn take_listener_error_count(&self) -> u64 {
        self.agent
            .as_ref()
            .map_or(0, page::take_listener_error_count)
    }

    /// The page's script agent, for developer tools (ADR 0072): attaching
    /// a debugger, setting breakpoints, reading a stopped stack. Running
    /// script through it bypasses the turn bookkeeping; use
    /// [`Self::evaluate_for_devtools`] for that.
    pub fn devtools_agent(&mut self) -> Result<&mut Agent, ScriptStop> {
        self.agent.as_mut().map_err(|reason| *reason)
    }

    /// Run `source` as a host script turn for developer tools and describe
    /// its completion value (`Ok`) or what it threw or why it did not parse
    /// (`Err`), without running page code to describe either. A throw is
    /// the tool's result, not an uncaught page error: no `error` event.
    pub fn evaluate_for_devtools(
        &mut self,
        source: &Source,
    ) -> Result<Result<String, String>, ScriptStop> {
        self.check_live()?;
        self.begin_turn()?;
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        let outcome = match agent.evaluate_source(source) {
            Ok(value) => Ok(agent.describe_value(value)),
            Err(EvalError::Throw(value)) => Err(agent.describe_value(value)),
            Err(EvalError::Parse(error)) => Err(format!("SyntaxError: {error}")),
            Err(EvalError::Compile(error)) => Err(format!("SyntaxError: {error:?}")),
            Err(stop) => {
                self.finish_script_turn(Err(stop))?;
                return Err(ScriptStop::DocumentCancelled);
            }
        };
        self.finish_script_turn(Ok(()))?;
        Ok(outcome)
    }

    /// One element's computed style, client rectangle and box sizes from
    /// the current layout, for developer tools; laid out first if stale.
    pub fn node_layout(&mut self, node: NodeId) -> Result<NodeLayout, LayoutReadError> {
        self.document.layout.borrow_mut().begin_turn();
        self.document
            .read_layout(NodeLayout::default(), |geometry| {
                Ok(NodeLayout {
                    style: geometry.computed_styles.get(&node).cloned(),
                    client_rect: geometry.client_rects.get(&node).copied().flatten(),
                    box_sizes: geometry.box_sizes.get(&node).copied(),
                })
            })
    }

    /// Take the page's console messages and the count dropped because the
    /// bounded buffer was full.
    pub fn take_console_messages(&self) -> page::ConsoleLog {
        self.agent
            .as_ref()
            .map_or_else(|_| page::ConsoleLog::default(), page::take_console_messages)
    }

    /// Focus and form-control state that lives in script, for the
    /// accessibility tree (ADR 0073). A stopped page reports none.
    pub fn accessibility_state(&mut self) -> rustic_a11y::LiveState {
        let Ok(agent) = self.agent.as_mut() else {
            return rustic_a11y::LiveState::default();
        };
        let snapshot = page::accessible_controls(agent);
        let mut live = rustic_a11y::LiveState {
            focused: snapshot.focused,
            ..rustic_a11y::LiveState::default()
        };
        for control in snapshot.controls {
            let checked = control.checked.map(|checked| {
                if control.indeterminate {
                    rustic_a11y::Checked::Mixed
                } else if checked {
                    rustic_a11y::Checked::True
                } else {
                    rustic_a11y::Checked::False
                }
            });
            live.controls.insert(
                control.node,
                rustic_a11y::ControlState {
                    value: control.value,
                    checked,
                    invalid: control.invalid,
                    selection: control.selection,
                },
            );
        }
        live
    }

    /// The document's scroll positions, as script left them (ADR 0064).
    pub fn scroll_state(&self) -> ScrollState {
        self.document.scroll.borrow().clone()
    }

    /// The user scrolled the viewport to `(x, y)` CSS pixels (TD-182):
    /// clamped to the current layout's scroll range like
    /// `window.scrollTo`, with its `scroll` event at the next rendering
    /// opportunity. Returns whether the viewport moved.
    pub fn scroll_viewport(&mut self, x: f64, y: f64) -> Result<bool, ScriptStop> {
        self.check_live()?;
        self.begin_turn()?;
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        let moved = match page::scroll_viewport(agent, x, y) {
            Ok(moved) => moved,
            Err(error) => {
                self.page_result(Err(error))?;
                false
            }
        };
        self.checkpoint()?;
        Ok(moved)
    }

    /// Where host keyboard input goes: the focused element, else the
    /// body, else the document element (`document.activeElement`).
    pub fn key_target(&self) -> Option<NodeId> {
        let engine_document = self.document();
        let document = engine_document.parsed().document();
        if let Some(focused) = self
            .agent
            .as_ref()
            .ok()
            .and_then(|agent| page::focused_element(agent, document))
        {
            return Some(focused);
        }
        let element = |parent: NodeId, name: &str| {
            document.children(parent).ok()?.find(|&child| {
                matches!(
                    document.node(child).map(|node| node.kind()),
                    Ok(rustic_dom::NodeKind::Element(element)) if element.name().local() == name
                )
            })
        };
        let html = document.children(document.root()).ok()?.find(|&child| {
            matches!(
                document.node(child).map(|node| node.kind()),
                Ok(rustic_dom::NodeKind::Element(_))
            )
        })?;
        Some(element(html, "body").unwrap_or(html))
    }

    /// Whether `requestAnimationFrame` callbacks wait for the next
    /// rendering opportunity (a live host keeps offering frames).
    pub fn wants_animation_frame(&self) -> bool {
        self.pending.queue.borrow().has_frames()
    }

    /// Take the popups `window.open()` asked for with transient activation;
    /// the host opens them (ADR 0062).
    pub fn take_popups(&self) -> Vec<page::PopupRequest> {
        self.agent
            .as_ref()
            .map_or_else(|_| Vec::new(), page::take_popups)
    }

    /// Take the form submissions that passed validation; nothing navigates.
    pub fn take_form_submissions(&self) -> page::FormSubmissions {
        self.agent.as_ref().map_or_else(
            |_| page::FormSubmissions::default(),
            page::take_form_submissions,
        )
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

    /// Queue a previously fetched and validated resource completion as a
    /// document task. The normal turn pump publishes it in task order, after
    /// any prior script's microtask checkpoint, before subsequent JS work.
    ///
    /// The supplied closure may only commit already-staged resource state:
    /// it must not perform file/network I/O or execute JavaScript. A failed
    /// commit returns `ResourceCompletion { applied: false }` without
    /// invalidating an old frame or offering a new one. The same task-count
    /// limit used for scripts, timers and input bounds pending completions.
    pub fn queue_resource_completion<E>(
        &mut self,
        commit: impl FnOnce() -> Result<(), E> + 'static,
    ) -> Result<(), ScriptQueueError> {
        self.check_live().map_err(ScriptQueueError::Stopped)?;
        self.pending
            .queue
            .borrow_mut()
            .queue(Task::ResourceCompletion(ResourceCompletion::new(commit)))
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

    /// Whether the next eligible task at the current clock time publishes a
    /// resource without entering JS. A manual-clock host can use this to
    /// enforce a script-turn budget before pumping the next task.
    pub fn next_task_is_resource_completion(&self) -> bool {
        self.pending
            .queue
            .borrow()
            .next_ready_is_resource(self.clock.now())
    }

    /// Offer this document a rendering opportunity at the current clock time.
    /// Offers made before the next rendering turn coalesce to the latest time.
    pub fn offer_rendering_opportunity(&mut self) -> Result<(), ScriptStop> {
        self.check_live()?;
        self.rendering.offer(self.clock.now());
        Ok(())
    }

    /// Pump one script, timer, input, resource completion or rendering turn.
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
        // A completed resource already next in task order must be published
        // before a pending render can consume the previous input revision.
        // Ordinary scripts and timers retain the existing alternating policy.
        let now = self.clock.now();
        let ready_resource = self.pending.queue.borrow().next_ready_is_resource(now);
        if ready_resource || !self.rendering.has_priority() {
            if let Some(turn) = self.run_script(now)? {
                return Ok(Some(DocumentTurn::Script(turn)));
            }
        }
        let Some(frame) = self.rendering.begin().map_err(|reason| self.stop(reason))? else {
            return Ok(None);
        };
        self.run_animation_callbacks(frame)?;
        let (output, revision) = {
            let document = self.document();
            let revision = self
                .document
                .layout
                .borrow()
                .revision(document.parsed().document());
            (render(&document, frame), revision)
        };
        self.check_live()?;
        self.rendering.record(frame, revision);
        Ok(Some(DocumentTurn::Rendered { frame, output }))
    }

    /// Accept the current frame once before presenting its output.
    /// A later rendering turn, DOM mutation or applied input change invalidates earlier tickets.
    pub fn accept_rendered_frame(&mut self, frame: RenderFrame) -> Result<bool, ScriptStop> {
        self.check_live()?;
        let revision = self
            .document
            .layout
            .borrow()
            .revision(self.document().parsed().document());
        Ok(self.rendering.accept(frame, revision))
    }

    /// Run one eligible task and drain JS promise jobs before a later task.
    /// Host resource completions run without executing JS and are reported as
    /// `ScriptTurn::ResourceCompletion` with their publication result.
    ///
    /// A terminal failure discards pending tasks and releases the JS agent.
    /// Returns `Ok(None)` when there is no eligible script, timer, input or
    /// resource completion. Use
    /// [`Self::run_next_with_render`] to service rendering offers as well.
    pub fn run_next(&mut self) -> Result<Option<ScriptTurn>, ScriptStop> {
        self.check_live()?;
        self.run_script(self.clock.now())
    }

    fn run_script(&mut self, now: Duration) -> Result<Option<ScriptTurn>, ScriptStop> {
        let Some(task) = self.pending.queue.borrow_mut().pop_ready(now) else {
            return Ok(None);
        };

        match task {
            Task::ResourceCompletion(commit) => {
                // Resource delivery is its own host task, not JS execution.
                // All previous JS checkpoints finished before this boundary.
                // The shared publication API changes the revision and queues
                // a frame only if the already-staged commit succeeds.
                let applied = self
                    .apply_resource_completion(|| if commit.apply() { Ok(()) } else { Err(()) })?
                    .is_ok();
                if applied {
                    self.rendering.after_script();
                }
                Ok(Some(ScriptTurn::ResourceCompletion { applied }))
            }
            task => {
                self.begin_turn()?;
                let agent = self.agent.as_mut().map_err(|reason| *reason)?;
                let result = match task {
                    Task::Script(source) => agent.evaluate_source(&source).map(|_| ()),
                    Task::Timer(timer) => timer.run(agent, &self.pending),
                    Task::Input { target, event } => {
                        page::deliver_input(agent, target, &event).map_err(EvalError::from)
                    }
                    Task::PromiseRejections(roots) => notify_rejections(agent, roots),
                    Task::ResourceCompletion(_) => unreachable!("handled above"),
                };
                self.finish_script_turn(result).map(Some)
            }
        }
    }

    fn finish_script_turn(
        &mut self,
        result: Result<(), EvalError>,
    ) -> Result<ScriptTurn, ScriptStop> {
        let turn = match result {
            Ok(_) => ScriptTurn::Completed,
            Err(EvalError::Parse(error)) => {
                self.report_syntax_error(&error.to_string())?;
                ScriptTurn::Failed(ScriptFailure::Parse(error))
            }
            Err(EvalError::Compile(error)) => {
                self.report_syntax_error(&format!("{error:?}"))?;
                ScriptTurn::Failed(ScriptFailure::Compile(error))
            }
            Err(EvalError::Throw(value)) => {
                self.report_value(value)?;
                ScriptTurn::Failed(ScriptFailure::UncaughtException)
            }
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
        self.begin_turn()?;
        // "Run the scroll steps" come before animation frame callbacks.
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        let fired = page::fire_scroll_events(agent);
        self.page_result(fired)?;
        self.checkpoint()?;
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
                Err(Abrupt::Throw(error)) => {
                    self.animation_errors = self.animation_errors.saturating_add(1);
                    self.report_value(error)?;
                }
                Err(Abrupt::OutOfMemory) => return Err(self.stop(ScriptStop::OutOfMemory)),
                Err(Abrupt::Cancelled) => return Err(self.stop_after_cancellation()),
            }
            self.checkpoint()?;
        }
        // Resize Observer steps of "update the rendering", each callback
        // followed by its own checkpoint as script cleanup requires.
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        let observed = page::broadcast_resize_observations(agent);
        self.page_result(observed)?;
        self.checkpoint()?;
        self.pending
            .queue
            .borrow_mut()
            .publish_timers(self.clock.now());
        Ok(())
    }

    fn checkpoint(&mut self) -> Result<(), ScriptStop> {
        loop {
            let agent = self.agent.as_mut().map_err(|reason| *reason)?;
            // Parser and host edits since the last binding call become mutation
            // records before the microtasks they schedule run.
            let flushed = page::flush_mutation_records(agent);
            self.page_result(flushed)?;
            let agent = self.agent.as_mut().map_err(|reason| *reason)?;
            match agent.run_jobs_limited(self.limits.jobs_per_checkpoint) {
                Ok(true) => {}
                Ok(false) => return Err(self.stop(ScriptStop::JobLimit)),
                Err(Abrupt::OutOfMemory) => return Err(self.stop(ScriptStop::OutOfMemory)),
                Err(Abrupt::Cancelled) => return Err(self.stop_after_cancellation()),
                Err(Abrupt::Throw(_)) => unreachable!("the job runner handles thrown values"),
            }
            // Taking the tracker's events releases their roots; the page roots
            // the promises again while it dispatches.
            let rejections = agent.take_rejection_events();
            let mut failed = Vec::new();
            let mut module_errors = 0u64;
            self.pending_modules.retain(|&root| {
                let promise = agent
                    .rooted(root)
                    .and_then(Value::as_object)
                    .expect("retained module completion");
                match agent.promise_rejected(promise) {
                    Some(rejected) => {
                        if rejected {
                            module_errors = module_errors.saturating_add(1);
                            if failed.try_reserve(1).is_ok() {
                                failed.push(
                                    agent.promise_result(promise).unwrap_or(Value::UNDEFINED),
                                );
                            }
                        }
                        false
                    }
                    None => true,
                }
            });
            self.module_script_errors = self.module_script_errors.saturating_add(module_errors);
            // HTML reports a module script's evaluation error like a classic's.
            for reason in failed {
                self.report_value(reason)?;
            }
            self.check_live()?;
            if rejections.is_empty() {
                return Ok(());
            }
            // "Notify about rejected promises" queues a task. If the queue
            // is full the page is notified now, at the end of this checkpoint.
            let admitted = self.pending.queue.borrow().check_admission(0).is_ok();
            if !admitted {
                let agent = self.agent.as_mut().map_err(|reason| *reason)?;
                let reported = page::report_rejections(agent, rejections);
                self.page_result(reported)?;
                continue;
            }
            let agent = self.agent.as_mut().map_err(|reason| *reason)?;
            let mut roots = Vec::new();
            if roots.try_reserve_exact(rejections.len()).is_err() {
                return Err(self.stop(ScriptStop::OutOfMemory));
            }
            for event in &rejections {
                match agent.root(Value::object(event.promise)) {
                    Ok(root) => roots.push((event.operation, root)),
                    Err(OutOfMemory) => return Err(self.stop(ScriptStop::OutOfMemory)),
                }
            }
            let queued = self
                .pending
                .queue
                .borrow_mut()
                .queue(Task::PromiseRejections(roots));
            if queued.is_err() {
                // Admission was checked: only allocation can fail, and a
                // fatal stop releases the agent with its roots.
                return Err(self.stop(ScriptStop::OutOfMemory));
            }
            return Ok(());
        }
    }

    /// Run queued rejection notifications before a readiness step of the
    /// same task source, in their queue order.
    pub(crate) fn run_rejection_notifications(&mut self) -> Result<(), ScriptStop> {
        loop {
            let task = self.pending.queue.borrow_mut().take_rejection_task();
            let Some(Task::PromiseRejections(roots)) = task else {
                return Ok(());
            };
            self.check_live()?;
            self.begin_turn()?;
            let agent = self.agent.as_mut().map_err(|reason| *reason)?;
            let result = notify_rejections(agent, roots);
            self.finish_script_turn(result)?;
        }
    }

    /// Map a page reporting call's result: page exceptions were already
    /// reported, and only fatal stops end the turn.
    fn page_result(&mut self, result: JsResult<()>) -> Result<(), ScriptStop> {
        match result {
            Ok(()) | Err(Abrupt::Throw(_)) => Ok(()),
            Err(Abrupt::OutOfMemory) => Err(self.stop(ScriptStop::OutOfMemory)),
            Err(Abrupt::Cancelled) => Err(self.stop_after_cancellation()),
        }
    }

    /// HTML "report an exception" for a value a script, callback or module threw.
    fn report_value(&mut self, value: Value) -> Result<(), ScriptStop> {
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        let reported = page::report_exception(agent, value);
        self.page_result(reported)
    }

    /// Report a script that failed to parse or compile as a SyntaxError.
    fn report_syntax_error(&mut self, message: &str) -> Result<(), ScriptStop> {
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        match agent.new_error(rustic_js::runtime::ErrorKind::SyntaxError, message) {
            Ok(error) => self.report_value(error),
            Err(error) => self.page_result(Err(error)),
        }
    }

    fn begin_turn(&mut self) -> Result<(), ScriptStop> {
        self.document.layout.borrow_mut().begin_turn();
        let now = self.clock.now();
        let agent = self.agent.as_mut().map_err(|reason| *reason)?;
        agent.set_step_budget(Some(self.limits.steps_per_turn));
        // A page without a DOM host has no activation to time.
        let _ = page::set_time(agent, now);
        Ok(())
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
        self.document.layout.borrow_mut().clear();
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

/// Unroot a queued notification's promises and fire their events.
fn notify_rejections(
    agent: &mut Agent,
    roots: Vec<(rustic_js::runtime::RejectionOperation, HostRoot)>,
) -> Result<(), EvalError> {
    let mut events = Vec::new();
    let reserved = events.try_reserve_exact(roots.len());
    for (operation, root) in roots {
        let promise = agent
            .rooted(root)
            .and_then(Value::as_object)
            .expect("rooted rejected promise");
        agent.unroot(root);
        if reserved.is_ok() {
            events.push(RejectionEvent { operation, promise });
        }
    }
    match reserved {
        // Nothing runs between unrooting and the page rooting them again.
        Ok(()) => page::report_rejections(agent, events).map_err(EvalError::from),
        Err(_) => Err(EvalError::OutOfMemory),
    }
}
