//! Asynchronous mutex.

use crate::notify::{Notify, Listener};

use alloc::rc::Rc;

use core::cell::{Cell, RefCell, RefMut};
use core::future::Future;
use core::ops::{Deref, DerefMut};

use pin_project_lite::pin_project;

/// An asynchronous mutex.
pub struct Mutex<T: ?Sized> {
    /// Waits for the mutex to be unlocked.
    unlock: Notify<()>,

    /// The value protected by the mutex.
    value: RefCell<T>,
}

impl<T> Mutex<T> {
    /// Create a new `Mutex`.
    pub const fn new(value: T) -> Self {
        Self {
            unlock: Notify::new(),
            value: RefCell::new(value),
        }
    }

    /// Unwrap the mutex, returning the underlying value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Tell whether the mutex is locked.
    pub fn is_locked(&self) -> bool {
        self.value.try_borrow().is_ok()
    }

    /// Get a mutable reference to the underlying value.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    /// Try to lock the mutex.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.value.try_borrow_mut().ok().map(|guard| MutexGuard {
            mutex: self,
            guard,
        })
    }
}

/// A lock guard for a `Mutex`.
pub struct MutexGuard<'a, T: ?Sized> {
    /// The mutex.
    mutex: &'a Mutex<T>,

    /// The guard on the `RefCell`.
    guard: RefMut<'a, T>,
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}
