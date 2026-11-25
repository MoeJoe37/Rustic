use std::{cell::Cell, rc::Rc};

use rustic_base::{FixedBytes, LinearBuffer};

use super::*;
use crate::{AddressType, Limits};

/// Number of bytes in one WebAssembly page.
pub const MEMORY_PAGE_BYTES: usize = 65_536;
/// Addressable pages in a 32-bit WebAssembly memory.
pub const MEMORY32_MAX_PAGES: u32 = AddressType::I32.max_memory_pages() as u32;

struct Pool {
    used: Cell<usize>,
    limit: usize,
}

/// Aggregate budget for memories allocated by one instance.
#[derive(Clone)]
pub(super) struct MemoryPool(Rc<Pool>);

impl MemoryPool {
    pub(super) fn new(limit: usize) -> Self {
        Self(Rc::new(Pool {
            used: Cell::new(0),
            limit,
        }))
    }

    fn admits(&self, bytes: usize) -> bool {
        bytes <= self.0.limit - self.0.used.get()
    }
}

struct Memory {
    bytes: LinearBuffer,
    address: AddressType,
    maximum: Option<u64>,
    pool: MemoryPool,
}

impl Drop for Memory {
    fn drop(&mut self) {
        self.pool
            .0
            .used
            .set(self.pool.0.used.get() - self.bytes.len());
    }
}

/// An unshared memory that can be linked by several instances in one agent.
#[derive(Clone)]
pub struct LinearMemory(Rc<Memory>);

impl LinearMemory {
    /// Allocate a standalone 32-bit memory against its own aggregate byte limit.
    pub fn new(initial: u32, maximum: Option<u32>, byte_limit: usize) -> Option<Self> {
        Self::with_limits(
            Limits {
                address_type: AddressType::I32,
                min: initial.into(),
                max: maximum.map(u64::from),
            },
            byte_limit,
        )
    }

    /// Allocate a standalone memory with a declared address width and page limits.
    pub fn with_limits(limits: Limits, byte_limit: usize) -> Option<Self> {
        let pool = MemoryPool::new(byte_limit);
        let length = Self::initial_length(limits, &pool)?;
        Self::allocate(limits, length, &pool)
    }

    /// Storage charged for the backing store and its allocation records.
    pub fn storage_bytes(byte_limit: usize) -> usize {
        byte_limit.saturating_add(
            size_of::<Memory>()
                + size_of::<Pool>()
                + 4 * size_of::<usize>()
                + FixedBytes::storage_bytes(),
        )
    }

    fn initial_length(limits: Limits, pool: &MemoryPool) -> Option<usize> {
        if !limits.fits(limits.address_type.max_memory_pages()) {
            return None;
        }
        byte_length(limits.min).filter(|&length| pool.admits(length))
    }

    fn allocate(limits: Limits, length: usize, pool: &MemoryPool) -> Option<Self> {
        let bytes = LinearBuffer::zeroed(length)?;
        pool.0.used.set(pool.0.used.get() + length);
        Some(Self(Rc::new(Memory {
            bytes,
            address: limits.address_type,
            maximum: limits.max,
            pool: pool.clone(),
        })))
    }

    pub(super) fn local(
        limits: Limits,
        pool: &MemoryPool,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ExecutionError> {
        let length = Self::initial_length(limits, pool)
            .ok_or_else(|| error(offset, Fault::LimitExceeded))?;
        budget.spend(offset, length + 1)?;
        Self::allocate(limits, length, pool).ok_or_else(|| error(offset, Fault::LimitExceeded))
    }

    pub(super) fn matches(&self, limits: Limits) -> bool {
        limits.address_type == self.address_type()
            && self.pages() >= limits.min
            && limits
                .max
                .is_none_or(|required| self.0.maximum.is_some_and(|actual| actual <= required))
    }

    pub(super) fn aliases(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// Address width of this memory, fixed for the lifetime of its allocation.
    pub fn address_type(&self) -> AddressType {
        self.0.address
    }

    /// Current number of allocated WebAssembly pages.
    pub fn pages(&self) -> u64 {
        (self.len() / MEMORY_PAGE_BYTES) as u64
    }

    pub(super) fn page_result(&self, pages: Option<u64>) -> Value {
        match self.0.address {
            AddressType::I32 => Value::I32(pages.map_or(-1, |pages| pages as i32)),
            AddressType::I64 => Value::I64(pages.map_or(-1, |pages| pages as i64)),
        }
    }

    /// A fixed-length, live byte view of the current generation.
    pub fn buffer(&self) -> FixedBytes {
        self.0.bytes.fixed()
    }

    pub(super) fn len(&self) -> usize {
        self.0.bytes.len()
    }

    pub(super) fn read<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        self.0.bytes.read(read)
    }

    pub(super) fn write<T>(&self, write: impl FnOnce(&mut [u8]) -> T) -> T {
        self.0.bytes.write(write)
    }

    /// Grow by the specified page count, returning the previous size on success.
    /// Validation and the host's work charge precede mutation.
    pub fn grow<E>(
        &self,
        pages: u64,
        charge: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<Option<u64>, E> {
        let previous = self.pages();
        let old_length = self.len();
        let Some(length) = previous
            .checked_add(pages)
            .filter(|&count| count <= self.0.maximum.unwrap_or(self.0.address.max_memory_pages()))
            .and_then(byte_length)
            .filter(|&length| self.0.pool.admits(length - old_length))
        else {
            return Ok(None);
        };
        charge(self.0.bytes.growth_work(length))?;
        if self.len() != old_length || !self.0.pool.admits(length - old_length) {
            return Ok(None);
        }
        if !self.0.bytes.grow_to(length) {
            return Ok(None);
        }
        let used = &self.0.pool.0.used;
        used.set(used.get() + (length - old_length));
        Ok(Some(previous))
    }
}

fn byte_length(pages: u64) -> Option<usize> {
    usize::try_from(pages).ok()?.checked_mul(MEMORY_PAGE_BYTES)
}
