//! Request admission and execution order for external classic scripts.

use std::collections::{HashMap, VecDeque};

use rustic_base::Budget;
use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_html::ParseError;
use rustic_js::Source;
use rustic_url::{Url, UrlPath};

use crate::inline_scripts::attribute;
use crate::{ScriptLoadError, ScriptStop};

/// Lifetime request and response budgets for an opt-in local script load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassicScriptLimits {
    /// Maximum external classic preparation attempts, including rejected URLs.
    pub max_requests: usize,
    /// Maximum raw bytes in one source response.
    pub max_response_bytes: usize,
    /// Maximum cumulative raw bytes accepted for external sources.
    pub max_total_bytes: usize,
}

impl Default for ClassicScriptLimits {
    fn default() -> Self {
        Self {
            max_requests: 64,
            max_response_bytes: 512 * 1024,
            max_total_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Opaque request identity tied to the requesting document and element.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ClassicRequestId(NodeId);

/// HTML ordering selected by a parser-inserted external classic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassicScriptMode {
    /// The parser waits for the result.
    Blocking,
    /// Execute when the source arrives.
    Async,
    /// Execute in parser order after the input finishes.
    Defer,
}

/// A request that an embedder can fulfill from its local resource provider.
#[derive(Debug)]
pub struct ClassicScriptRequest {
    /// Identity to pass back with the result.
    pub id: ClassicRequestId,
    /// Resolved local URL within the document directory.
    pub url: Url,
    /// The script's execution ordering.
    pub mode: ClassicScriptMode,
    /// Maximum bytes the host should read for this request.
    pub max_bytes: usize,
}

/// How a host's local source acquisition failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassicFetchError {
    /// The request was outside the host's local resource grant.
    Blocked,
    /// The requested resource was absent.
    NotFound,
    /// The response exceeded the request budget.
    TooLarge,
    /// The resource could not be read.
    Failed,
}

pub(crate) struct ExternalScripts {
    document_url: Url,
    attempts: Budget,
    total_bytes: Budget,
    max_response_bytes: usize,
    requests: VecDeque<ClassicScriptRequest>,
    entries: HashMap<ClassicRequestId, Entry>,
    blocker: Option<ClassicRequestId>,
    deferred: VecDeque<ClassicRequestId>,
}

struct Entry {
    mode: ClassicScriptMode,
    state: State,
}

enum State {
    Undispatched,
    Requested,
    Ready(Option<Source>),
}

pub(crate) struct ReadyClassic {
    pub(crate) element: NodeId,
    pub(crate) source: Option<Source>,
}

impl ExternalScripts {
    pub(crate) fn new(url: Url, limits: ClassicScriptLimits) -> Result<Self, ScriptLoadError> {
        local_file_path(&url).ok_or(ScriptLoadError::InvalidDocumentUrl)?;
        Ok(Self {
            document_url: url,
            attempts: Budget::new(limits.max_requests),
            total_bytes: Budget::new(limits.max_total_bytes),
            max_response_bytes: limits.max_response_bytes,
            requests: VecDeque::new(),
            entries: HashMap::new(),
            blocker: None,
            deferred: VecDeque::new(),
        })
    }

    pub(crate) fn admit(
        &mut self,
        document: &Document,
        element: NodeId,
        src: &[u16],
        mode: ClassicScriptMode,
    ) -> Result<bool, ScriptLoadError> {
        if self.attempts.charge(1).is_err() {
            return Ok(false);
        }
        let Some(url) = self.resolve(document, src)? else {
            return Ok(false);
        };
        let id = ClassicRequestId(element);
        self.entries
            .try_reserve(1)
            .map_err(|_| ScriptStop::OutOfMemory)?;
        self.requests
            .try_reserve(1)
            .map_err(|_| ScriptStop::OutOfMemory)?;
        match mode {
            ClassicScriptMode::Blocking => {
                debug_assert!(self.blocker.is_none(), "blocked parser cannot advance");
                self.blocker = Some(id);
            }
            ClassicScriptMode::Async => {}
            ClassicScriptMode::Defer => {
                self.deferred
                    .try_reserve(1)
                    .map_err(|_| ScriptStop::OutOfMemory)?;
                self.deferred.push_back(id);
            }
        }
        self.entries.insert(
            id,
            Entry {
                mode,
                state: State::Undispatched,
            },
        );
        self.requests.push_back(ClassicScriptRequest {
            id,
            url,
            mode,
            max_bytes: self.max_bytes(),
        });
        Ok(true)
    }

