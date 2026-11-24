use super::*;
use crate::{Declarations, ImportType};

type Callback<'host> = dyn Fn(&[Value], &mut [Value], &CancellationToken) -> Result<(), HostTrap>
    + Send
    + Sync
    + 'host;

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
    pub(super) callback: &'module Callback<'module>,
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
                callback: binding.callback,
            },
            offset,
        )?;
    }
    Ok(functions)
}
