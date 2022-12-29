//! Asynchronous read-write lock.

use super::{Mutex, MutexGuard};
use crate::notify::{LazyNotify, Listener};

use core::cell::{Cell, UnsafeCell};
use core::future::Future;
use core::num::NonZeroUsize;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

/// An asynchronous read-write lock.
pub struct RwLock<T: ?Sized> {
    /// A mutex to protect the writer status.
    writer: Mutex<()>,

    /// Event to notify when there are no more readers.
    no_readers: LazyNotify<()>,

    /// Event to notify when there are no more writers.
    no_writers: LazyNotify<()>,

    /// Current state of the lock.
    state: Cell<LockState>,

    /// The value protected by the lock.
    value: UnsafeCell<T>,
}

enum LockState {
    /// The lock is unlocked.
    Unlocked,

    /// The lock is locked with one or more readers.
    Readers(NonZeroUsize),

    /// The lock is locked with one writer.
    Writer,
}

impl<T> RwLock<T> {
    /// Create a new `RwLock`.
    pub const fn new(value: T) -> Self {
        Self {
            writer: Mutex::new(()),
            no_readers: LazyNotify::new(),
            no_writers: LazyNotify::new(),
            state: Cell::new(LockState::Unlocked),
            value: UnsafeCell::new(value),
        }
    }

    /// Unwrap the lock, returning the underlying value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Get a mutable reference to the underlying value.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}
