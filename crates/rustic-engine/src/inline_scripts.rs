//! Opt-in HTML loads that execute parser-inserted classics and local modules.

use std::fmt;
use std::time::Duration;

use rustic_base::{CancellationToken, Clock, SystemClock};
use rustic_bindings::page::LifecycleEvent;
use rustic_dom::{Document, Element, NodeId, NodeKind, ReadyState};
use rustic_html::{
    Encoding, HtmlDocumentParser, HtmlError, ParseError, ParserLimits, ScriptBoundary, Scripting,
};
use rustic_js::module::{GraphError, GraphLimits};
use rustic_js::{MAX_SOURCE_UNITS, Source};
use rustic_url::Url;

use crate::classic_scripts::{ExternalScripts, ModuleJob, ReadyClassic, ReadyScript};
use crate::lifecycle::DocumentScope;
use crate::{
    ClassicFetchError, ClassicRequestId, ClassicScriptLimits, ClassicScriptMode,
    ClassicScriptRequest, DocumentStartError, Engine, LayoutSource, ScriptStop, ScriptTurn,
    ScriptedDocument, TaskLimits,
};

/// Failure of an opt-in scripted document load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptLoadError {
    /// Engine document admission failed.
    Admission(DocumentStartError),
    /// HTML parsing failed or the stream has already failed.
    Parse(ParseError),
    /// The document's execution stopped.
    Stopped(ScriptStop),
    /// The URL granting local resources must identify a local file.
    InvalidDocumentUrl,
    /// The completion does not identify an outstanding dispatched request.
    UnknownScriptRequest,
    /// An inline script exceeded the source size limit.
    SourceLimit {
        /// Maximum UTF-16 code units allowed for the script.
        limit: usize,
    },
}

impl From<ParseError> for ScriptLoadError {
    fn from(error: ParseError) -> Self {
        match error {
            ParseError::Tokenizer(HtmlError::Cancelled) => {
                Self::Stopped(ScriptStop::DocumentCancelled)
            }
            error => Self::Parse(error),
        }
    }
}

impl From<ScriptStop> for ScriptLoadError {
    fn from(error: ScriptStop) -> Self {
        Self::Stopped(error)
    }
}

impl fmt::Display for ScriptLoadError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(error) => error.fmt(output),
            Self::Parse(error) => error.fmt(output),
            Self::Stopped(error) => error.fmt(output),
            Self::InvalidDocumentUrl => {
                output.write_str("scripted document URL must identify a local file")
            }
            Self::UnknownScriptRequest => {
                output.write_str("script request is not outstanding on this document")
            }
            Self::SourceLimit { limit } => {
                write!(
                    output,
                    "inline script exceeds the {limit}-unit UTF-16 source limit"
                )
            }
        }
    }
}

impl std::error::Error for ScriptLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Stopped(error) => Some(error),
            Self::InvalidDocumentUrl | Self::UnknownScriptRequest | Self::SourceLimit { .. } => {
                None
            }
        }
    }
}

/// An active document with a realm that runs inline classics as markup arrives.
pub struct InlineScriptLoad<C = SystemClock> {
    pending: Option<Pending<C>>,
}

/// An active HTML load with host-supplied local script resources.
pub struct ClassicScriptLoad<C = SystemClock> {
    inner: InlineScriptLoad<C>,
}

struct Pending<C> {
    document: ScriptedDocument<C>,
    scope: DocumentScope,
    source_limit: usize,
    external: Option<ExternalScripts>,
    input_closed: bool,
    eof: bool,
    content_loaded: bool,
    load_delivered: bool,
}

impl Engine {
    /// Admit a streaming local HTML load with parser-inserted inline classics.
    pub fn begin_inline_scripted_document(
        &mut self,
        parser_limits: ParserLimits,
        task_limits: TaskLimits,
    ) -> Result<InlineScriptLoad, ScriptLoadError> {
        self.begin_inline_scripted_document_with_clock(
            parser_limits,
            task_limits,
            SystemClock::default(),
        )
    }

