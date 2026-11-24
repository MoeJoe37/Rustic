use std::{
    cell::{Ref, RefCell, RefMut},
    rc::Rc,
};

/// One fixed-length view of a linear byte store. Growth detaches existing views.
#[derive(Clone, Debug)]
pub struct FixedBytes(Rc<RefCell<Option<Vec<u8>>>>);

impl FixedBytes {
    /// Bytes retained by a generation's metadata, excluding its contents.
    pub const fn storage_bytes() -> usize {
        size_of::<RefCell<Option<Vec<u8>>>>() + 2 * size_of::<usize>()
    }

    /// Borrow the attached bytes.
    pub fn read(&self) -> Option<Ref<'_, [u8]>> {
        Ref::filter_map(self.0.borrow(), |bytes| bytes.as_deref()).ok()
    }

    /// Borrow the attached bytes for writing without changing their length.
    pub fn write(&self) -> Option<RefMut<'_, [u8]>> {
        RefMut::filter_map(self.0.borrow_mut(), |bytes| bytes.as_deref_mut()).ok()
    }
}

/// An agent-local byte store with detachable fixed-length views.
#[derive(Debug)]
pub struct LinearBuffer(RefCell<FixedBytes>);

impl LinearBuffer {
    /// Allocate zeroed bytes, returning `None` when reservation fails.
    pub fn zeroed(length: usize) -> Option<Self> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).ok()?;
        bytes.resize(length, 0);
        Some(Self(RefCell::new(FixedBytes(Rc::new(RefCell::new(Some(
            bytes,
        )))))))
    }

    /// Return a handle to the current fixed-length generation.
    pub fn fixed(&self) -> FixedBytes {
        self.0.borrow().clone()
    }

    /// Read the current length in bytes.
    pub fn len(&self) -> usize {
        self.read(<[u8]>::len)
    }

    /// Whether the store has no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read bytes for a synchronous operation.
    pub fn read<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        read(
            &self
                .0
                .borrow()
                .read()
                .expect("current generation is attached"),
        )
    }

    /// Write bytes for a synchronous operation.
    pub fn write<T>(&self, write: impl FnOnce(&mut [u8]) -> T) -> T {
        write(
            &mut self
                .0
                .borrow()
                .write()
                .expect("current generation is attached"),
        )
    }

    /// Work required to zero and, when necessary, relocate the current store.
    pub fn growth_work(&self, new_length: usize) -> usize {
        let current = self.0.borrow();
        let bytes = current.0.borrow();
        let bytes = bytes.as_ref().expect("current generation is attached");
        if new_length > bytes.capacity() {
            new_length
        } else {
            new_length.saturating_sub(bytes.len())
        }
    }

    /// Grow the store and detach its old views, including a zero-length grow.
    /// Rejected sizes and failed reservations leave the store intact.
    pub fn grow_to(&self, new_length: usize) -> bool {
        let Ok(mut current) = self.0.try_borrow_mut() else {
            return false;
        };
        let has_views = Rc::strong_count(&current.0) != 1;
        let Ok(mut data) = current.0.try_borrow_mut() else {
            return false;
        };
        let bytes = data.as_mut().expect("current generation is attached");
        let Some(additional) = new_length.checked_sub(bytes.len()) else {
            return false;
        };
        if bytes.try_reserve_exact(additional).is_err() {
            return false;
        }
        bytes.resize(new_length, 0);
        if has_views {
            let next = data.take();
            drop(data);
            *current = FixedBytes(Rc::new(RefCell::new(next)));
        }
        true
    }
}
