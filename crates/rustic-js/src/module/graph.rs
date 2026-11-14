use std::collections::{HashMap, HashSet};

use crate::{ParseError, ParseLimits, Source};

use super::{ExportTarget, ImportName, ModuleRecord, ModuleRequest, RequestIndex};

/// Input returned by an explicit module loader.
pub enum ModuleSource {
    /// ECMAScript source text.
    JavaScript(Source),
    /// JSON text for a synthetic default export.
    Json(Source),
    /// Uninterpreted UTF-16 text for a synthetic default export.
    Text(Source),
}

impl ModuleSource {
    fn len(&self) -> usize {
        match self {
            Self::JavaScript(source) | Self::Json(source) | Self::Text(source) => source.len(),
        }
    }
}

/// Budgets for loading a static module graph and resolving a single export.
#[derive(Clone, Debug)]
pub struct GraphLimits {
    /// Maximum number of distinct modules, including the entry module.
    pub max_modules: usize,
    /// Maximum total UTF-16 source length over distinct modules.
    pub max_source_units: usize,
    /// Maximum number of import and export resolution operations.
    pub max_resolution_steps: usize,
    /// Parser limits and cancellation shared with graph traversal.
    pub parse: ParseLimits,
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_modules: 1024,
            max_source_units: 4 * 1024 * 1024,
            max_resolution_steps: 1_000_000,
            parse: ParseLimits::default(),
        }
    }
}

/// Failure while loading or validating the static graph.
#[derive(Debug, PartialEq, Eq)]
pub enum GraphError<E> {
    /// The host rejected a request.
    Host(E),
    /// A module failed to parse.
    Parse(ParseError),
    /// The graph exceeded its module count or cumulative source limit.
    GraphLimit,
    /// Static import/export resolution exceeded its work limit.
    ResolutionLimit,
    /// The graph operation was cancelled.
    Cancelled,
    /// An import or explicit re-export has no unambiguous target.
    UnresolvedExport {
        /// Index of the module that requires the export.
        importer: usize,
        /// Exact UTF-16 spelling of the requested binding.
        name: Vec<u16>,
        /// Whether distinct bindings matched the same export name.
        ambiguous: bool,
    },
}

/// A resolved binding in its original defining module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedBinding {
    /// A locally declared binding.
    Local(Box<str>),
    /// A synthetic binding for an anonymous default export.
    Default,
    /// A module namespace.
    Namespace,
}

/// Result of resolving a static export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedExport {
    /// A unique binding.
    Binding {
        /// Index of the defining module in the graph.
        module: usize,
        /// Binding within that module.
        binding: ResolvedBinding,
    },
    /// No binding matched.
    Missing,
    /// Multiple distinct bindings matched through star exports.
    Ambiguous,
}

/// Static module dependencies resolved through explicit host callbacks.
///
/// Module keys are opaque to the engine. The host decides how to resolve
/// specifiers, enforce import attributes, and provide source.
pub struct ModuleGraph<K> {
    modules: Vec<Node<K>>,
    source_units: usize,
}

struct Node<K> {
    key: K,
    record: ModuleRecord,
    dependencies: Vec<usize>,
    exports: HashMap<Vec<u16>, usize>,
    data: Option<ModuleSource>,
}

impl<K> Node<K> {
    fn new(key: K, record: ModuleRecord, data: Option<ModuleSource>) -> Self {
        let exports = record
            .exports()
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.name.clone(), index))
            .collect();
        Self {
            key,
            record,
            dependencies: Vec::new(),
            exports,
            data,
        }
    }
}

impl<K: Clone + Eq + std::hash::Hash> ModuleGraph<K> {
    /// Load and validate the static graph from an entry key.
    ///
    /// The resolver sees the referrer's key and exact request. Source is read
    /// once per unique resolved key. Nothing is evaluated during validation.
    pub fn load<E>(
        entry: K,
        limits: &GraphLimits,
        resolve: impl FnMut(&K, &ModuleRequest) -> Result<K, E>,
        mut source: impl FnMut(&K) -> Result<Source, E>,
    ) -> Result<Self, GraphError<E>> {
        Self::load_sources(entry, limits, resolve, |key| {
            source(key).map(ModuleSource::JavaScript)
        })
    }

