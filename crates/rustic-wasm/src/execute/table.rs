use std::{cell::Cell, ops::Range, rc::Rc};

use super::state::{GlobalSlot, Transfer, checked_range, constant};
use super::*;
use crate::{Declarations, ElementItems, ElementMode, ImportType, Limits, TableType};

const OUT_OF_BOUNDS: Fault = Fault::TableOutOfBounds;

const FUNCREF: ValueType = ValueType::Ref {
    nullable: true,
    heap_type: -16,
};
const EXTERNREF: ValueType = ValueType::Ref {
    nullable: true,
    heap_type: -17,
};

#[derive(Default)]
pub(super) struct Tables {
    tables: Vec<Rc<Table>>,
    imported: usize,
    elements: Vec<Vec<Reference>>,
    pool: Rc<TablePool>,
}

#[derive(Default)]
struct TablePool {
    used: Cell<usize>,
    limit: usize,
}

impl TablePool {
    fn admits(&self, count: usize) -> bool {
        count <= self.limit - self.used.get()
    }

    fn add(&self, count: usize) {
        self.used.set(self.used.get() + count);
    }

    fn remove(&self, count: usize) {
        self.used.set(self.used.get() - count);
    }
}

struct Table {
    entries: RefCell<Vec<Reference>>,
    ty: TableType,
    pool: Rc<TablePool>,
}

impl Drop for Table {
    fn drop(&mut self) {
        self.pool.remove(self.entries.get_mut().len());
    }
}

impl Drop for Tables {
    fn drop(&mut self) {
        for segment in &self.elements {
            self.pool.remove(segment.len());
        }
    }
}

impl Table {
    fn allocate(
        ty: TableType,
        count: usize,
        initial: Reference,
        pool: &Rc<TablePool>,
    ) -> Option<Rc<Self>> {
        if !pool.admits(count) {
            return None;
        }
        let mut entries = Vec::new();
        entries.try_reserve_exact(count).ok()?;
        entries.resize(count, initial);
        pool.add(count);
        Some(Rc::new(Self {
            entries: RefCell::new(entries),
            ty,
            pool: Rc::clone(pool),
        }))
    }

    fn len(&self) -> u64 {
        self.entries.borrow().len() as u64
    }

    fn growth(&self, count: u64) -> Option<(usize, usize, usize)> {
        let count = usize::try_from(count)
            .ok()
            .filter(|&count| self.pool.admits(count))?;
        let entries = self.entries.borrow();
        let limits = self.ty.limits;
        let max = limits
            .max
            .unwrap_or_else(|| limits.address_type.max_table_elements());
        let new = entries
            .len()
            .checked_add(count)
            .filter(|&new| new as u64 <= max)?;
        let work = if new > entries.capacity() { new } else { count };
        Some((count, new, work))
    }

    fn grow<E>(
        &self,
        count: u64,
        value: Reference,
        charge: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<Option<u64>, E> {
        let Some((added, new, work)) = self.growth(count) else {
            return Ok(None);
        };
        charge(work)?;
        if self.growth(count) != Some((added, new, work)) {
            return Ok(None);
        }
        let mut entries = self.entries.borrow_mut();
        let old = entries.len();
        if entries.try_reserve_exact(added).is_err() {
            return Ok(None);
        }
        entries.resize(new, value);
        self.pool.add(added);
        Ok(Some(old as u64))
    }
}

/// One entry in a host-linked nullable reference table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableEntry {
    /// Null in the table's declared reference type.
    Null,
    /// An instance-bound function address.
    Function(FunctionAddress),
    /// An identity supplied by the embedder.
    External(ExternalReference),
}

impl TableEntry {
    fn reference(self, ty: ValueType) -> Option<Reference> {
        match (ty, self) {
            (FUNCREF, Self::Null) => Some(Reference::NullFunction),
            (EXTERNREF, Self::Null) => Some(Reference::NullExtern),
            (FUNCREF, Self::Function(address)) if address.owner().is_some() => {
                Some(Reference::Function(address))
            }
            (EXTERNREF, Self::External(reference)) => Some(Reference::Extern(reference)),
            _ => None,
        }
    }
}

