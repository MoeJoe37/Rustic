//! Opt-in HTML loads that execute parser-inserted inline classics.

use std::fmt;

use rustic_base::{CancellationToken, Clock, SystemClock};
use rustic_dom::{Document, Element, NodeId, NodeKind};
use rustic_html::{
    HtmlDocumentParser, HtmlError, ParseError, ParserLimits, ScriptBoundary, Scripting,
};
use rustic_js::{MAX_SOURCE_UNITS, Source};

use crate::lifecycle::DocumentScope;
use crate::{DocumentStartError, Engine, ScriptStop, ScriptedDocument, TaskLimits};

/// Failure of an opt-in inline-script document load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineLoadError {
    /// Engine document admission failed.
    Admission(DocumentStartError),
    /// HTML parsing failed or the stream has already failed.
    Parse(ParseError),
    /// The document's execution stopped.
    Stopped(ScriptStop),
    /// A classic script exceeded the source size limit.
    SourceLimit {
        /// Maximum UTF-16 code units allowed for the script.
        limit: usize,
    },
}

impl From<ParseError> for InlineLoadError {
    fn from(error: ParseError) -> Self {
        match error {
            ParseError::Tokenizer(HtmlError::Cancelled) => {
                Self::Stopped(ScriptStop::DocumentCancelled)
            }
            error => Self::Parse(error),
        }
    }
}

impl From<ScriptStop> for InlineLoadError {
    fn from(error: ScriptStop) -> Self {
        Self::Stopped(error)
    }
}

impl fmt::Display for InlineLoadError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(error) => error.fmt(output),
            Self::Parse(error) => error.fmt(output),
            Self::Stopped(error) => error.fmt(output),
            Self::SourceLimit { limit } => {
                write!(
                    output,
                    "inline script exceeds the {limit}-unit UTF-16 source limit"
                )
            }
        }
    }
}

impl std::error::Error for InlineLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Stopped(error) => Some(error),
            Self::SourceLimit { .. } => None,
        }
    }
}

/// An active document with a realm that runs inline classics as markup arrives.
pub struct InlineScriptLoad<C = SystemClock> {
    pending: Option<Pending<C>>,
}

struct Pending<C> {
    document: ScriptedDocument<C>,
    scope: DocumentScope,
    source_limit: usize,
}

impl Engine {
    /// Admit a streaming local HTML load with parser-inserted inline classics.
    pub fn begin_inline_scripted_document(
        &mut self,
        parser_limits: ParserLimits,
        task_limits: TaskLimits,
    ) -> Result<InlineScriptLoad, InlineLoadError> {
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
    ) -> Result<InlineScriptLoad<C>, InlineLoadError> {
        let scope = self
            .open_document_scope()
            .map_err(InlineLoadError::Admission)?;
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
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), InlineLoadError> {
        let pending = self.pending.as_mut().ok_or(ParseError::Poisoned)?;
        let result = pending
            .document
            .with_active_parser(|parser| parser.write(bytes))
            .map_err(InlineLoadError::from)
            .and_then(|()| pending.run_available_scripts());
        if result.is_err() {
            self.pending = None;
        }
        result
    }

    /// Close input and return the same live document, realm and queued work.
    pub fn finish(self) -> Result<ScriptedDocument<C>, InlineLoadError> {
        let mut pending = self.pending.ok_or(ParseError::Poisoned)?;
        pending
            .document
            .with_active_parser(HtmlDocumentParser::close_input)?;
        pending.run_available_scripts()?;
        pending.document.finish_parsing(pending.scope)?;
        Ok(pending.document)
    }
}

impl<C: Clock> Pending<C> {
    fn run_available_scripts(&mut self) -> Result<(), InlineLoadError> {
        loop {
            let next = self.document.with_active_parser(|parser| {
                while let Some(boundary) = parser.advance_to_script()? {
                    if let Some(source) = inline_classic_source(&boundary, self.source_limit)? {
                        return Ok::<_, InlineLoadError>(Some((boundary.element, source)));
                    }
                }
                Ok(None)
            })?;
            let Some((element, source)) = next else {
                return Ok(());
            };
            self.document.run_inline_script(element, &source)?;
        }
    }
}

fn inline_classic_source(
    boundary: &ScriptBoundary<'_>,
    limit: usize,
) -> Result<Option<Source>, InlineLoadError> {
    let document = boundary.document;
    let id = boundary.element;
    let NodeKind::Element(element) = document.node(id).map_err(ParseError::from)?.kind() else {
        return Ok(None);
    };
    if attribute(element, "src").is_some()
        || attribute(element, "nomodule").is_some()
        || !is_classic(element)
        || !matches_legacy_event(element)
        || !connected(document, id)?
    {
        return Ok(None);
    }
    let mut units = Vec::new();
    for child in document.children(id).map_err(ParseError::from)? {
        if let NodeKind::Text(text) = document.node(child).map_err(ParseError::from)?.kind() {
            let text = text.as_units();
            if text.len() > limit - units.len() {
                return Err(InlineLoadError::SourceLimit { limit });
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
        .map_err(|_| InlineLoadError::SourceLimit { limit })
}

fn connected(document: &Document, mut node: NodeId) -> Result<bool, ParseError> {
    while let Some(parent) = document.node(node)?.parent() {
        node = parent;
    }
    Ok(node == document.root())
}

fn attribute<'a>(element: &'a Element, local_name: &str) -> Option<&'a [u16]> {
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
