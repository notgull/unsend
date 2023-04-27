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

//! Asynchronous semaphore.

use crate::{Event, EventListener};

use core::cell::Cell;
use core::pin::Pin;

#[cfg(feature = "alloc")]
use alloc::rc::Rc;

/// An asynchronous semaphore.
pub struct Semaphore {
    /// The event for waiting on the semaphore.
    event: Event<()>,

    /// The number of permits.
    permits: Cell<usize>,
}

/// A guard that releases the permit when dropped.
pub struct SemaphoreGuard<'a> {
    /// The origin semaphore.
    semaphore: &'a Semaphore,
}

/// A guard that releases the permit when dropped.
#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
pub struct SemaphoreGuardRc {
    /// The origin semaphore.
    semaphore: Rc<Semaphore>,
}

impl Semaphore {
    /// Creates a new asynchronous semaphore.
    pub fn new(permits: usize) -> Semaphore {
        Semaphore {
            event: Event::new(),
            permits: Cell::new(permits),
        }
    }

    /// Try to acquire a permit.
    pub fn try_acquire(&self) -> Option<SemaphoreGuard<'_>> {
        let permits = self.permits.get();
        if permits > 0 {
            self.permits.set(permits - 1);
            Some(SemaphoreGuard { semaphore: self })
        } else {
            None
        }
    }

    /// Try to acquire a permit through an `Rc`.
    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    pub fn try_acquire_rc(self: Rc<Self>) -> Option<SemaphoreGuardRc> {
        let permits = self.permits.get();
        if permits > 0 {
            self.permits.set(permits - 1);
            Some(SemaphoreGuardRc { semaphore: self })
        } else {
            None
        }
    }

    /// Acquire a permit.
    pub async fn acquire(&self) -> SemaphoreGuard<'_> {
        let mut listener = EventListener::new(&self.event);

        {
            let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

            loop {
                if let Some(guard) = self.try_acquire() {
                    return guard;
                }

                listener.as_mut().await;
            }
        }
    }

    /// Acquire a permit through an `Rc`.
    #[cfg(feature = "alloc")]
    #[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
    pub async fn acquire_rc(self: Rc<Self>) -> SemaphoreGuardRc {
        let mut listener = EventListener::new(&self.event);

        {
            let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

            loop {
                if let Some(guard) = self.clone().try_acquire_rc() {
                    return guard;
                }

                listener.as_mut().await;
            }
        }
    }
}

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        self.semaphore.permits.set(self.semaphore.permits.get() + 1);
        self.semaphore.event.notify(1);
    }
}

#[cfg(feature = "alloc")]
impl Drop for SemaphoreGuardRc {
    fn drop(&mut self) {
        self.semaphore.permits.set(self.semaphore.permits.get() + 1);
        self.semaphore.event.notify(1);
    }
}
