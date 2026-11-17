//! Opt-in HTML loads that execute parser-inserted classics.

use std::fmt;

use rustic_base::{CancellationToken, Clock, SystemClock};
use rustic_dom::{Document, Element, NodeId, NodeKind};
use rustic_html::{
    HtmlDocumentParser, HtmlError, ParseError, ParserLimits, ScriptBoundary, Scripting,
};
use rustic_js::{MAX_SOURCE_UNITS, Source};
use rustic_url::Url;

use crate::classic_scripts::{ExternalScripts, ReadyClassic};
use crate::lifecycle::DocumentScope;
use crate::{
    ClassicFetchError, ClassicRequestId, ClassicScriptLimits, ClassicScriptMode,
    ClassicScriptRequest, DocumentStartError, Engine, ScriptStop, ScriptedDocument, TaskLimits,
};

/// Failure of an opt-in classic-script document load.
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

/// An active HTML load with host-supplied local external classics.
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

    /// Admit a scripted HTML load that can request local external classics.
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
            }),
        })
    }
}

impl<C: Clock> InlineScriptLoad<C> {
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
            )
        }) {
            self.pending = None;
        }
        result
    }
}

impl<C: Clock> ClassicScriptLoad<C> {
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

    /// Deliver one requested source or a recoverable acquisition failure.
    /// The load runs any newly eligible classics before returning.
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
            if let Some(ready) = external.complete(id, result, pending.source_limit)? {
                pending.run_external(ready)?;
            }
            pending.run_available_scripts()
        })
    }

    /// Take the completed document once EOF and all requests have settled.
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
        self.eof && self.external.as_ref().is_none_or(ExternalScripts::is_empty)
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

    fn run_available_scripts(&mut self) -> Result<(), ScriptLoadError> {
        loop {
            self.check_live()?;
            if let Some(external) = self.external.as_mut() {
                if let Some(ready) = external.take_runnable(self.eof) {
                    self.run_external(ready)?;
                    continue;
                }
                if external.blocks_parser() {
                    return Ok(());
                }
            }
            if self.eof {
                return Ok(());
            }
            let next = self.document.with_active_parser(|parser| {
                while let Some(boundary) = parser.advance_to_script()? {
                    let Some(element) = eligible_classic(&boundary)? else {
                        continue;
                    };
                    if let Some(src) = attribute(element, "src") {
                        if let Some(external) = &mut self.external {
                            let admitted = external.admit(
                                boundary.document,
                                boundary.element,
                                trim_ascii_space(src),
                                external_mode(element),
                            )?;
                            let prepared = if admitted {
                                Prepared::ExternalPending(boundary.element)
                            } else {
                                Prepared::ExternalRejected
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
                Some(Prepared::ExternalPending(element)) => {
                    self.document.retain_external_classic(element)?
                }
                Some(Prepared::ExternalRejected) => self.document.note_external_script_error(),
                None if self.input_closed => self.eof = true,
                None => return Ok(()),
            }
        }
    }
}

enum Prepared {
    Inline(NodeId, Source),
    ExternalPending(NodeId),
    ExternalRejected,
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
