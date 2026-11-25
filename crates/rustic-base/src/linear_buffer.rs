use std::{
    cell::{Ref, RefCell, RefMut},
    rc::Rc,
};

/// A detachable generation of a linear byte store.
#[derive(Clone, Debug)]
pub struct LinearBytes {
    data: Rc<RefCell<Option<Vec<u8>>>>,
    resizable: bool,
}

impl LinearBytes {
    fn new(bytes: Option<Vec<u8>>, resizable: bool) -> Self {
        Self {
            data: Rc::new(RefCell::new(bytes)),
            resizable,
        }
    }

    /// Bytes retained by a generation's metadata, excluding its contents.
    pub const fn storage_bytes() -> usize {
        size_of::<RefCell<Option<Vec<u8>>>>() + 2 * size_of::<usize>()
    }

    /// Whether this generation stays attached when its store grows.
    pub fn is_resizable(&self) -> bool {
        self.resizable
    }

    /// Borrow the attached bytes.
    pub fn read(&self) -> Option<Ref<'_, [u8]>> {
        Ref::filter_map(self.data.borrow(), |bytes| bytes.as_deref()).ok()
    }

    /// Borrow the attached bytes for writing without changing their length.
    pub fn write(&self) -> Option<RefMut<'_, [u8]>> {
        RefMut::filter_map(self.data.borrow_mut(), |bytes| bytes.as_deref_mut()).ok()
    }
}

/// An agent-local byte store with detachable views.
#[derive(Debug)]
pub struct LinearBuffer(RefCell<LinearBytes>);

impl LinearBuffer {
    /// Allocate zeroed bytes, returning `None` when reservation fails.
    pub fn zeroed(length: usize) -> Option<Self> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).ok()?;
        bytes.resize(length, 0);
        Some(Self(RefCell::new(LinearBytes::new(Some(bytes), false))))
    }

    /// Return a handle to the current generation.
    pub fn view(&self) -> LinearBytes {
        self.0.borrow().clone()
    }

    /// Prepare a wrapper for a new generation, then move the bytes into it.
    ///
    /// The callback receives a detached handle until it returns successfully.
    /// It must not enter the store or retain a borrow of the handle.
    pub fn replace_view<T, E>(
        &self,
        resizable: bool,
        prepare: impl FnOnce(LinearBytes) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut current = self.0.borrow_mut();
        let next = LinearBytes::new(None, resizable);
        let result = prepare(next.clone())?;
        *next.data.borrow_mut() = current.data.borrow_mut().take();
        *current = next;
        Ok(result)
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
        let bytes = current.data.borrow();
        let bytes = bytes.as_ref().expect("current generation is attached");
        if new_length > bytes.capacity() {
            new_length
        } else {
            new_length.saturating_sub(bytes.len())
        }
    }

    /// Grow the store, detaching fixed-length views even for zero-length growth.
    /// Rejected sizes and failed reservations leave the store intact.
    pub fn grow_to(&self, new_length: usize) -> bool {
        let Ok(mut current) = self.0.try_borrow_mut() else {
            return false;
        };
        let detach = !current.resizable && Rc::strong_count(&current.data) != 1;
        let Ok(mut data) = current.data.try_borrow_mut() else {
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
        if detach {
            let next = data.take();
            drop(data);
            *current = LinearBytes::new(next, false);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_view_preparation_preserves_bytes_and_growth_mode() {
        let store = LinearBuffer::zeroed(1).unwrap();
        let fixed = store.view();
        store.write(|bytes| bytes[0] = 9);
        let failed = store.replace_view(true, |pending| {
            assert!(pending.read().is_none());
            Err::<(), _>(())
        });
        assert_eq!(failed, Err(()));
        assert_eq!(&*fixed.read().unwrap(), &[9]);
        assert!(store.grow_to(1));
        assert!(fixed.read().is_none());

        let resizable = store.replace_view(true, Ok::<_, ()>).unwrap();
        assert!(store.grow_to(2));
        assert_eq!(&*resizable.read().unwrap(), &[9, 0]);
        let next = store.replace_view(false, Ok::<_, ()>).unwrap();
        assert!(resizable.read().is_none());
        assert_eq!(&*next.read().unwrap(), &[9, 0]);
    }
}
