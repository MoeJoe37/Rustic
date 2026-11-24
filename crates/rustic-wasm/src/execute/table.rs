use super::state::{Transfer, checked_range, constant};
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
    max: usize,
}

impl Tables {
    pub(super) fn new(
        declarations: &Declarations<'_>,
        globals: &[Slot],
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
            let count = table.ty.limits.min as usize;
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
            let max = table.ty.limits.max.unwrap_or(u64::from(u32::MAX)) as usize;
            append(&mut state.tables, Table { entries, max }, offset)?;
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
                    let start =
                        constant(globals, &active.offset, stack_limit, budget)?.i32() as u32;
                    let target = &mut state.tables[active.target as usize].entries;
                    let range = checked_range(
                        target.len(),
                        u64::from(start),
                        count,
                        offset,
                        OUT_OF_BOUNDS,
                    )?;
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
        index: u32,
        offset: usize,
    ) -> Result<Reference, ExecutionError> {
        self.tables[table as usize]
            .entries
            .get(index as usize)
            .copied()
            .ok_or_else(|| error(offset, OUT_OF_BOUNDS))
    }

    pub(super) fn set(
        &mut self,
        table: u32,
        index: u32,
        value: Reference,
        offset: usize,
    ) -> Result<(), ExecutionError> {
        let target = self.tables[table as usize]
            .entries
            .get_mut(index as usize)
            .ok_or_else(|| error(offset, OUT_OF_BOUNDS))?;
        *target = value;
        Ok(())
    }

    pub(super) fn size(&self, table: u32) -> i32 {
        self.tables[table as usize].entries.len() as i32
    }

    pub(super) fn grow(
        &mut self,
        table: u32,
        count: u32,
        value: Reference,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<i32, ExecutionError> {
        let table = &mut self.tables[table as usize];
        let old = table.entries.len();
        let count = count as usize;
        let Some(new) = old.checked_add(count).filter(|&new| new <= table.max) else {
            return Ok(-1);
        };
        if count > self.limit - self.used {
            return Ok(-1);
        }
        let work = if new > table.entries.capacity() {
            new
        } else {
            count
        };
        budget.spend(offset, work)?;
        if table.entries.try_reserve_exact(count).is_err() {
            return Ok(-1);
        }
        table.entries.resize(new, value);
        self.used += count;
        Ok(old as i32)
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
        budget.spend(offset, transfer.size as usize)?;
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
        budget.spend(offset, transfer.size as usize)?;
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
        index: u32,
        count: u32,
        value: Reference,
        budget: &mut Budget<'_>,
        offset: usize,
    ) -> Result<(), ExecutionError> {
        let entries = &mut self.tables[table as usize].entries;
        let range = checked_range(
            entries.len(),
            u64::from(index),
            count as usize,
            offset,
            OUT_OF_BOUNDS,
        )?;
        budget.spend(offset, count as usize)?;
        entries[range].fill(value);
        Ok(())
    }
}

fn element_value(
    items: &ElementItems,
    index: usize,
    globals: &[Slot],
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