    /// Admit a scripted HTML load with a host-selected monotonic clock.
    pub fn begin_inline_scripted_document_with_clock<C: Clock>(
        &mut self,
        parser_limits: ParserLimits,
        task_limits: TaskLimits,
        clock: C,
    ) -> Result<InlineScriptLoad<C>, ScriptLoadError> {
        self.begin_classic_load(parser_limits, task_limits, clock, None)
    }

    /// Admit a scripted HTML load that can request local classics and JavaScript modules.
    pub fn begin_local_scripted_document(
        &mut self,
        parser_limits: ParserLimits,
        task_limits: TaskLimits,
        document_url: Url,
        limits: ClassicScriptLimits,
    ) -> Result<ClassicScriptLoad, ScriptLoadError> {
        self.begin_local_scripted_document_with_clock(
            parser_limits,
            task_limits,
            document_url,
            limits,
            SystemClock::default(),
        )
    }

    /// Admit a local scripted load with a host-selected monotonic clock.
    pub fn begin_local_scripted_document_with_clock<C: Clock>(
        &mut self,
        parser_limits: ParserLimits,
        task_limits: TaskLimits,
        document_url: Url,
        limits: ClassicScriptLimits,
        clock: C,
    ) -> Result<ClassicScriptLoad<C>, ScriptLoadError> {
        let external = ExternalScripts::new(document_url, limits)?;
        let inner = self.begin_classic_load(parser_limits, task_limits, clock, Some(external))?;
        Ok(ClassicScriptLoad { inner })
    }

    fn begin_classic_load<C: Clock>(
        &mut self,
        parser_limits: ParserLimits,
        task_limits: TaskLimits,
        clock: C,
        external: Option<ExternalScripts>,
    ) -> Result<InlineScriptLoad<C>, ScriptLoadError> {
        let scope = self
            .open_document_scope()
            .map_err(ScriptLoadError::Admission)?;
        let cancellation = scope.cancellation().clone();
        let parser = HtmlDocumentParser::new_with_scripting(
            parser_limits,
            cancellation.clone(),
            Scripting::Enabled,
        )?;
        let document = ScriptedDocument::with_parser(parser, cancellation, task_limits, clock)?;
        Ok(InlineScriptLoad {
            pending: Some(Pending {
                document,
                scope,
                source_limit: task_limits.pending_source_units.min(MAX_SOURCE_UNITS),
                external,
                input_closed: false,
                eof: false,
                content_loaded: false,
                load_delivered: false,
            }),
        })
    }
}

impl<C: Clock> InlineScriptLoad<C> {
    /// Commit the byte-stream encoding before any parser or author script runs.
    pub fn set_initial_encoding(&mut self, encoding: Encoding) -> Result<(), ScriptLoadError> {
        self.with_pending(|pending| {
            if pending.input_closed {
                return Err(ParseError::EncodingLocked.into());
            }
            pending
                .document
                .with_active_parser(|parser| parser.set_initial_encoding(encoding))?;
            Ok(())
        })
    }

    /// The shared work token while the load is active.
    pub fn cancellation(&self) -> Option<&CancellationToken> {
        self.pending.as_ref().map(|load| load.scope.cancellation())
    }

    /// Feed one chunk, evaluating each eligible inline classic and its jobs.
    /// A fatal failure discards the active document and releases its slot.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), ScriptLoadError> {
        self.with_pending(|pending| {
            pending
                .document
                .with_active_parser(|parser| parser.write(bytes))?;
            pending.run_available_scripts()
        })
    }

    /// Close input and return the same live document, realm and queued work.
    pub fn finish(mut self) -> Result<ScriptedDocument<C>, ScriptLoadError> {
        self.with_pending(Pending::close_input)?;
        self.pending
            .take()
            .ok_or(ParseError::Poisoned)?
            .into_document()
    }

    fn with_pending<T>(
        &mut self,
        run: impl FnOnce(&mut Pending<C>) -> Result<T, ScriptLoadError>,
    ) -> Result<T, ScriptLoadError> {
        let pending = self.pending.as_mut().ok_or(ParseError::Poisoned)?;
        let result = pending.check_live().and_then(|()| run(pending));
        if result.as_ref().is_err_and(|error| {
            !matches!(
                error,
                ScriptLoadError::UnknownScriptRequest
                    | ScriptLoadError::Parse(ParseError::InputClosed)
                    | ScriptLoadError::Parse(ParseError::EncodingLocked)
            )
        }) {
            self.pending = None;
        }
        result
    }
}

