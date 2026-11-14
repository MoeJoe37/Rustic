//! Static source text module records (ECMA-262 §16.2.1.7).

use std::collections::HashMap;

mod asynchronous;
mod evaluate;
mod graph;
pub(crate) use asynchronous::AsyncModuleGraph;
pub use evaluate::ModuleEvalError;
pub(crate) use evaluate::ModuleInstance;
pub use graph::{GraphError, GraphLimits, ModuleGraph, ResolvedBinding, ResolvedExport};

use crate::ast::{
    DefaultExport, ExportDeclaration, Identifier, ImportAttribute, ImportSpecifier,
    ModuleExportName, Program, Statement, StatementKind,
};
use crate::{ParseError, ParseLimits, Source, Span, parse_module};

/// A module request keyed by exact UTF-16 specifier and attributes.
/// Attributes are sorted by key, so their source order does not affect identity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModuleRequest {
    specifier: Vec<u16>,
    attributes: Vec<(Vec<u16>, Vec<u16>)>,
}

impl ModuleRequest {
    /// The specifier as written, before host resolution.
    pub fn specifier(&self) -> &[u16] {
        &self.specifier
    }

    /// Import attribute names and values, in UTF-16 key order.
    pub fn attributes(&self) -> &[(Vec<u16>, Vec<u16>)] {
        &self.attributes
    }
}

/// Index of a requested module in its enclosing source text module record.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RequestIndex(usize);

impl RequestIndex {
    /// Offset into the record's request list.
    pub fn index(self) -> usize {
        self.0
    }
}

/// What a binding imports or forwards from another module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportName {
    /// A named binding, including the default export.
    Binding(Vec<u16>),
    /// The requested module's namespace.
    Namespace,
}

/// An import entry associated with a local identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportEntry {
    /// Request in this record.
    pub request: RequestIndex,
    /// Binding name or namespace in the requested module.
    pub imported: ImportName,
    /// Local binding name.
    pub local: Box<str>,
    /// Location of the local binding.
    pub span: Span,
}

/// A locally defined binding or a binding forwarded from another module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportTarget {
    /// A binding in the declaring module.
    Local(Box<str>),
    /// The synthetic binding of an anonymous default declaration or expression.
    Default,
    /// A binding or namespace in a requested module.
    Indirect {
        /// Request in this record.
        request: RequestIndex,
        /// Binding name or namespace in the requested module.
        imported: ImportName,
    },
}

/// A directly named export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportEntry {
    /// Exact UTF-16 export name.
    pub name: Vec<u16>,
    /// Source of the exported binding.
    pub target: ExportTarget,
}

/// Parsed module code and static linker inputs.
/// Request indices belong to the enclosing record.
#[derive(Debug)]
pub struct ModuleRecord {
    program: Program,
    requests: Vec<ModuleRequest>,
    imports: Vec<ImportEntry>,
    exports: Vec<ExportEntry>,
    star_exports: Vec<RequestIndex>,
}

