use super::state::{GlobalSlot, Transfer, checked_range, constant};
use super::*;
use crate::{Declarations, ElementItems, ElementMode};

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
    address: AddressType,
    max: u64,
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
            let address = table.ty.limits.address_type;
            let max = table.ty.limits.max.unwrap_or(address.max_table_elements());
            append(
                &mut state.tables,
                Table {
                    entries,
                    address,
                    max,
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
        table.address.index_value(table.entries.len() as u64)
    }

    pub(super) fn grow(
        &mut self,
        table: u32,
        count: u64,
        value: Reference,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Value, ExecutionError> {
        let table = &mut self.tables[table as usize];
        let old = table.entries.len();
        let failed = table.address.index_value(u64::MAX);
        let Some(count) = usize::try_from(count)
            .ok()
            .filter(|&count| count <= self.limit - self.used)
        else {
            return Ok(failed);
        };
        let Some(new) = old
            .checked_add(count)
            .filter(|&new| new as u64 <= table.max)
        else {
            return Ok(failed);
        };
        let work = if new > table.entries.capacity() {
            new
        } else {
            count
        };
        budget.spend(offset, work)?;
        if table.entries.try_reserve_exact(count).is_err() {
            return Ok(failed);
        }
        table.entries.resize(new, value);
        self.used += count;
        Ok(table.address.index_value(old as u64))
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
