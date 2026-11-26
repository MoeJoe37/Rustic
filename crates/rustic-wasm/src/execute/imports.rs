use super::*;
use crate::{Declarations, ImportType};

type Callback<'host> = dyn Fn(&[Value], &mut [Value], &CancellationToken) -> Result<(), HostTrap>
    + Send
    + Sync
    + 'host;

/// Dispatch a declared function import by its zero-based function index.
/// The host receives typed arguments and correctly typed zero results to
/// fill. Cancellation is checked before and after the call by the interpreter.
pub type HostDispatch<'host> =
    dyn FnMut(usize, &[Value], &mut [Value]) -> Result<(), HostTrap> + 'host;

/// A numeric call that leaves the current instance.
#[derive(Clone, Copy)]
pub enum HostCall<'a> {
    /// An explicitly bound import by function index.
    Import(usize),
    /// An indirect call through a function address owned by another instance.
    Indirect {
        /// The defining instance and index.
        address: FunctionAddress,
        /// The caller's expected signature, checked before invoking the callee.
        expected: &'a FunctionType,
    },
}

/// A numeric host dispatcher with the host identities in suspended frames.
/// These identities and any indirect target must stay live during the call.
pub type TracingHostDispatch<'host> = dyn FnMut(
        HostCall<'_>,
        &[Value],
        &mut [Value],
        &mut dyn Iterator<Item = ReferenceRoot>,
    ) -> Result<(), HostTrap>
    + 'host;

pub(super) enum Dispatch<'call, 'host> {
    Plain(&'call mut HostDispatch<'host>),
    Tracing(&'call mut TracingHostDispatch<'host>),
}

/// A trap reported by an imported host function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostTrap;

/// One explicitly supplied host function in the module's import namespace.
pub struct FunctionImport<'host> {
    module: &'host str,
    name: &'host str,
    signature: &'host FunctionType,
    callback: &'host Callback<'host>,
}

impl<'host> FunctionImport<'host> {
    /// Attach a callback to a name and a scalar numeric signature.
    ///
    /// The interpreter supplies immutable arguments and a result slice initialized
    /// with correctly typed zeros. The callback can use the cancellation token
    /// for its own work. The embedder controls its capabilities and resource use.
    pub fn new<F>(
        module: &'host str,
        name: &'host str,
        signature: &'host FunctionType,
        callback: &'host F,
    ) -> Self
    where
        F: Fn(&[Value], &mut [Value], &CancellationToken) -> Result<(), HostTrap> + Send + Sync,
    {
        Self {
            module,
            name,
            signature,
            callback,
        }
    }
}

pub(super) struct BoundFunction<'module> {
    pub(super) type_index: u32,
    pub(super) callback: Option<&'module Callback<'module>>,
}

pub(super) fn link<'module>(
    declarations: &Declarations<'_>,
    bindings: &[FunctionImport<'module>],
    budget: &mut Budget<'_>,
) -> Result<Vec<BoundFunction<'module>>, ExecutionError> {
    let mut names = HashMap::new();
    for binding in bindings {
        budget.spend(0, 1)?;
        budget.spend(0, binding.module.len())?;
        budget.spend(0, binding.name.len())?;
        names
            .try_reserve(1)
            .map_err(|_| error(0, Fault::LimitExceeded))?;
        if names
            .insert((binding.module, binding.name), binding)
            .is_some()
        {
            return Err(error(0, Fault::DuplicateImportBinding));
        }
    }
    let mut functions = Vec::new();
    for entry in &declarations.imports {
        let offset = entry.offset;
        let import = &entry.value;
        budget.spend(offset, 1 + import.module.len() + import.name.len())?;
        let ImportType::Function(type_index) = import.ty else {
            return Err(error(offset, Fault::UnsupportedModule));
        };
        let binding = names
            .get(&(import.module, import.name))
            .ok_or_else(|| error(offset, Fault::MissingImport))?;
        let signature = &declarations.types[type_index as usize].value;
        let types = signature.params.iter().chain(&signature.results);
        budget.spend(offset, signature.params.len() + signature.results.len())?;
        if signature != binding.signature || !types.copied().all(is_numeric) {
            return Err(error(offset, Fault::ImportTypeMismatch));
        }
        append(
            &mut functions,
            BoundFunction {
                type_index,
                callback: Some(binding.callback),
            },
            offset,
        )?;
    }
    Ok(functions)
}

pub(super) fn for_dispatch(
    declarations: &Declarations<'_>,
    budget: &mut Budget<'_>,
) -> Result<Vec<BoundFunction<'static>>, ExecutionError> {
    let mut functions = Vec::new();
    for entry in &declarations.imports {
        let offset = entry.offset;
        budget.spend(offset, 1)?;
        let type_index = match entry.value.ty {
            ImportType::Function(index) => index,
            ImportType::Global(_) | ImportType::Memory(_) | ImportType::Table(_) => continue,
            _ => return Err(error(offset, Fault::UnsupportedModule)),
        };
        let signature = &declarations.types[type_index as usize].value;
        budget.spend(offset, signature.params.len() + signature.results.len())?;
        if !signature
            .params
            .iter()
            .chain(&signature.results)
            .copied()
            .all(is_numeric)
        {
            return Err(error(offset, Fault::ImportTypeMismatch));
        }
        append(
            &mut functions,
            BoundFunction {
                type_index,
                callback: None,
            },
            offset,
        )?;
    }
    Ok(functions)
}