impl<C: Clock> ClassicScriptLoad<C> {
    /// Commit the byte-stream encoding before any parser or author script runs.
    pub fn set_initial_encoding(&mut self, encoding: Encoding) -> Result<(), ScriptLoadError> {
        self.inner.set_initial_encoding(encoding)
    }

    /// Supply the layout pipeline before feeding HTML so parser scripts can read layout.
    pub fn set_layout_source(
        &mut self,
        source: Box<dyn LayoutSource>,
    ) -> Result<(), ScriptLoadError> {
        self.inner.with_pending(|pending| {
            pending.document.set_layout_source(source)?;
            Ok(())
        })
    }

    /// The shared work token while the load is active.
    pub fn cancellation(&self) -> Option<&CancellationToken> {
        self.inner.cancellation()
    }

    /// Access the host clock while the load is active.
    pub fn clock(&self) -> Option<&C> {
        self.inner
            .pending
            .as_ref()
            .map(|pending| pending.document.clock())
    }

    /// Feed a chunk. While a blocking script waits, the parser buffers input within its byte limit.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), ScriptLoadError> {
        self.inner.feed(bytes)
    }

    /// Finish the input stream. Outstanding requests can still be dispatched and completed.
    pub fn close_input(&mut self) -> Result<(), ScriptLoadError> {
        self.inner.with_pending(Pending::close_input)
    }

    /// Take the next discovered request, if any.
    pub fn next_request(&mut self) -> Option<ClassicScriptRequest> {
        if self.cancellation()?.is_cancelled() {
            return None;
        }
        self.inner
            .pending
            .as_mut()?
            .external
            .as_mut()?
            .next_request()
    }

    /// Whether a blocking classic is holding the parser at its insertion point.
    pub fn is_parser_blocked(&self) -> bool {
        self.inner
            .pending
            .as_ref()
            .and_then(|pending| pending.external.as_ref())
            .is_some_and(ExternalScripts::blocks_parser)
    }

    /// Deliver one requested source or a recoverable acquisition failure.
    /// The load runs any newly eligible scripts before returning.
    pub fn complete_script(
        &mut self,
        id: ClassicRequestId,
        result: Result<&[u8], ClassicFetchError>,
    ) -> Result<(), ScriptLoadError> {
        if self.inner.pending.is_none() {
            return Err(ScriptLoadError::UnknownScriptRequest);
        }
        self.inner.with_pending(|pending| {
            let external = pending
                .external
                .as_mut()
                .expect("local script load has a queue");
            if let Some(ready) = external.complete(
                id,
                result,
                pending.source_limit,
                pending.scope.cancellation(),
            )? {
                pending.run_external(ready)?;
            }
            pending.run_available_scripts()
        })
    }

    /// Pump one eligible page task, for example a timer awaited by a module.
    pub fn run_next(&mut self) -> Result<Option<ScriptTurn>, ScriptLoadError> {
        self.inner.with_pending(|pending| {
            let turn = pending.document.run_next()?;
            pending.run_available_scripts()?;
            Ok(turn)
        })
    }

    /// Delay until the next pending timer, if the active load has one.
    pub fn time_until_next_timer(&mut self) -> Result<Option<Duration>, ScriptLoadError> {
        self.inner
            .with_pending(|pending| Ok(pending.document.time_until_next_timer()?))
    }

    /// Take the completed document once EOF and all scripts have settled.
    pub fn take_document(&mut self) -> Result<Option<ScriptedDocument<C>>, ScriptLoadError> {
        if self.inner.pending.is_none()
            || !self.inner.with_pending(|pending| Ok(pending.ready()))?
        {
            return Ok(None);
        }
        self.inner
            .pending
            .take()
            .map(Pending::into_document)
            .transpose()
    }
}

