use super::state::{GlobalSlot, Transfer, checked_range, constant};
use super::*;
use crate::{Declarations, ElementItems, ElementMode, Limits, TableType};

const OUT_OF_BOUNDS: Fault = Fault::TableOutOfBounds;

#[derive(Default)]
pub(super) struct Tables {
    tables: Vec<Table>,
    elements: Vec<Vec<Reference>>,
    used: usize,
    limit: usize,
}

struct Table {
    entries: Vec<Reference>,
    ty: TableType,
}

impl Instance<'_, '_> {
    /// Read the declared type of a module-defined table.
    pub fn table_type(&self, table: u32) -> Option<TableType> {
        Some(self.state.borrow().tables.tables.get(table as usize)?.ty)
    }

    /// Read the current length of a module-defined table.
    pub fn table_len(&self, table: u32) -> Option<u64> {
        Some(
            self.state
                .borrow()
                .tables
                .tables
                .get(table as usize)?
                .entries
                .len() as u64,
        )
    }

    /// Read an external table entry. The inner `None` represents null.
    pub fn external_table_get(&self, table: u32, index: u64) -> Option<Option<ExternalReference>> {
        let state = self.state.borrow();
        let table = state.tables.reference_table(table, -17)?;
        match table.entries.get(usize::try_from(index).ok()?)? {
            Reference::Extern(reference) => Some(Some(*reference)),
            Reference::NullExtern => Some(None),
            Reference::Function(_) => unreachable!("validated external table entry"),
        }
    }

    /// Write a host identity or null to a nullable external table.
    /// The embedder must keep the identity alive whenever it is reachable.
    pub fn external_table_set(
        &self,
        table: u32,
        index: u64,
        reference: Option<ExternalReference>,
    ) -> bool {
        let mut state = self.state.borrow_mut();
        let value = reference.map_or(Reference::NullExtern, Reference::Extern);
        if state.tables.reference_table(table, -17).is_none()
            || state.tables.set(table, index, value, 0).is_err()
        {
            return false;
        }
        state.has_external_references |= reference.is_some();
        true
    }

    /// Visit the external identities reachable from persistent guest state.
    /// The visitor must not reenter the instance.
    pub fn visit_external_references(&self, visitor: &mut dyn FnMut(ExternalReference)) {
        let state = self.state.borrow();
        if !state.has_external_references {
            return;
        }
        state.tables.visit_external_references(visitor);
        for reference in state
            .globals
            .iter()
            .filter_map(|slot| slot.value().external_reference())
        {
            visitor(reference);
        }
    }

    /// Read the declared limits of a nullable, module-defined function table.
    pub fn function_table_limits(&self, table: u32) -> Option<Limits> {
        Some(
            self.state
                .borrow()
                .tables
                .reference_table(table, -16)?
                .ty
                .limits,
        )
    }

    /// Read the current length of a nullable function table.
    pub fn function_table_len(&self, table: u32) -> Option<u64> {
        Some(
            self.state
                .borrow()
                .tables
                .reference_table(table, -16)?
                .entries
                .len() as u64,
        )
    }

    /// Read a function table entry. The inner `None` represents a null reference.
    pub fn function_table_get(&self, table: u32, index: u64) -> Option<Option<u32>> {
        let state = self.state.borrow();
        let table = state.tables.reference_table(table, -16)?;
        let index = usize::try_from(index).ok()?;
        let Reference::Function(function) = *table.entries.get(index)? else {
            unreachable!("validated function table entry")
        };
        Some(function)
    }

    /// Write a function by its index in this instance, or write null. Invalid
    /// indices or table types leave the table unchanged.
    pub fn function_table_set(&self, table: u32, index: u64, function: Option<u32>) -> bool {
        if function.is_some_and(|index| self.function_type_at(index).is_none()) {
            return false;
        }
        let mut state = self.state.borrow_mut();
        state.tables.reference_table(table, -16).is_some()
            && state
                .tables
                .set(table, index, Reference::Function(function), 0)
                .is_ok()
    }

    /// Return the work growth would require, if admitted by the table and
    /// instance limits. The host can charge that work before entering the store.
    pub fn function_table_growth_work(&self, table: u32, count: u64) -> Option<usize> {
        self.function_table_limits(table)?;
        self.table_growth_work(table, count)
    }

    /// Preview the admitted work for growing a module-defined table.
    pub fn table_growth_work(&self, table: u32, count: u64) -> Option<usize> {
        let state = self.state.borrow();
        state.tables.tables.get(table as usize)?;
        state.tables.growth(table, count).map(|(_, _, work)| work)
    }

    /// Grow a function table using the same budget and limits as guest growth.
    /// Return the previous length or `None` for invalid or unallocatable growth.
    pub fn grow_function_table(
        &self,
        table: u32,
        count: u64,
        function: Option<u32>,
        cancellation: &CancellationToken,
    ) -> Result<Option<u64>, ExecutionError> {
        if function.is_some_and(|index| self.function_type_at(index).is_none()) {
            return Ok(None);
        }
        self.grow_host_table(
            table,
            count,
            -16,
            Reference::Function(function),
            cancellation,
        )
    }

