//! Local script resource admission and execution order.

use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroU64;
use std::sync::Arc;

use rustic_base::{Budget, CancellationToken, DocumentId, IdSequence};
use rustic_dom::{Document, HTML_NAMESPACE, NodeId, NodeKind};
use rustic_html::{ParseError, ScriptBoundary};
use rustic_js::module::ModuleRecord;
use rustic_js::{ParseLimits, Source};
use rustic_url::{Url, UrlPath};

use crate::inline_scripts::attribute;
use crate::local_modules::{LocalModules, ModuleKey, ModuleState, resolve_import};
use crate::{ScriptLoadError, ScriptStop};

/// Lifetime request and response budgets for an opt-in local script load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassicScriptLimits {
    /// Maximum external classic and module preparation attempts, plus new dependencies.
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

/// Opaque request identity tied to the requesting document.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ClassicRequestId {
    document: DocumentId,
    sequence: NonZeroU64,
}

/// HTML ordering selected by the script element that first requests a resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassicScriptMode {
    /// The parser waits for the result.
    Blocking,
    /// Execute when the source arrives.
    Async,
    /// Execute in parser order after the input finishes.
    Defer,
}

/// Kind of source expected from the local resource provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptResourceKind {
    /// Source text for a classic script element.
    Classic,
    /// Source text for a JavaScript module in the document's module map.
    JavaScriptModule,
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
    /// Whether these bytes are for a classic or a module.
    pub kind: ScriptResourceKind,
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
    ids: IdSequence,
    entries: HashMap<ClassicRequestId, Entry>,
    blocker: Option<ClassicRequestId>,
    deferred: VecDeque<Deferred>,
    async_modules: VecDeque<ModuleJob>,
    module_requests: HashMap<ClassicRequestId, ModuleFetch>,
    pub(crate) modules: LocalModules,
}

struct Entry {
    element: NodeId,
    mode: ClassicScriptMode,
    state: State,
}

enum Deferred {
    Classic(ClassicRequestId),
    Module(Box<ModuleJob>),
}

