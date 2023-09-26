// SPDX-License-Identifier: LGPL-3.0-or-later OR MPL-2.0
// This file is a part of `unsend`.
//
// `unsend` is free software: you can redistribute it and/or modify it under the
// terms of either:
//
// * GNU Lesser General Public License as published by the Free Software Foundation, either
//   version 3 of the License, or (at your option) any later version.
// * Mozilla Public License as published by the Mozilla Foundation, version 2.
//
// `unsend` is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
// PURPOSE. See the GNU Lesser General Public License or the Mozilla Public License for more
// details.
//
// You should have received a copy of the GNU Lesser General Public License and the Mozilla
// Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>.

//! Wait for a set number of tasks to reach this point.

use crate::{Event, EventListener};

use core::cell::Cell;
use core::pin::Pin;

/// A barrier that can be used to synchronize a set of tasks.
pub struct Barrier {
    /// Number of tasks to wait for.
    n: usize,

    /// Number of tasks that have reached the barrier.
    count: Cell<usize>,

    /// The generation of the barrier.
    generation: Cell<u64>,

    /// The event for waiting on the barrier.
    event: Event<()>,
}

impl Barrier {
    /// Create a new barrier that waits for this number of tasks.
    pub fn new(n: usize) -> Barrier {
        Barrier {
            n,
            count: Cell::new(0),
            generation: Cell::new(0),
            event: Event::new(),
        }
    }

    /// Wait for the barrier.
    pub async fn wait(&self) -> BarrierWaitResult {
        let local_gen = self.generation.get();
        self.count.set(self.count.get() + 1);
        let mut listener = EventListener::new(&self.event);
        let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

        if self.count.get() < self.n {
            // Wait for the count.
            while local_gen == self.generation.get() && self.count.get() < self.n {
                listener.as_mut().await;
            }

            BarrierWaitResult { is_leader: false }
        } else {
            self.count.set(0);
            self.generation.set(local_gen + 1);
            self.event.notify(core::usize::MAX);

            BarrierWaitResult { is_leader: true }
        }
    }
}

/// The result of waiting on the barrier.
#[derive(Debug, Clone)]
pub struct BarrierWaitResult {
    /// Is this task the leader?
    is_leader: bool,
}

impl BarrierWaitResult {
    /// Is this task the leader?
    pub fn is_leader(&self) -> bool {
        self.is_leader
    }
}
