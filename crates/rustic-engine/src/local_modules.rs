//! Parsed local module records and dependency readiness.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rustic_dom::NodeId;
use rustic_js::module::{GraphError, GraphLimits, ModuleGraph, ModuleRecord, ModuleRequest};
use rustic_url::Url;

use crate::ScriptStop;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ModuleKey {
    Inline(NodeId),
    External(Url),
}

pub(crate) enum ModuleState {
    Fetching,
    Failed,
    Parsed {
        inline_base: Option<Box<Url>>,
        record: Arc<ModuleRecord>,
        units: usize,
        dependencies: Vec<ModuleKey>,
    },
}

#[derive(Default)]
pub(crate) struct LocalModules {
    records: HashMap<ModuleKey, ModuleState>,
}

impl LocalModules {
    pub(crate) fn contains(&self, key: &ModuleKey) -> bool {
        self.records.contains_key(key)
    }

    pub(crate) fn set(&mut self, key: ModuleKey, state: ModuleState) -> Result<(), ScriptStop> {
        self.records
            .try_reserve(1)
            .map_err(|_| ScriptStop::OutOfMemory)?;
        self.records.insert(key, state);
        Ok(())
    }

    pub(crate) fn ready(&self, entry: &ModuleKey) -> Result<bool, ScriptStop> {
        let mut visited = HashSet::new();
        let mut pending = vec![entry];
        let mut ready = true;
        while let Some(key) = pending.pop() {
            visited
                .try_reserve(1)
                .map_err(|_| ScriptStop::OutOfMemory)?;
            if !visited.insert(key) {
                continue;
            }
            match self.records.get(key) {
                Some(ModuleState::Parsed { dependencies, .. }) => {
                    pending
                        .try_reserve(dependencies.len())
                        .map_err(|_| ScriptStop::OutOfMemory)?;
                    pending.extend(dependencies);
                }
                Some(ModuleState::Fetching) => ready = false,
                Some(ModuleState::Failed) | None => return Ok(true),
            }
        }
        Ok(ready)
    }

    pub(crate) fn graph(
        &self,
        entry: ModuleKey,
        limits: &GraphLimits,
    ) -> Result<ModuleGraph<ModuleKey>, GraphError<()>> {
        ModuleGraph::load_records(
            entry,
            limits,
            |referrer, request| {
                let base = self.base(referrer).ok_or(())?;
                resolve_import(base, request)
                    .map(ModuleKey::External)
                    .ok_or(())
            },
            |key| match self.records.get(key) {
                Some(ModuleState::Parsed { record, units, .. }) => Ok((record.clone(), *units)),
                _ => Err(()),
            },
        )
    }

    pub(crate) fn base<'a>(&'a self, key: &'a ModuleKey) -> Option<&'a Url> {
        match key {
            ModuleKey::External(url) => Some(url),
            ModuleKey::Inline(_) => match self.records.get(key) {
                Some(ModuleState::Parsed { inline_base, .. }) => inline_base.as_deref(),
                _ => None,
            },
        }
    }
}

pub(crate) fn resolve_import(base: &Url, request: &ModuleRequest) -> Option<Url> {
    if !request.attributes().is_empty() {
        return None;
    }
    let specifier = String::from_utf16_lossy(request.specifier());
    if specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") {
        Url::parse(&specifier, Some(base)).ok()
    } else {
        Url::parse(&specifier, None).ok()
    }
}