struct ModuleFetch {
    url: Url,
    mode: ClassicScriptMode,
    dispatched: bool,
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

pub(crate) enum ReadyScript {
    Classic(ReadyClassic),
    Module(ModuleJob),
}

#[derive(Clone)]
pub(crate) struct ModuleJob {
    pub(crate) key: ModuleKey,
    pub(crate) element: NodeId,
    pub(crate) external: bool,
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
            ids: IdSequence::new(),
            entries: HashMap::new(),
            blocker: None,
            deferred: VecDeque::new(),
            async_modules: VecDeque::new(),
            module_requests: HashMap::new(),
            modules: LocalModules::default(),
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
        let id = self.new_request(element.document())?;
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
                self.deferred.push_back(Deferred::Classic(id));
            }
        }
        self.entries.insert(
            id,
            Entry {
                element,
                mode,
                state: State::Undispatched,
            },
        );
        self.requests.push_back(ClassicScriptRequest {
            id,
            url,
            mode,
            kind: ScriptResourceKind::Classic,
            max_bytes: self.max_bytes(),
        });
        Ok(true)
    }

    pub(crate) fn admit_module(
        &mut self,
        boundary: &ScriptBoundary<'_>,
        mode: ClassicScriptMode,
        src: Option<&[u16]>,
        inline_source: impl FnOnce() -> Result<Option<Source>, ScriptLoadError>,
        cancellation: &CancellationToken,
    ) -> Result<bool, ScriptLoadError> {
        if self.attempts.charge(1).is_err() {
            return Ok(false);
        }
        let owner = boundary.element.document();
        let key = if let Some(src) = src {
            let Some(url) = self.resolve(boundary.document, src)? else {
                return Ok(false);
            };
            self.fetch_module(owner, url, mode)?
        } else {
            let key = ModuleKey::Inline(boundary.element);
            let base = document_base(boundary.document, &self.document_url)?;
            let source = match inline_source() {
                Ok(source) => source,
                Err(ScriptLoadError::SourceLimit { .. }) => return Ok(false),
                Err(error) => return Err(error),
            };
            let source =
                source.unwrap_or_else(|| Source::from_str_limited("", 0).expect("empty source"));
            self.parse_module_source(key.clone(), base, source, owner, mode, cancellation)?;
            key
        };
        let job = ModuleJob {
            key,
            element: boundary.element,
            external: src.is_some(),
        };
        if mode == ClassicScriptMode::Async {
            self.async_modules
                .try_reserve(1)
                .map_err(|_| ScriptStop::OutOfMemory)?;
            self.async_modules.push_back(job);
        } else {
            self.deferred
                .try_reserve(1)
                .map_err(|_| ScriptStop::OutOfMemory)?;
            self.deferred.push_back(Deferred::Module(Box::new(job)));
        }
        Ok(true)
    }

    fn new_request(&self, document: DocumentId) -> Result<ClassicRequestId, ScriptStop> {
        let sequence = self.ids.allocate().map_err(|_| ScriptStop::OutOfMemory)?;
        Ok(ClassicRequestId { document, sequence })
    }

    fn fetch_module(
        &mut self,
        document: DocumentId,
        url: Url,
        mode: ClassicScriptMode,
    ) -> Result<ModuleKey, ScriptStop> {
        let key = ModuleKey::External(url.clone());
        if self.modules.contains(&key) {
            return Ok(key);
        }
        self.module_requests
            .try_reserve(1)
            .map_err(|_| ScriptStop::OutOfMemory)?;
        self.requests
            .try_reserve(1)
            .map_err(|_| ScriptStop::OutOfMemory)?;
        let id = self.new_request(document)?;
        self.modules.set(key.clone(), ModuleState::Fetching)?;
        self.module_requests.insert(
            id,
            ModuleFetch {
                url: url.clone(),
                mode,
                dispatched: false,
            },
        );
        self.requests.push_back(ClassicScriptRequest {
            id,
            url,
            mode,
            kind: ScriptResourceKind::JavaScriptModule,
            max_bytes: self.max_bytes(),
        });
        Ok(key)
    }

    fn parse_module_source(
        &mut self,
        key: ModuleKey,
        base: Url,
        source: Source,
        document: DocumentId,
        mode: ClassicScriptMode,
        cancellation: &CancellationToken,
    ) -> Result<(), ScriptLoadError> {
        let limits = ParseLimits {
            cancellation: cancellation.clone(),
            ..ParseLimits::default()
        };
        let record = match ModuleRecord::parse(&source, &limits) {
            Ok(record) => record,
            Err(rustic_js::ParseError::Cancelled) => {
                return Err(ScriptStop::DocumentCancelled.into());
            }
            Err(_) => return Ok(self.modules.set(key, ModuleState::Failed)?),
        };
        let mut dependencies = Vec::new();
        dependencies
            .try_reserve(record.requests().len())
            .map_err(|_| ScriptStop::OutOfMemory)?;
        for request in record.requests() {
            let Some(url) = resolve_import(&base, request).filter(|url| self.in_grant(url)) else {
                return Ok(self.modules.set(key, ModuleState::Failed)?);
            };
            dependencies.push(ModuleKey::External(url));
        }
        let mut missing = HashSet::new();
        missing
            .try_reserve(dependencies.len())
            .map_err(|_| ScriptStop::OutOfMemory)?;
        for dependency in &dependencies {
            if *dependency != key && !self.modules.contains(dependency) {
                missing.insert(dependency);
            }
        }
        if self.attempts.charge(missing.len()).is_err() {
            return Ok(self.modules.set(key, ModuleState::Failed)?);
        }
        let inline_base = matches!(key, ModuleKey::Inline(_)).then(|| Box::new(base));
        self.modules.set(
            key,
            ModuleState::Parsed {
                inline_base,
                record: Arc::new(record),
                units: source.len(),
                dependencies: dependencies.clone(),
            },
        )?;
        for dependency in dependencies {
            let ModuleKey::External(url) = dependency else {
                unreachable!("imports resolve to URLs")
            };
            self.fetch_module(document, url, mode)?;
        }
        Ok(())
    }

    pub(crate) fn next_request(&mut self) -> Option<ClassicScriptRequest> {
        let mut request = self.requests.pop_front()?;
        request.max_bytes = self.max_bytes();
        if let Some(entry) = self.entries.get_mut(&request.id) {
            entry.state = State::Requested;
        } else {
            self.module_requests
                .get_mut(&request.id)
                .expect("queued module request")
                .dispatched = true;
        }
        Some(request)
    }

    pub(crate) fn complete(
        &mut self,
        id: ClassicRequestId,
        result: Result<&[u8], ClassicFetchError>,
        source_limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<Option<ReadyClassic>, ScriptLoadError> {
        if let Some(fetch) = self.module_requests.get(&id) {
            if !fetch.dispatched {
                return Err(ScriptLoadError::UnknownScriptRequest);
            }
            let fetch = self
                .module_requests
                .remove(&id)
                .expect("dispatched module request");
            let key = ModuleKey::External(fetch.url.clone());
            if let Some(source) = result
                .ok()
                .and_then(|bytes| self.decode(bytes, source_limit))
            {
                self.parse_module_source(
                    key,
                    fetch.url,
                    source,
                    id.document,
                    fetch.mode,
                    cancellation,
                )?;
            } else {
                self.modules.set(key, ModuleState::Failed)?;
            }
            return Ok(None);
        }
        let Some(entry) = self
            .entries
            .get(&id)
            .filter(|entry| matches!(entry.state, State::Requested))
        else {
            return Err(ScriptLoadError::UnknownScriptRequest);
        };
        let mode = entry.mode;
        let element = entry.element;
        let source = result
            .ok()
            .and_then(|bytes| self.decode(bytes, source_limit));
        if mode == ClassicScriptMode::Async {
            self.entries.remove(&id);
            return Ok(Some(ReadyClassic { element, source }));
        }
        self.entries.get_mut(&id).expect("validated request").state = State::Ready(source);
        Ok(None)
    }

    pub(crate) fn take_runnable(&mut self, eof: bool) -> Result<Option<ReadyScript>, ScriptStop> {
        for (index, job) in self.async_modules.iter().enumerate() {
            if self.modules.ready(&job.key)? {
                return Ok(self.async_modules.remove(index).map(ReadyScript::Module));
            }
        }
        if let Some(id) = self.blocker {
            let result = self.take_ready(id);
            if result.is_some() {
                self.blocker = None;
            }
            return Ok(result.map(ReadyScript::Classic));
        }
        if eof {
            let result = match self.deferred.front() {
                Some(Deferred::Classic(id)) => self.take_ready(*id).map(ReadyScript::Classic),
                Some(Deferred::Module(job)) if self.modules.ready(&job.key)? => {
                    Some(ReadyScript::Module(job.as_ref().clone()))
                }
                _ => None,
            };
            if result.is_some() {
                self.deferred.pop_front();
            }
            return Ok(result);
        }
        Ok(None)
    }

    pub(crate) fn blocks_parser(&self) -> bool {
        self.blocker.is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
            && self.module_requests.is_empty()
            && self.async_modules.is_empty()
            && self.deferred.is_empty()
    }

    pub(crate) fn deferred_is_empty(&self) -> bool {
        self.deferred.is_empty()
    }

    fn take_ready(&mut self, id: ClassicRequestId) -> Option<ReadyClassic> {
        if !matches!(self.entries.get(&id)?.state, State::Ready(_)) {
            return None;
        }
        let Entry {
            element,
            state: State::Ready(source),
            ..
        } = self.entries.remove(&id)?
        else {
            unreachable!("ready request state checked");
        };
        Some(ReadyClassic { element, source })
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
        Ok(self.in_grant(&url).then_some(url))
    }

    fn in_grant(&self, url: &Url) -> bool {
        let Some(path) = local_file_path(url) else {
            return false;
        };
        let directory = local_file_path(&self.document_url)
            .and_then(|path| path.split_last().map(|(_, directory)| directory))
            .expect("local document URL validated at admission");
        path.len() > directory.len() && path.starts_with(directory)
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
