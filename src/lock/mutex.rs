//! An asynchronous mutex.

use crate::{Event, EventListener};

use core::cell::{RefCell, RefMut};
use core::ops;
use core::pin::Pin;

/// An asynchronous mutex.
pub struct Mutex<T: ?Sized> {
    /// The event for waiting on the mutex.
    unlocked: Event<()>,

    /// The underlying data.
    data: RefCell<T>,
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Mutex<T> {
        Mutex::new(Default::default())
    }
}

/// A guard that unlocks the mutex when dropped.
pub struct MutexGuard<'a, T: ?Sized> {
    /// The event to signal.
    event: &'a Event<()>,

    /// The underlying data.
    data: RefMut<'a, T>,
}

impl<T> Mutex<T> {
    /// Creates a new asynchronous mutex.
    pub fn new(data: T) -> Mutex<T> {
        Mutex {
            unlocked: Event::new(),
            data: RefCell::new(data),
        }
    }

    /// Unwraps the underlying data.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Get a mutable reference to the underlying data.
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    /// Try to lock the mutex.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.data.try_borrow_mut().ok().map(|data| MutexGuard {
            event: &self.unlocked,
            data,
        })
    }

    /// Lock the mutex.
    pub async fn lock(&self) -> MutexGuard<'_, T> {
        // TODO: Use a fairer locking algorithm.
        let mut listener = EventListener::new(&self.unlocked);

        {
            let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

            loop {
                // Try to lock the mutex.
                if let Some(lock) = self.try_lock() {
                    return lock;
                }

                // Wait for the mutex to be unlocked.
                listener.as_mut().await;
            }
        }
    }
}

impl<T: ?Sized> ops::Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T: ?Sized> ops::DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.event.notify(1);
    }
}
