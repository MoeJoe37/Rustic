use std::{cell::Cell, rc::Rc};

use rustic_base::{FixedBytes, LinearBuffer};

use super::*;
use crate::{AddressType, Limits};

/// Number of bytes in one WebAssembly page.
pub const MEMORY_PAGE_BYTES: usize = 65_536;
/// Addressable pages in a 32-bit WebAssembly memory.
pub const MEMORY32_MAX_PAGES: u32 = 65_536;

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
    maximum: Option<u32>,
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

/// An unshared 32-bit memory that can be linked by several instances in one agent.
#[derive(Clone)]
pub struct LinearMemory(Rc<Memory>);

impl LinearMemory {
    /// Allocate a standalone memory against its own aggregate byte limit.
    pub fn new(initial: u32, maximum: Option<u32>, byte_limit: usize) -> Option<Self> {
        Self::allocate(initial, maximum, &MemoryPool::new(byte_limit))
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

    fn allocate(initial: u32, maximum: Option<u32>, pool: &MemoryPool) -> Option<Self> {
        let maximum_pages = maximum.unwrap_or(MEMORY32_MAX_PAGES);
        if initial > maximum_pages || maximum_pages > MEMORY32_MAX_PAGES {
            return None;
        }
        let length = (initial as usize).checked_mul(MEMORY_PAGE_BYTES)?;
        if !pool.admits(length) {
            return None;
        }
        let bytes = LinearBuffer::zeroed(length)?;
        pool.0.used.set(pool.0.used.get() + length);
        Some(Self(Rc::new(Memory {
            bytes,
            maximum,
            pool: pool.clone(),
        })))
    }

    pub(super) fn local(
        limits: Limits,
        pool: &MemoryPool,
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ExecutionError> {
        if limits.address_type != AddressType::I32 {
            return Err(error(offset, Fault::UnsupportedModule));
        }
        let initial = u32::try_from(limits.min).map_err(|_| error(offset, Fault::LimitExceeded))?;
        let length = (initial as usize)
            .checked_mul(MEMORY_PAGE_BYTES)
            .filter(|&bytes| pool.admits(bytes))
            .ok_or_else(|| error(offset, Fault::LimitExceeded))?;
        budget.spend(offset, 1 + length)?;
        // The validator bounds the declared maximum to the 32-bit page limit.
        let maximum = limits.max.map(|pages| pages as u32);
        Self::allocate(initial, maximum, pool).ok_or_else(|| error(offset, Fault::LimitExceeded))
    }

    pub(super) fn matches(&self, limits: Limits) -> bool {
        limits.address_type == AddressType::I32
            && u64::from(self.pages()) >= limits.min
            && limits.max.is_none_or(|required| {
                self.0
                    .maximum
                    .is_some_and(|actual| u64::from(actual) <= required)
            })
    }

    pub(super) fn aliases(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// Current number of allocated WebAssembly pages.
    pub fn pages(&self) -> u32 {
        (self.len() / MEMORY_PAGE_BYTES) as u32
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
        pages: u32,
        charge: impl FnOnce(usize) -> Result<(), E>,
    ) -> Result<Option<u32>, E> {
        let previous = self.pages();
        let old_length = self.len();
        let Some(length) = previous
            .checked_add(pages)
            .filter(|&count| count <= self.0.maximum.unwrap_or(MEMORY32_MAX_PAGES))
            .and_then(|count| (count as usize).checked_mul(MEMORY_PAGE_BYTES))
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