/// A nullable function or external-reference table shared in one host store.
/// The allocator owns entry limits and the host traces its live identities.
#[derive(Clone)]
pub struct ReferenceTable(Rc<Table>);

impl ReferenceTable {
    /// Allocate an independent table. Host entry limits do not change the
    /// declared maximum used during import matching.
    pub fn new(ty: TableType, entry_limit: usize) -> Option<Self> {
        if !ty.limits.fits(ty.limits.address_type.max_table_elements()) {
            return None;
        }
        let initial = TableEntry::Null.reference(ty.element_type)?;
        let count = usize::try_from(ty.limits.min).ok()?;
        let pool = Rc::new(TablePool {
            used: Cell::new(0),
            limit: entry_limit,
        });
        Table::allocate(ty, count, initial, &pool).map(Self)
    }

    /// Heap reservation for a standalone allocation and its growth.
    pub const fn storage_bytes(entry_limit: usize) -> usize {
        entry_limit
            .saturating_mul(size_of::<Reference>())
            .saturating_add(size_of::<Table>() + size_of::<TablePool>() + 4 * size_of::<usize>())
    }

    /// Check the current size, declared maximum, address width and element type.
    pub fn matches(&self, ty: TableType) -> bool {
        let actual = self.0.ty.limits;
        ty.element_type == self.0.ty.element_type
            && actual.address_type == ty.limits.address_type
            && self.length() >= ty.limits.min
            && ty
                .limits
                .max
                .is_none_or(|limit| actual.max.is_some_and(|max| max <= limit))
    }

    /// Current number of entries.
    pub fn length(&self) -> u64 {
        self.0.len()
    }

    /// Read a table entry by its full-width index.
    pub fn get(&self, index: u64) -> Option<TableEntry> {
        Some(
            match *self.0.entries.borrow().get(usize::try_from(index).ok()?)? {
                Reference::NullFunction | Reference::NullExtern => TableEntry::Null,
                Reference::Function(address) => TableEntry::Function(address),
                Reference::Extern(reference) => TableEntry::External(reference),
            },
        )
    }

    /// Replace an entry if its type and index match.
    pub fn set(&self, index: u64, value: TableEntry) -> bool {
        let (Ok(index), Some(value)) = (
            usize::try_from(index),
            value.reference(self.0.ty.element_type),
        ) else {
            return false;
        };
        let mut entries = self.0.entries.borrow_mut();
        let Some(slot) = entries.get_mut(index) else {
            return false;
        };
        *slot = value;
        true
    }

    /// Grow against the allocating owner's limits. Charge work before mutation
    /// and return the previous size upon success.
    pub fn grow<E>(
        &self,
        count: u64,
        value: TableEntry,
        charge: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<Option<u64>, E> {
        let Some(value) = value.reference(self.0.ty.element_type) else {
            return Ok(None);
        };
        self.0.grow(count, value, charge)
    }

    /// Visit live host identities. The visitor must not reenter the table.
    pub fn visit_reference_roots(&self, visitor: &mut dyn FnMut(ReferenceRoot)) {
        trace_entries(&self.0.entries.borrow(), visitor);
    }
}

fn external(value: Option<ExternalReference>) -> Reference {
    value.map_or(Reference::NullExtern, Reference::Extern)
}

fn trace_entries(entries: &[Reference], visitor: &mut dyn FnMut(ReferenceRoot)) {
    for reference in entries.iter().filter_map(|entry| entry.host_root()) {
        visitor(reference);
    }
}

impl Instance<'_, '_> {
    /// Read the declared type of a module-defined table.
    pub fn table_type(&self, table: u32) -> Option<TableType> {
        Some(self.state.borrow().tables.tables.get(table as usize)?.ty)
    }

