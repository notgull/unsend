//! Asynchronous mutex.

use crate::notify::{LazyNotify, Listener};

use core::cell::{RefCell, RefMut};
use core::future::Future;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

/// An asynchronous mutex.
pub struct Mutex<T: ?Sized> {
    /// Waits for the mutex to be unlocked.
    unlock: LazyNotify<()>,

    /// The value protected by the mutex.
    value: RefCell<T>,
}

impl<T> Mutex<T> {
    /// Create a new `Mutex`.
    pub const fn new(value: T) -> Self {
        Self {
            unlock: LazyNotify::new(),
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
        self.value
            .try_borrow_mut()
            .ok()
            .map(|guard| MutexGuard { mutex: self, guard })
    }

    /// Lock the mutex.
    pub fn lock(&self) -> Lock<'_, T> {
        Lock {
            mutex: self,
            listener: None,
        }
    }
}

pin_project! {
    /// The future for locking a `Mutex`.
    pub struct Lock<'a, T: ?Sized> {
        // The mutex.
        mutex: &'a Mutex<T>,

        // The listener for the mutex.
        #[pin]
        listener: Option<Listener<'a, ()>>,
    }
}

impl<'a, T: ?Sized> Future for Lock<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            // Try to lock the mutex.
            if let Some(guard) = this.mutex.try_lock() {
                return Poll::Ready(guard);
            }

            // Create the listener if it doesn't exist and poll it.
            loop {
                if this.listener.as_ref().is_none() {
                    this.listener.as_mut().set(Some(this.mutex.unlock.listen()));
                }

                ready!(this.listener.as_mut().as_pin_mut().unwrap().poll(cx));
            }
        }
    }
}

/// A lock guard for a `Mutex`.
pub struct MutexGuard<'a, T: ?Sized> {
    /// Back-reference to the mutex.
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

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock.notify(1, || ());
    }
}