    /// Load a graph of JavaScript and synthetic data modules. The host
    /// decides which requests and attributes resolve to each module key.
    pub fn load_sources<E>(
        entry: K,
        limits: &GraphLimits,
        mut resolve: impl FnMut(&K, &ModuleRequest) -> Result<K, E>,
        mut source: impl FnMut(&K) -> Result<ModuleSource, E>,
    ) -> Result<Self, GraphError<E>> {
        let mut graph = Self {
            modules: Vec::new(),
            source_units: 0,
        };
        graph.add(entry.clone(), limits, &mut source)?;
        let mut indexes = HashMap::from([(entry, 0)]);
        let mut index = 0;
        while index < graph.modules.len() {
            check(limits)?;
            let mut dependencies = Vec::new();
            for request_index in 0..graph.modules[index].record.requests().len() {
                check(limits)?;
                let node = &graph.modules[index];
                let request = &node.record.requests()[request_index];
                let key = resolve(&node.key, request).map_err(GraphError::Host)?;
                let target = if let Some(&target) = indexes.get(&key) {
                    target
                } else {
                    let target = graph.modules.len();
                    graph.add(key.clone(), limits, &mut source)?;
                    indexes.insert(key, target);
                    target
                };
                dependencies.push(target);
            }
            graph.modules[index].dependencies = dependencies;
            index += 1;
        }
        graph.validate(limits)?;
        Ok(graph)
    }

    fn add<E>(
        &mut self,
        key: K,
        limits: &GraphLimits,
        source: &mut impl FnMut(&K) -> Result<ModuleSource, E>,
    ) -> Result<(), GraphError<E>> {
        check(limits)?;
        if self.modules.len() == limits.max_modules {
            return Err(GraphError::GraphLimit);
        }
        let input = source(&key).map_err(GraphError::Host)?;
        self.source_units = self
            .source_units
            .checked_add(input.len())
            .filter(|total| *total <= limits.max_source_units)
            .ok_or(GraphError::GraphLimit)?;
        let (record, data) = match input {
            ModuleSource::JavaScript(source) => {
                let record =
                    ModuleRecord::parse(&source, &limits.parse).map_err(|error| match error {
                        ParseError::Cancelled => GraphError::Cancelled,
                        other => GraphError::Parse(other),
                    })?;
                (record, None)
            }
            data => (ModuleRecord::synthetic_default(), Some(data)),
        };
        self.modules.push(Node::new(key, record, data));
        Ok(())
    }

    pub(super) fn data(&self, index: usize) -> Option<&ModuleSource> {
        self.modules[index].data.as_ref()
    }

    pub(super) fn dependencies(&self, index: usize) -> &[usize] {
        &self.modules[index].dependencies
    }

    pub(super) fn module_count(&self) -> usize {
        self.modules.len()
    }

    pub(super) fn source_units(&self) -> usize {
        self.source_units
    }

    /// The module record at the given index, where zero is the entry module.
    pub fn module(&self, index: usize) -> Option<(&K, &ModuleRecord)> {
        self.modules
            .get(index)
            .map(|node| (&node.key, &node.record))
    }

    /// Graph index of the module selected by a record-local request.
    pub fn dependency(&self, index: usize, request: RequestIndex) -> Option<usize> {
        self.modules
            .get(index)?
            .dependencies
            .get(request.index())
            .copied()
    }

    /// Export names collected across star edges, sorted by UTF-16 code unit.
    /// Names with ambiguous bindings are included; resolve them separately.
    pub fn exported_names(
        &self,
        index: usize,
        limits: &GraphLimits,
    ) -> Result<Vec<Vec<u16>>, GraphError<std::convert::Infallible>> {
        let mut remaining = limits.max_resolution_steps;
        self.collect_exported_names(index, limits, &mut remaining)
    }