impl<C: Clock> Pending<C> {
    fn check_live(&self) -> Result<(), ScriptLoadError> {
        self.scope
            .cancellation()
            .check()
            .map_err(|_| ScriptStop::DocumentCancelled.into())
    }

    fn close_input(&mut self) -> Result<(), ScriptLoadError> {
        if !self.input_closed {
            self.document
                .with_active_parser(HtmlDocumentParser::close_input)?;
            self.input_closed = true;
        }
        self.run_available_scripts()
    }

    fn ready(&self) -> bool {
        self.load_delivered
    }

    fn into_document(mut self) -> Result<ScriptedDocument<C>, ScriptLoadError> {
        debug_assert!(self.ready(), "the parser and its requests have settled");
        self.check_live()?;
        self.document.finish_parsing(self.scope)?;
        Ok(self.document)
    }

    fn run_external(&mut self, ready: ReadyClassic) -> Result<(), ScriptLoadError> {
        self.document
            .run_external_classic(ready.element, ready.source.as_ref())?;
        Ok(())
    }

    fn run_module(&mut self, job: ModuleJob) -> Result<(), ScriptLoadError> {
        let modules = &self
            .external
            .as_ref()
            .expect("module load has a queue")
            .modules;
        let mut limits = GraphLimits::default();
        limits.parse.cancellation = self.scope.cancellation().clone();
        let loaded = match modules.graph(job.key, &limits) {
            Ok(graph) => {
                self.document.run_module_graph(&graph, &limits, modules)?;
                job.external.then_some(true)
            }
            Err(GraphError::Cancelled) => return Err(ScriptStop::DocumentCancelled.into()),
            Err(_) => {
                self.document.note_module_script_error();
                Some(false)
            }
        };
        self.document.finish_element_script(job.element, loaded)?;
        Ok(())
    }

    fn change_readiness(&mut self, state: ReadyState) -> Result<(), ScriptLoadError> {
        self.document
            .with_active_parser(|parser| {
                parser
                    .live_document_mut()
                    .expect("active document")
                    .advance_ready_state(state)
            })
            .map_err(ParseError::from)?;
        self.document
            .deliver_lifecycle(LifecycleEvent::ReadyStateChange)?;
        Ok(())
    }

    fn advance_lifecycle(&mut self) -> Result<(), ScriptLoadError> {
        if !self.content_loaded
            && self
                .external
                .as_ref()
                .is_none_or(ExternalScripts::deferred_is_empty)
        {
            // Rejection notifications queued by earlier scripts share the
            // DOM manipulation task source and run first.
            self.document.run_rejection_notifications()?;
            self.document
                .deliver_lifecycle(LifecycleEvent::DomContentLoaded)?;
            self.content_loaded = true;
        }
        if self.content_loaded
            && !self.load_delivered
            && self.external.as_ref().is_none_or(ExternalScripts::is_empty)
            && !self.document.has_pending_modules()
        {
            self.document.run_rejection_notifications()?;
            self.change_readiness(ReadyState::Complete)?;
            self.document
                .deliver_lifecycle(LifecycleEvent::WindowLoad)?;
            self.load_delivered = true;
        }
        Ok(())
    }