    pub(crate) fn next_request(&mut self) -> Option<ClassicScriptRequest> {
        let mut request = self.requests.pop_front()?;
        request.max_bytes = self.max_bytes();
        self.entries
            .get_mut(&request.id)
            .expect("queued request has entry")
            .state = State::Requested;
        Some(request)
    }

    pub(crate) fn complete(
        &mut self,
        id: ClassicRequestId,
        result: Result<&[u8], ClassicFetchError>,
        source_limit: usize,
    ) -> Result<Option<ReadyClassic>, ScriptLoadError> {
        let Some(entry) = self
            .entries
            .get(&id)
            .filter(|entry| matches!(entry.state, State::Requested))
        else {
            return Err(ScriptLoadError::UnknownScriptRequest);
        };
        let mode = entry.mode;
        let source = result
            .ok()
            .and_then(|bytes| self.decode(bytes, source_limit));
        if mode == ClassicScriptMode::Async {
            self.entries.remove(&id);
            return Ok(Some(ReadyClassic {
                element: id.0,
                source,
            }));
        }
        self.entries.get_mut(&id).expect("validated request").state = State::Ready(source);
        Ok(None)
    }

    pub(crate) fn take_runnable(&mut self, eof: bool) -> Option<ReadyClassic> {
        if let Some(id) = self.blocker {
            let result = self.take_ready(id)?;
            self.blocker = None;
            return Some(result);
        }
        if eof {
            let id = *self.deferred.front()?;
            let result = self.take_ready(id)?;
            self.deferred.pop_front();
            return Some(result);
        }
        None
    }

    pub(crate) fn blocks_parser(&self) -> bool {
        self.blocker.is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn take_ready(&mut self, id: ClassicRequestId) -> Option<ReadyClassic> {
        if !matches!(self.entries.get(&id)?.state, State::Ready(_)) {
            return None;
        }
        let State::Ready(source) = self.entries.remove(&id)?.state else {
            unreachable!("ready request state checked");
        };
        Some(ReadyClassic {
            element: id.0,
            source,
        })
    }

    fn max_bytes(&self) -> usize {
        self.max_response_bytes.min(self.total_bytes.remaining())
    }

    fn decode(&mut self, bytes: &[u8], limit: usize) -> Option<Source> {
        if bytes.len() > self.max_response_bytes || self.total_bytes.charge(bytes.len()).is_err() {
            return None;
        }
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        Source::from_str_limited(&String::from_utf8_lossy(bytes), limit).ok()
    }

    fn resolve(&self, document: &Document, src: &[u16]) -> Result<Option<Url>, ParseError> {
        if src.is_empty() {
            return Ok(None);
        }
        let base = document_base(document, &self.document_url)?;
        let Ok(url) = Url::parse(&String::from_utf16_lossy(src), Some(&base)) else {
            return Ok(None);
        };
        let Some(path) = local_file_path(&url) else {
            return Ok(None);
        };
        let directory = local_file_path(&self.document_url)
            .and_then(|path| path.split_last().map(|(_, directory)| directory))
            .expect("local document URL validated at admission");
        Ok((path.len() > directory.len() && path.starts_with(directory)).then_some(url))
    }
}

fn local_file_path(url: &Url) -> Option<&[String]> {
    if url.scheme() != "file" || url.host().is_some_and(|host| !host.serialize().is_empty()) {
        return None;
    }
    let UrlPath::Segments(path) = url.path() else {
        return None;
    };
    if path.last().is_none_or(String::is_empty)
        || path
            .iter()
            .any(|segment| segment.as_bytes().windows(3).any(encoded_separator))
    {
        return None;
    }
    Some(path)
}

fn encoded_separator(triplet: &[u8]) -> bool {
    matches!(triplet, b"%00" | b"%2f" | b"%2F" | b"%5c" | b"%5C")
}

fn document_base(document: &Document, fallback: &Url) -> Result<Url, ParseError> {
    for (id, _) in document.walk(document.root())? {
        if let NodeKind::Element(element) = document.node(id)?.kind() {
            let name = element.name();
            if name.namespace() == Some(HTML_NAMESPACE) && name.local() == "base" {
                if let Some(href) = attribute(element, "href") {
                    if let Ok(base) = Url::parse(&String::from_utf16_lossy(href), Some(fallback)) {
                        if !matches!(base.scheme(), "data" | "javascript") {
                            return Ok(base);
                        }
                    }
                    break;
                }
            }
        }
    }
    Ok(fallback.clone())
}