    /// Read the current length of a table in the instance's index space.
    pub fn table_len(&self, table: u32) -> Option<u64> {
        Some(self.state.borrow().tables.tables.get(table as usize)?.len())
    }

    /// Retain a handle in the same host store. The embedder keeps the
    /// allocation owner alive and traces the table's host identities.
    pub fn shared_table(&self, index: u32) -> Option<ReferenceTable> {
        let state = self.state.borrow();
        let table = state.tables.tables.get(index as usize)?;
        match table.ty.element_type {
            FUNCREF if state.function_owner.is_some() => {}
            EXTERNREF => {}
            _ => return None,
        }
        state.has_host_references.set(true);
        Some(ReferenceTable(Rc::clone(table)))
    }

    /// Retain a handle specifically to a nullable external table.
    pub fn external_table(&self, index: u32) -> Option<ReferenceTable> {
        self.state.borrow().tables.reference_table(index, -17)?;
        self.shared_table(index)
    }

    /// Read an external table entry. The inner `None` represents null.
    pub fn external_table_get(&self, table: u32, index: u64) -> Option<Option<ExternalReference>> {
        match self.external_table(table)?.get(index)? {
            TableEntry::Null => Some(None),
            TableEntry::External(reference) => Some(Some(reference)),
            TableEntry::Function(_) => unreachable!("external table element"),
        }
    }

    /// Write a host identity or null to a nullable external table.
    pub fn external_table_set(
        &self,
        table: u32,
        index: u64,
        reference: Option<ExternalReference>,
    ) -> bool {
        self.external_table(table).is_some_and(|table| {
            table.set(
                index,
                reference.map_or(TableEntry::Null, TableEntry::External),
            )
        })
    }

    /// Visit host identities in persistent guest state. Imported tables are
    /// traced through their owners. The visitor must not reenter the instance.
    pub fn visit_reference_roots(&self, visitor: &mut dyn FnMut(ReferenceRoot)) {
        let state = self.state.borrow();
        if !state.has_host_references.get() {
            return;
        }
        state.tables.visit_reference_roots(visitor);
        for reference in state
            .globals
            .iter()
            .filter_map(|slot| slot.value().host_root())
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
                .len(),
        )
    }

    /// Read a local function index. The inner `None` represents null.
    /// Use [`Self::shared_table`] for entries owned by another instance.
    pub fn function_table_get(&self, table: u32, index: u64) -> Option<Option<u32>> {
        let state = self.state.borrow();
        let table = state.tables.reference_table(table, -16)?;
        let index = usize::try_from(index).ok()?;
        match *table.entries.borrow().get(index)? {
            Reference::NullFunction => Some(None),
            Reference::Function(address) if address.owner() == state.function_owner => {
                Some(Some(address.index()))
            }
            Reference::Function(_) => None,
            _ => unreachable!("validated function table entry"),
        }
    }

    /// Write a function by its index in this instance, or write null. Invalid
    /// indices or table types leave the table unchanged.
    pub fn function_table_set(&self, table: u32, index: u64, function: Option<u32>) -> bool {
        if function.is_some_and(|index| self.function_type_at(index).is_none()) {
            return false;
        }
        let mut state = self.state.borrow_mut();
        let value = function.map_or(Reference::NullFunction, |index| {
            FunctionAddress::reference(state.function_owner, index)
        });
        state.tables.reference_table(table, -16).is_some()
            && state.tables.set(table, index, value, 0).is_ok()
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
        state.tables.tables[table as usize]
            .growth(count)
            .map(|(_, _, work)| work)
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
        let owner = self.state.borrow().function_owner;
        let value = function.map_or(Reference::NullFunction, |index| {
            FunctionAddress::reference(owner, index)
        });
        self.grow_host_table(table, count, -16, value, cancellation)
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
        self.grow_host_table(table, count, -17, external(reference), cancellation)
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
        if previous.is_some() && count != 0 && value.host_root().is_some() {
            state.has_host_references.set(true);
        }
        Ok(previous)
    }
}