    fn run_available_scripts(&mut self) -> Result<(), ScriptLoadError> {
        loop {
            self.check_live()?;
            if let Some(external) = self.external.as_mut() {
                if let Some(ready) = external.take_runnable(self.eof)? {
                    match ready {
                        ReadyScript::Classic(ready) => self.run_external(ready)?,
                        ReadyScript::Module(entry) => self.run_module(entry)?,
                    }
                    continue;
                }
                if external.blocks_parser() {
                    return Ok(());
                }
            }
            if self.eof {
                return self.advance_lifecycle();
            }
            let next = self.document.with_active_parser(|parser| {
                let document_encoding = parser.encoding();
                while let Some(boundary) = parser.advance_to_script()? {
                    if let Some(external) = &mut self.external {
                        if let Some(element) = module_element(&boundary)? {
                            let mode = if attribute(element, "async").is_some() {
                                ClassicScriptMode::Async
                            } else {
                                ClassicScriptMode::Defer
                            };
                            let admitted = external.admit_module(
                                &boundary,
                                mode,
                                attribute(element, "src").map(trim_ascii_space),
                                || inline_classic_source(&boundary, self.source_limit),
                                self.scope.cancellation(),
                            )?;
                            if !admitted {
                                return Ok::<_, ScriptLoadError>(Some(Prepared::ModuleRejected(
                                    boundary.element,
                                )));
                            }
                            return Ok(Some(Prepared::ModuleQueued(boundary.element)));
                        }
                    }
                    let Some(element) = eligible_classic(&boundary)? else {
                        continue;
                    };
                    if let Some(src) = attribute(element, "src") {
                        if let Some(external) = &mut self.external {
                            let encoding = attribute(element, "charset")
                                .and_then(|label| String::from_utf16(label).ok())
                                .and_then(|label| Encoding::for_label(&label))
                                .unwrap_or(document_encoding);
                            let admitted = external.admit(
                                boundary.document,
                                boundary.element,
                                trim_ascii_space(src),
                                external_mode(element),
                                encoding,
                            )?;
                            let prepared = if admitted {
                                Prepared::ExternalPending(boundary.element)
                            } else {
                                Prepared::ExternalRejected(boundary.element)
                            };
                            return Ok::<_, ScriptLoadError>(Some(prepared));
                        }
                    } else if let Some(source) =
                        inline_classic_source(&boundary, self.source_limit)?
                    {
                        return Ok(Some(Prepared::Inline(boundary.element, source)));
                    }
                }
                Ok(None)
            })?;
            match next {
                Some(Prepared::Inline(element, source)) => {
                    self.document.run_inline_script(element, &source)?
                }
                Some(Prepared::ExternalPending(element) | Prepared::ModuleQueued(element)) => {
                    self.document.retain_external_classic(element)?
                }
                Some(Prepared::ExternalRejected(element)) => {
                    self.document.note_external_script_error();
                    self.document.finish_element_script(element, Some(false))?;
                }
                Some(Prepared::ModuleRejected(element)) => {
                    self.document.note_module_script_error();
                    self.document.finish_element_script(element, Some(false))?;
                }
                None if self.input_closed => {
                    self.eof = true;
                    self.change_readiness(ReadyState::Interactive)?;
                }
                None => return Ok(()),
            }
        }
    }
}

enum Prepared {
    Inline(NodeId, Source),
    ExternalPending(NodeId),
    ExternalRejected(NodeId),
    ModuleQueued(NodeId),
    ModuleRejected(NodeId),
}

fn module_element<'a>(boundary: &ScriptBoundary<'a>) -> Result<Option<&'a Element>, ParseError> {
    let id = boundary.element;
    let NodeKind::Element(element) = boundary.document.node(id)?.kind() else {
        return Ok(None);
    };
    if attribute(element, "type").is_some_and(|value| ascii_matches(value, "module"))
        && connected(boundary.document, id)?
    {
        return Ok(Some(element));
    }
    Ok(None)
}

fn eligible_classic<'a>(boundary: &ScriptBoundary<'a>) -> Result<Option<&'a Element>, ParseError> {
    let document = boundary.document;
    let id = boundary.element;
    let NodeKind::Element(element) = document.node(id)?.kind() else {
        return Ok(None);
    };
    if attribute(element, "nomodule").is_some()
        || !is_classic(element)
        || !matches_legacy_event(element)
        || !connected(document, id)?
    {
        return Ok(None);
    }
    Ok(Some(element))
}

