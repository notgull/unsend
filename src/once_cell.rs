//! A one-time initialization cell.

use core::cell::UnsafeCell;

/// A thread-unsafe one-time-initialization cell.
pub(crate) struct OnceCell<T> {
    value: UnsafeCell<Option<T>>,
}

impl<T> OnceCell<T> {
    /// Create a new, empty `OnceCell`.
    pub(crate) const fn new() -> Self {
        Self {
            value: UnsafeCell::new(None),
        }
    }

    /// Get the value or initialize it.
    pub(crate) fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        unsafe {
            if (*self.value.get()).is_none() {
                *self.value.get() = Some(f());
            }
            (*self.value.get()).as_ref().unwrap()
        }
    }
}

impl<T> From<T> for OnceCell<T> {
    fn from(value: T) -> Self {
        Self {
            value: UnsafeCell::new(Some(value)),
        }
    }
}