    pub(super) fn collect_exported_names(
        &self,
        index: usize,
        limits: &GraphLimits,
        remaining: &mut usize,
    ) -> Result<Vec<Vec<u16>>, GraphError<std::convert::Infallible>> {
        let mut seen = HashSet::new();
        let mut names = HashSet::new();
        let mut pending = vec![index];
        while let Some(module) = pending.pop() {
            spend(limits, remaining)?;
            if !seen.insert(module) {
                continue;
            }
            let Some(node) = self.modules.get(module) else {
                continue;
            };
            for entry in node.record.exports() {
                spend(limits, remaining)?;
                if module == index || !is_default(&entry.name) {
                    names.insert(entry.name.as_slice());
                }
            }
            for request in node.record.star_exports().iter().rev() {
                spend(limits, remaining)?;
                pending.push(node.dependencies[request.index()]);
            }
        }
        let mut names: Vec<_> = names.into_iter().map(<[u16]>::to_vec).collect();
        names.sort_unstable();
        check(limits)?;
        Ok(names)
    }

    /// Resolve one name from the given module. An unknown index returns missing.
    pub fn resolve_export(
        &self,
        index: usize,
        name: &[u16],
        limits: &GraphLimits,
    ) -> Result<ResolvedExport, GraphError<std::convert::Infallible>> {
        let mut remaining = limits.max_resolution_steps;
        self.resolve(index, name, limits, &mut remaining)
    }

    fn validate<E>(&self, limits: &GraphLimits) -> Result<(), GraphError<E>> {
        let mut remaining = limits.max_resolution_steps;
        for (importer, node) in self.modules.iter().enumerate() {
            check(limits)?;
            let imports = node
                .record
                .imports()
                .iter()
                .map(|entry| (entry.request, &entry.imported));
            let reexports = node
                .record
                .exports()
                .iter()
                .filter_map(|entry| match &entry.target {
                    ExportTarget::Indirect { request, imported } => Some((*request, imported)),
                    _ => None,
                });
            for (request, imported) in imports.chain(reexports) {
                if let ImportName::Binding(name) = imported {
                    let index = node.dependencies[request.index()];
                    match self.resolve(index, name, limits, &mut remaining)? {
                        ResolvedExport::Binding { .. } => {}
                        other => {
                            return Err(GraphError::UnresolvedExport {
                                importer,
                                name: name.clone(),
                                ambiguous: other == ResolvedExport::Ambiguous,
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn resolve<E>(
        &self,
        index: usize,
        name: &[u16],
        limits: &GraphLimits,
        remaining: &mut usize,
    ) -> Result<ResolvedExport, GraphError<E>> {
        let mut seen = HashSet::new();
        let mut pending = vec![(index, name)];
        let mut result = ResolvedExport::Missing;
        while let Some((module, name)) = pending.pop() {
            spend(limits, remaining)?;
            if !seen.insert((module, name)) {
                continue;
            }
            let Some(node) = self.modules.get(module) else {
                continue;
            };
            let binding = match node
                .exports
                .get(name)
                .map(|&i| &node.record.exports()[i].target)
            {
                Some(ExportTarget::Local(local)) => (module, ResolvedBinding::Local(local.clone())),
                Some(ExportTarget::Default) => (module, ResolvedBinding::Default),
                Some(ExportTarget::Indirect { request, imported }) => {
                    let target = node.dependencies[request.index()];
                    match imported {
                        ImportName::Namespace => (target, ResolvedBinding::Namespace),
                        ImportName::Binding(name) => {
                            pending.push((target, name));
                            continue;
                        }
                    }
                }
                None => {
                    if !is_default(name) {
                        for request in node.record.star_exports().iter().rev() {
                            spend(limits, remaining)?;
                            pending.push((node.dependencies[request.index()], name));
                        }
                    }
                    continue;
                }
            };
            let candidate = ResolvedExport::Binding {
                module: binding.0,
                binding: binding.1,
            };
            if result != ResolvedExport::Missing && result != candidate {
                return Ok(ResolvedExport::Ambiguous);
            }
            result = candidate;
        }
        Ok(result)
    }
}

fn check<E>(limits: &GraphLimits) -> Result<(), GraphError<E>> {
    limits
        .parse
        .cancellation
        .check()
        .map_err(|_| GraphError::Cancelled)
}

pub(super) fn spend<E>(limits: &GraphLimits, remaining: &mut usize) -> Result<(), GraphError<E>> {
    check(limits)?;
    *remaining = remaining
        .checked_sub(1)
        .ok_or(GraphError::ResolutionLimit)?;
    Ok(())
}

fn is_default(name: &[u16]) -> bool {
    name.iter().copied().eq("default".encode_utf16())
}