impl ModuleRecord {
    /// Parse module source and derive the static linker inputs.
    pub fn parse(source: &Source, limits: &ParseLimits) -> Result<Self, ParseError> {
        let program = parse_module(source, limits)?;
        let mut requests = RequestTable::default();
        let mut imports = Vec::new();
        let mut imported_locals = HashMap::new();
        let mut reexports = HashMap::new();

        for (index, statement) in program.body.iter().enumerate() {
            limits
                .cancellation
                .check()
                .map_err(|_| ParseError::Cancelled)?;
            match &statement.kind {
                StatementKind::Import(declaration) => {
                    let request = requests.insert(&declaration.source, &declaration.attributes);
                    for specifier in &declaration.specifiers {
                        limits
                            .cancellation
                            .check()
                            .map_err(|_| ParseError::Cancelled)?;
                        let (local, imported) = match specifier {
                            ImportSpecifier::Default(local) => {
                                (local, ImportName::Binding(default_name()))
                            }
                            ImportSpecifier::Namespace(local) => (local, ImportName::Namespace),
                            ImportSpecifier::Named { imported, local } => {
                                (local, ImportName::Binding(export_name(imported)))
                            }
                        };
                        imported_locals.insert(local.name.as_ref(), imports.len());
                        imports.push(ImportEntry {
                            request,
                            imported,
                            local: local.name.clone(),
                            span: local.span,
                        });
                    }
                }
                StatementKind::Export(
                    ExportDeclaration::Named {
                        source: Some(source),
                        attributes,
                        ..
                    }
                    | ExportDeclaration::All {
                        source, attributes, ..
                    },
                ) => {
                    reexports.insert(index, requests.insert(source, attributes));
                }
                _ => {}
            }
        }

        let mut exports = Vec::new();
        let mut star_exports = Vec::new();
        for (index, statement) in program.body.iter().enumerate() {
            limits
                .cancellation
                .check()
                .map_err(|_| ParseError::Cancelled)?;
            let StatementKind::Export(declaration) = &statement.kind else {
                continue;
            };
            match declaration {
                ExportDeclaration::Named { specifiers, .. } => {
                    for specifier in specifiers {
                        limits
                            .cancellation
                            .check()
                            .map_err(|_| ParseError::Cancelled)?;
                        let target = if let Some(&request) = reexports.get(&index) {
                            ExportTarget::Indirect {
                                request,
                                imported: ImportName::Binding(export_name(&specifier.local)),
                            }
                        } else {
                            let ModuleExportName::Identifier(local) = &specifier.local else {
                                unreachable!(
                                    "module parser requires an identifier for local exports"
                                );
                            };
                            match imported_locals
                                .get(local.name.as_ref())
                                .map(|&entry| &imports[entry])
                            {
                                Some(ImportEntry {
                                    request, imported, ..
                                }) => ExportTarget::Indirect {
                                    request: *request,
                                    imported: imported.clone(),
                                },
                                _ => ExportTarget::Local(local.name.clone()),
                            }
                        };
                        exports.push(ExportEntry {
                            name: export_name(&specifier.exported),
                            target,
                        });
                    }
                }
                ExportDeclaration::All { exported, .. } => {
                    let request = reexports[&index];
                    if let Some(name) = exported {
                        exports.push(ExportEntry {
                            name: export_name(name),
                            target: ExportTarget::Indirect {
                                request,
                                imported: ImportName::Namespace,
                            },
                        });
                    } else {
                        star_exports.push(request);
                    }
                }
                ExportDeclaration::Declaration(declaration) => {
                    for name in declaration_names(declaration) {
                        limits
                            .cancellation
                            .check()
                            .map_err(|_| ParseError::Cancelled)?;
                        exports.push(ExportEntry {
                            name: name.name.encode_utf16().collect(),
                            target: ExportTarget::Local(name.name.clone()),
                        });
                    }
                }
                ExportDeclaration::Default(declaration) => {
                    let local = match declaration {
                        DefaultExport::Function(function) => function.id.as_ref(),
                        DefaultExport::Class(class) => class.id.as_ref(),
                        DefaultExport::Expression(_) => None,
                    };
                    exports.push(ExportEntry {
                        name: default_name(),
                        target: local.map_or(ExportTarget::Default, |id| {
                            ExportTarget::Local(id.name.clone())
                        }),
                    });
                }
            }
        }
        Ok(Self {
            program,
            requests: requests.into_requests(),
            imports,
            exports,
            star_exports,
        })
    }

    /// The parsed module body.
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Distinct static requests, ordered by their first occurrence.
    pub fn requests(&self) -> &[ModuleRequest] {
        &self.requests
    }

    /// Imported bindings in source order.
    pub fn imports(&self) -> &[ImportEntry] {
        &self.imports
    }

    /// Explicitly named exports in source order.
    pub fn exports(&self) -> &[ExportEntry] {
        &self.exports
    }

    /// Requests targeted by bare star exports, in source order.
    pub fn star_exports(&self) -> &[RequestIndex] {
        &self.star_exports
    }
}

#[derive(Default)]
struct RequestTable(HashMap<ModuleRequest, RequestIndex>);

impl RequestTable {
    fn insert(&mut self, specifier: &[u16], attributes: &[ImportAttribute]) -> RequestIndex {
        let mut attributes: Vec<_> = attributes
            .iter()
            .map(|a| (a.key.clone(), a.value.clone()))
            .collect();
        attributes.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let index = RequestIndex(self.0.len());
        *self
            .0
            .entry(ModuleRequest {
                specifier: specifier.to_vec(),
                attributes,
            })
            .or_insert(index)
    }

    fn into_requests(self) -> Vec<ModuleRequest> {
        let mut entries: Vec<_> = self.0.into_iter().collect();
        entries.sort_unstable_by_key(|(_, index)| index.0);
        entries.into_iter().map(|(request, _)| request).collect()
    }
}

fn export_name(name: &ModuleExportName) -> Vec<u16> {
    match name {
        ModuleExportName::Identifier(id) => id.name.encode_utf16().collect(),
        ModuleExportName::String(units, _) => units.clone(),
    }
}

fn default_name() -> Vec<u16> {
    "default".encode_utf16().collect()
}

fn declaration_names(declaration: &Statement) -> Vec<&Identifier> {
    let mut names = Vec::new();
    match &declaration.kind {
        StatementKind::Variable(variable) => {
            for declaration in &variable.declarations {
                declaration.target.bound_identifiers(&mut names);
            }
        }
        StatementKind::Function(function) => names.extend(function.id.as_ref()),
        StatementKind::Class(class) => names.extend(class.id.as_ref()),
        _ => unreachable!("module parser checks exported declaration kinds"),
    }
    names
}
