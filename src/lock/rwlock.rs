// SPDX-License-Identifier: LGPL-3.0-or-later OR MPL-2.0
// This file is a part of `unsend`.
//
// `unsend` is free software: you can redistribute it and/or modify it under the
// terms of either:
//
// * GNU Lesser General Public License as published by the Free Software Foundation, either
//   version 3 of the License, or (at your option) any later version.
// * Mozilla Public License as published by the Mozilla Foundation, version 2.
// * The Patron License (https://github.com/notgull/unsend/blob/main/LICENSE-PATRON.md)
//   for sponsors and contributors, who can ignore the copyleft provisions of the above licenses
//   for this project.
//
// `unsend` is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
// PURPOSE. See the GNU Lesser General Public License or the Mozilla Public License for more
// details.
//
// You should have received a copy of the GNU Lesser General Public License and the Mozilla
// Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>.

//! Asynchronous read/write lock.

use crate::{Event, EventListener};

use core::cell::{Cell, Ref, RefCell, RefMut};
use core::ops;
use core::pin::Pin;

/// An asynchronous read/write lock.
pub struct RwLock<T: ?Sized> {
    /// There are no more readers.
    no_readers: Event<()>,

    /// There are no more writers.
    no_writers: Event<()>,

    /// The number of readers.
    readers: Cell<usize>,

    /// The underlying data.
    data: RefCell<T>,
}

/// A guard that unlocks the read end of the lock when dropped.
pub struct RwLockReadGuard<'a, T: ?Sized> {
    /// The event to signal.
    event: &'a Event<()>,

    /// The number of readers.
    readers: &'a Cell<usize>,

    /// The underlying data.
    data: Ref<'a, T>,
}

/// A guard that unlocks the write end of the lock when dropped.
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    /// The event to signal.
    event: &'a Event<()>,

    /// The underlying data.
    data: RefMut<'a, T>,
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> RwLock<T> {
        RwLock::new(Default::default())
    }
}

impl<T> RwLock<T> {
    /// Creates a new asynchronous read/write lock.
    pub fn new(data: T) -> RwLock<T> {
        RwLock {
            no_readers: Event::new(),
            no_writers: Event::new(),
            readers: Cell::new(0),
            data: RefCell::new(data),
        }
    }

    /// Unwraps the underlying data.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Get a mutable reference to the underlying data.
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    /// Try to lock the read end of the lock.
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        self.data.try_borrow().ok().map(|data| {
            self.readers
                .set(self.readers.get().checked_add(1).expect("too many readers"));
            RwLockReadGuard {
                event: &self.no_readers,
                readers: &self.readers,
                data,
            }
        })
    }

    /// Try to lock the write end of the lock.
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        self.data
            .try_borrow_mut()
            .ok()
            .map(|data| RwLockWriteGuard {
                event: &self.no_writers,
                data,
            })
    }

    /// Lock the read end of the lock.
    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        let mut listener = EventListener::new(&self.no_writers);

        {
            let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

            loop {
                if let Some(guard) = self.try_read() {
                    return guard;
                }

                listener.as_mut().await;
            }
        }
    }

    /// Lock the write end of the lock.
    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        let mut listener = EventListener::new(&self.no_readers);

        {
            let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

            loop {
                if let Some(guard) = self.try_write() {
                    return guard;
                }

                listener.as_mut().await;
            }
        }
    }
}

impl<T: ?Sized> ops::Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        let readers = self.readers.get().checked_sub(1).expect("too few readers");
        self.readers.set(readers);

        if readers == 0 {
            self.event.notify(1);
        }
    }
}

impl<T: ?Sized> ops::Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T: ?Sized> ops::DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.event.notify(1);
    }
}