impl Tables {
    pub(super) fn new(
        declarations: &Declarations<'_>,
        globals: &[GlobalSlot],
        mut imports: &[ReferenceTable],
        function_owner: Option<NonZeroU32>,
        limits: InstanceLimits,
        budget: &mut Budget<'_>,
    ) -> Result<(Self, ActiveElements), ExecutionError> {
        let mut state = Self {
            pool: Rc::new(TablePool {
                used: Cell::new(0),
                limit: limits.table_elements,
            }),
            tables: Vec::new(),
            imported: 0,
            elements: Vec::new(),
        };
        for entry in &declarations.imports {
            let ImportType::Table(ty) = entry.value.ty else {
                continue;
            };
            budget.spend(entry.offset, 1)?;
            let Some((table, rest)) = imports.split_first() else {
                return Err(error(entry.offset, Fault::MissingImport));
            };
            if !table.matches(ty) || (ty.element_type == FUNCREF && function_owner.is_none()) {
                return Err(error(entry.offset, Fault::ImportTypeMismatch));
            }
            append(&mut state.tables, Rc::clone(&table.0), entry.offset)?;
            imports = rest;
        }
        if !imports.is_empty() {
            return Err(error(0, Fault::ImportTypeMismatch));
        }
        state.imported = state.tables.len();
        let stack_limit = limits.execution.values;
        for entry in &declarations.tables {
            let offset = entry.offset;
            let table = &entry.value;
            let count = usize::try_from(table.ty.limits.min)
                .map_err(|_| error(offset, Fault::LimitExceeded))?;
            budget.spend(offset, 1)?;
            budget.spend(offset, count)?;
            let initial = match &table.initializer {
                Some(expr) => constant(globals, function_owner, expr, stack_limit, budget)?,
                None => Slot::zero(table.ty.element_type),
            };
            let table = Table::allocate(table.ty, count, initial.reference(), &state.pool)
                .ok_or_else(|| error(offset, Fault::LimitExceeded))?;
            append(&mut state.tables, table, offset)?;
        }

        let mut active = ActiveElements::default();
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
                    if !state.pool.admits(count) {
                        return Err(error(offset, Fault::LimitExceeded));
                    }
                    budget.spend(offset, count)?;
                    passive
                        .try_reserve_exact(count)
                        .map_err(|_| error(offset, Fault::LimitExceeded))?;
                    for index in 0..count {
                        passive.push(element_value(
                            &element.items,
                            index,
                            globals,
                            function_owner,
                            stack_limit,
                            offset,
                            budget,
                        )?);
                    }
                }
                ElementMode::Active(segment) => {
                    let start = constant(
                        globals,
                        function_owner,
                        &segment.offset,
                        stack_limit,
                        budget,
                    )?
                    .address();
                    let target = &state.tables[segment.target as usize];
                    let range = match checked_range(
                        target.len() as usize,
                        start,
                        count as u64,
                        offset,
                        OUT_OF_BOUNDS,
                    ) {
                        Ok(range) => range,
                        Err(trap) => {
                            active.trap = Some(trap);
                            break;
                        }
                    };
                    budget.spend(offset, count)?;
                    let mut values = Vec::new();
                    values
                        .try_reserve_exact(count)
                        .map_err(|_| error(offset, Fault::LimitExceeded))?;
                    for index in 0..count {
                        values.push(element_value(
                            &element.items,
                            index,
                            globals,
                            function_owner,
                            stack_limit,
                            offset,
                            budget,
                        )?);
                    }
                    append(&mut active.writes, (segment.target, range, values), offset)?;
                }
                ElementMode::Declarative => {}
            }
            let len = passive.len();
            append(&mut state.elements, passive, offset)?;
            state.pool.add(len);
        }
        Ok((state, active))
    }

    pub(super) fn get(
        &self,
        table: u32,
        index: u64,
        offset: usize,
    ) -> Result<Reference, ExecutionError> {
        let entries = self.tables[table as usize].entries.borrow();
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
        let mut entries = self.tables[table as usize].entries.borrow_mut();
        let range = checked_range(entries.len(), index, 1, offset, OUT_OF_BOUNDS)?;
        entries[range.start] = value;
        Ok(())
    }

    pub(super) fn size(&self, table: u32) -> Value {
        let table = &self.tables[table as usize];
        table.ty.limits.address_type.index_value(table.len())
    }

    fn reference_table(&self, index: u32, heap_type: i64) -> Option<&Rc<Table>> {
        let table = self.tables.get(index as usize)?;
        (table.ty.element_type
            == ValueType::Ref {
                nullable: true,
                heap_type,
            })
        .then_some(table)
    }

    fn visit_reference_roots(&self, visitor: &mut dyn FnMut(ReferenceRoot)) {
        for table in &self.tables[self.imported..] {
            if matches!(table.ty.element_type, ValueType::Ref { .. }) {
                trace_entries(&table.entries.borrow(), visitor);
            }
        }
        for segment in &self.elements {
            trace_entries(segment, visitor);
        }
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
        self.tables[table as usize].grow(count, value, |work| budget.spend(offset, work))
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
        let mut target = self.tables[table as usize].entries.borrow_mut();
        let (src, dst) = transfer.ranges(source.len(), target.len(), offset, OUT_OF_BOUNDS)?;
        budget.spend(offset, dst.len())?;
        target[dst].copy_from_slice(&source[src]);
        Ok(())
    }

    pub(super) fn drop_element(&mut self, element: u32) {
        let previous = std::mem::take(&mut self.elements[element as usize]);
        self.pool.remove(previous.len());
    }

    pub(super) fn copy(
        &mut self,
        destination: u32,
        source: u32,
        transfer: Transfer,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<(), ExecutionError> {
        let target = &self.tables[destination as usize];
        let origin = &self.tables[source as usize];
        let (src, dst) = transfer.ranges(
            origin.len() as usize,
            target.len() as usize,
            offset,
            OUT_OF_BOUNDS,
        )?;
        budget.spend(offset, dst.len())?;
        if Rc::ptr_eq(target, origin) {
            target.entries.borrow_mut().copy_within(src, dst.start);
        } else {
            target.entries.borrow_mut()[dst].copy_from_slice(&origin.entries.borrow()[src]);
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
        let mut entries = self.tables[table as usize].entries.borrow_mut();
        let range = checked_range(entries.len(), index, count, offset, OUT_OF_BOUNDS)?;
        budget.spend(offset, range.len())?;
        entries[range].fill(value);
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct ActiveElements {
    writes: Vec<(u32, Range<usize>, Vec<Reference>)>,
    pub(super) trap: Option<ExecutionError>,
}

impl ActiveElements {
    pub(super) fn apply(self, tables: &Tables) -> Result<(), ExecutionError> {
        for (target, range, values) in self.writes {
            tables.tables[target as usize].entries.borrow_mut()[range].copy_from_slice(&values);
        }
        self.trap.map_or(Ok(()), Err)
    }
}

fn element_value(
    items: &ElementItems,
    index: usize,
    globals: &[GlobalSlot],
    function_owner: Option<NonZeroU32>,
    stack_limit: usize,
    offset: usize,
    budget: &mut Budget<'_>,
) -> Result<Reference, ExecutionError> {
    budget.spend(offset, 0)?;
    Ok(match items {
        ElementItems::Functions(indexes) => {
            FunctionAddress::reference(function_owner, indexes[index])
        }
        ElementItems::Expressions(expr) => {
            constant(globals, function_owner, &expr[index], stack_limit, budget)?.reference()
        }
    })
}