fn external_mode(element: &Element) -> ClassicScriptMode {
    if attribute(element, "async").is_some() {
        ClassicScriptMode::Async
    } else if attribute(element, "defer").is_some() {
        ClassicScriptMode::Defer
    } else {
        ClassicScriptMode::Blocking
    }
}

fn inline_classic_source(
    boundary: &ScriptBoundary<'_>,
    limit: usize,
) -> Result<Option<Source>, ScriptLoadError> {
    let document = boundary.document;
    let id = boundary.element;
    let mut units = Vec::new();
    for child in document.children(id).map_err(ParseError::from)? {
        if let NodeKind::Text(text) = document.node(child).map_err(ParseError::from)?.kind() {
            let text = text.as_units();
            if text.len() > limit - units.len() {
                return Err(ScriptLoadError::SourceLimit { limit });
            }
            units
                .try_reserve(text.len())
                .map_err(|_| ScriptStop::OutOfMemory)?;
            units.extend_from_slice(text);
        }
    }
    if units.is_empty() {
        return Ok(None);
    }
    Source::from_utf16(units, limit)
        .map(Some)
        .map_err(|_| ScriptLoadError::SourceLimit { limit })
}

fn connected(document: &Document, mut node: NodeId) -> Result<bool, ParseError> {
    while let Some(parent) = document.node(node)?.parent() {
        node = parent;
    }
    Ok(node == document.root())
}

pub(crate) fn attribute<'a>(element: &'a Element, local_name: &str) -> Option<&'a [u16]> {
    element.attributes().iter().find_map(|attribute| {
        let name = attribute.name();
        (name.namespace().is_none() && name.local() == local_name)
            .then(|| attribute.value().as_units())
    })
}

// WHATWG MIME Sniffing, JavaScript MIME type essence matches.
const JAVASCRIPT_TYPES: &[&str] = &[
    "application/ecmascript",
    "application/javascript",
    "application/x-ecmascript",
    "application/x-javascript",
    "text/ecmascript",
    "text/javascript",
    "text/javascript1.0",
    "text/javascript1.1",
    "text/javascript1.2",
    "text/javascript1.3",
    "text/javascript1.4",
    "text/javascript1.5",
    "text/jscript",
    "text/livescript",
    "text/x-ecmascript",
    "text/x-javascript",
];

fn is_classic(element: &Element) -> bool {
    if let Some(value) = attribute(element, "type") {
        if value.is_empty() {
            return true;
        }
        let value = trim_ascii_space(value);
        return JAVASCRIPT_TYPES
            .iter()
            .any(|name| ascii_matches(value, name));
    }
    let Some(language) = attribute(element, "language") else {
        return true;
    };
    language.is_empty()
        || JAVASCRIPT_TYPES
            .iter()
            .filter_map(|name| name.strip_prefix("text/"))
            .any(|name| ascii_matches(language, name))
}

fn matches_legacy_event(element: &Element) -> bool {
    match (attribute(element, "for"), attribute(element, "event")) {
        (Some(target), Some(event)) => {
            let event = trim_ascii_space(event);
            ascii_matches(trim_ascii_space(target), "window")
                && (ascii_matches(event, "onload") || ascii_matches(event, "onload()"))
        }
        _ => true,
    }
}

fn trim_ascii_space(mut units: &[u16]) -> &[u16] {
    let is_space = |&unit: &u16| matches!(unit, 0x09 | 0x0a | 0x0c | 0x0d | 0x20);
    while units.first().is_some_and(is_space) {
        units = &units[1..];
    }
    while units.last().is_some_and(is_space) {
        units = &units[..units.len() - 1];
    }
    units
}

fn ascii_matches(units: &[u16], text: &str) -> bool {
    units.len() == text.len()
        && units.iter().zip(text.bytes()).all(|(&unit, byte)| {
            unit == u16::from(byte) || unit == u16::from(byte.to_ascii_uppercase())
        })
}