    /// Grow a nullable external table and fill the new entries with the host
    /// identity or null. The embedder must keep reachable identities alive.
    pub fn grow_external_table(
        &self,
        table: u32,
        count: u64,
        reference: Option<ExternalReference>,
        cancellation: &CancellationToken,
    ) -> Result<Option<u64>, ExecutionError> {
        let value = reference.map_or(Reference::NullExtern, Reference::Extern);
        self.grow_host_table(table, count, -17, value, cancellation)
    }

    fn grow_host_table(
        &self,
        table: u32,
        count: u64,
        heap_type: i64,
        value: Reference,
        cancellation: &CancellationToken,
    ) -> Result<Option<u64>, ExecutionError> {
        let mut state = self.state.borrow_mut();
        if state.tables.reference_table(table, heap_type).is_none() {
            return Ok(None);
        }
        let mut budget = Budget {
            remaining: self.executable.limits.invocation_work,
            cancellation,
        };
        let previous = state
            .tables
            .grow_entries(table, count, value, 0, &mut budget)?;
        state.has_external_references |=
            count != 0 && previous.is_some() && matches!(value, Reference::Extern(_));
        Ok(previous)
    }
}

impl Tables {
    pub(super) fn new(
        declarations: &Declarations<'_>,
        globals: &[GlobalSlot],
        limits: InstanceLimits,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ExecutionError> {
        let mut state = Self {
            limit: limits.table_elements,
            ..Self::default()
        };
        let stack_limit = limits.execution.values;
        for entry in &declarations.tables {
            let offset = entry.offset;
            let table = &entry.value;
            let count = usize::try_from(table.ty.limits.min)
                .map_err(|_| error(offset, Fault::LimitExceeded))?;
            budget.spend(offset, 1)?;
            state.reserve(count, offset)?;
            budget.spend(offset, count)?;
            let initial = match &table.initializer {
                Some(expr) => constant(globals, expr, stack_limit, budget)?,
                None => Slot::zero(table.ty.element_type),
            };
            let mut entries = Vec::new();
            entries
                .try_reserve_exact(count)
                .map_err(|_| error(offset, Fault::LimitExceeded))?;
            entries.resize(count, initial.reference());
            append(
                &mut state.tables,
                Table {
                    entries,
                    ty: table.ty,
                },
                offset,
            )?;
        }

        for entry in &declarations.elements {
            let offset = entry.offset;
            let element = &entry.value;
            budget.spend(offset, 1)?;
            let count = match &element.items {
                ElementItems::Functions(indexes) => indexes.len(),
                ElementItems::Expressions(expressions) => expressions.len(),
            };
            let mut passive = Vec::new();
            match &element.mode {
                ElementMode::Passive => {
                    state.reserve(count, offset)?;
                    budget.spend(offset, count)?;
                    passive
                        .try_reserve_exact(count)
                        .map_err(|_| error(offset, Fault::LimitExceeded))?;
                    for index in 0..count {
                        passive.push(element_value(
                            &element.items,
                            index,
                            globals,
                            stack_limit,
                            offset,
                            budget,
                        )?);
                    }
                }
                ElementMode::Active(active) => {
                    let start = constant(globals, &active.offset, stack_limit, budget)?.address();
                    let target = &mut state.tables[active.target as usize].entries;
                    let range =
                        checked_range(target.len(), start, count as u64, offset, OUT_OF_BOUNDS)?;
                    budget.spend(offset, count)?;
                    // No state is exposed until every segment and the start function succeed.
                    for (index, slot) in target[range].iter_mut().enumerate() {
                        *slot = element_value(
                            &element.items,
                            index,
                            globals,
                            stack_limit,
                            offset,
                            budget,
                        )?;
                    }
                }
                ElementMode::Declarative => {}
            }
            append(&mut state.elements, passive, offset)?;
        }
        Ok(state)
    }

    fn reserve(&mut self, entries: usize, offset: usize) -> Result<(), ExecutionError> {
        if entries > self.limit - self.used {
            return Err(error(offset, Fault::LimitExceeded));
        }
        self.used += entries;
        Ok(())
    }

    pub(super) fn get(
        &self,
        table: u32,
        index: u64,
        offset: usize,
    ) -> Result<Reference, ExecutionError> {
        let entries = &self.tables[table as usize].entries;
        let range = checked_range(entries.len(), index, 1, offset, OUT_OF_BOUNDS)?;
        Ok(entries[range.start])
    }

    pub(super) fn set(
        &mut self,
        table: u32,
        index: u64,
        value: Reference,
        offset: usize,
    ) -> Result<(), ExecutionError> {
        let entries = &mut self.tables[table as usize].entries;
        let range = checked_range(entries.len(), index, 1, offset, OUT_OF_BOUNDS)?;
        entries[range.start] = value;
        Ok(())
    }

    pub(super) fn size(&self, table: u32) -> Value {
        let table = &self.tables[table as usize];
        table
            .ty
            .limits
            .address_type
            .index_value(table.entries.len() as u64)
    }

    fn reference_table(&self, index: u32, heap_type: i64) -> Option<&Table> {
        let table = self.tables.get(index as usize)?;
        (table.ty.element_type
            == ValueType::Ref {
                nullable: true,
                heap_type,
            })
        .then_some(table)
    }

    fn visit_external_references(&self, visitor: &mut dyn FnMut(ExternalReference)) {
        let tables = self
            .tables
            .iter()
            .filter(|table| matches!(table.ty.element_type, ValueType::Ref { heap_type: -17, .. }))
            .map(|table| &table.entries);
        for entry in tables.chain(&self.elements).flatten() {
            if let Reference::Extern(reference) = *entry {
                visitor(reference);
            }
        }
    }

    fn growth(&self, table: u32, count: u64) -> Option<(usize, usize, usize)> {
        let table = &self.tables[table as usize];
        let count = usize::try_from(count)
            .ok()
            .filter(|&count| count <= self.limit - self.used)?;
        let limits = table.ty.limits;
        let max = limits
            .max
            .unwrap_or_else(|| limits.address_type.max_table_elements());
        let new = table
            .entries
            .len()
            .checked_add(count)
            .filter(|&new| new as u64 <= max)?;
        let work = if new > table.entries.capacity() {
            new
        } else {
            count
        };
        Some((count, new, work))
    }

    pub(super) fn grow(
        &mut self,
        table: u32,
        count: u64,
        value: Reference,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Value, ExecutionError> {
        let address = self.tables[table as usize].ty.limits.address_type;
        let previous = self.grow_entries(table, count, value, offset, budget)?;
        Ok(address.index_value(previous.unwrap_or(u64::MAX)))
    }

    fn grow_entries(
        &mut self,
        table: u32,
        count: u64,
        value: Reference,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Option<u64>, ExecutionError> {
        let Some((count, new, work)) = self.growth(table, count) else {
            return Ok(None);
        };
        budget.spend(offset, work)?;
        let table = &mut self.tables[table as usize];
        let old = table.entries.len();
        if table.entries.try_reserve_exact(count).is_err() {
            return Ok(None);
        }
        table.entries.resize(new, value);
        self.used += count;
        Ok(Some(old as u64))
    }

    pub(super) fn init(
        &mut self,
        element: u32,
        table: u32,
        transfer: Transfer,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), ExecutionError> {
        let source = &self.elements[element as usize];
        let target = &mut self.tables[table as usize].entries;
        let (src, dst) = transfer.ranges(source.len(), target.len(), offset, OUT_OF_BOUNDS)?;
        budget.spend(offset, dst.len())?;
        target[dst].copy_from_slice(&source[src]);
        Ok(())
    }

    pub(super) fn drop_element(&mut self, element: u32) {
        let previous = std::mem::take(&mut self.elements[element as usize]);
        self.used -= previous.len();
    }

    pub(super) fn copy(
        &mut self,
        destination: u32,
        source: u32,
        transfer: Transfer,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), ExecutionError> {
        let (destination, source) = (destination as usize, source as usize);
        let (src, dst) = transfer.ranges(
            self.tables[source].entries.len(),
            self.tables[destination].entries.len(),
            offset,
            OUT_OF_BOUNDS,
        )?;
        budget.spend(offset, dst.len())?;
        if destination == source {
            self.tables[destination].entries.copy_within(src, dst.start);
        } else {
            let (before, after) = self.tables.split_at_mut(destination.max(source));
            let (target, origin) = if destination < source {
                (&mut before[destination], &after[0])
            } else {
                (&mut after[0], &before[source])
            };
            target.entries[dst].copy_from_slice(&origin.entries[src]);
        }
        Ok(())
    }

    pub(super) fn fill(
        &mut self,
        table: u32,
        index: u64,
        count: u64,
        value: Reference,
        budget: &mut Budget<'_>,
        offset: usize,
    ) -> Result<(), ExecutionError> {
        let entries = &mut self.tables[table as usize].entries;
        let range = checked_range(entries.len(), index, count, offset, OUT_OF_BOUNDS)?;
        budget.spend(offset, range.len())?;
        entries[range].fill(value);
        Ok(())
    }
}

fn element_value(
    items: &ElementItems,
    index: usize,
    globals: &[GlobalSlot],
    stack_limit: usize,
    offset: usize,
    budget: &mut Budget<'_>,
) -> Result<Reference, ExecutionError> {
    budget.spend(offset, 0)?;
    Ok(match items {
        ElementItems::Functions(indexes) => Reference::Function(Some(indexes[index])),
        ElementItems::Expressions(expr) => {
            constant(globals, &expr[index], stack_limit, budget)?.reference()
        }
    })
}
