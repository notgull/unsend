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

//! An asynchronous executor.

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use core::cell::{Cell, RefCell};
use core::future::Future;
use core::marker::PhantomData;
use core::mem::{forget, ManuallyDrop};
use core::num::NonZeroUsize;
use core::task::{Poll, Waker};

use crate::sync::Arc;
use crate::{Event, EventListenerRc, IntoNotification};

use async_task::{Runnable, Task};
use atomic_waker::AtomicWaker;
use concurrent_queue::ConcurrentQueue;
use futures_lite::prelude::*;
use slab::Slab;

pub struct Executor<'a, T = DefaultThreadId> {
    /// Inner state of the executor.
    state: Arc<State<T>>,

    /// Capture the invariant lifetime.
    _marker: PhantomData<&'a Cell<Rc<()>>>,
}

impl<T> Drop for Executor<'_, T> {
    fn drop(&mut self) {
        // Wake all tasks.
        loop {
            let mut thread_state = self.state.thread_state.borrow_mut();
            let waker = match thread_state.active.drain().next() {
                Some(waker) => waker,
                None => break,
            };

            drop(thread_state);
            waker.wake();
        }

        // Drain the queues.
        while self.state.task_queue.pop().is_ok()
            && self
                .state
                .thread_state
                .borrow_mut()
                .task_queue
                .pop_front()
                .is_some()
        {}

        // Destroy the thread state.
        unsafe {
            ManuallyDrop::drop(&mut self.state.thread_state.borrow_mut());
        }
    }
}

struct State<T> {
    /// Mainstream queue of tasks.
    task_queue: ConcurrentQueue<Runnable>,

    /// Waker for the mainstream queue.
    mainstream_waker: AtomicWaker,

    /// Getter for the thread ID.
    thread_id: T,

    /// The thread ID of the origin thread.
    origin_thread: Option<NonZeroUsize>,

    /// State that can only be accessed by the thread that owns the executor.
    thread_state: RefCell<ManuallyDrop<ThreadState>>,
}

unsafe impl<T: Send + Sync> Send for State<T> {}
unsafe impl<T: Send + Sync> Sync for State<T> {}

struct ThreadState {
    /// Thread-local queue of tasks.
    task_queue: VecDeque<Runnable>,

    /// Waker for the thread-local queue.
    thread_waker: Rc<Event<Option<Runnable>>>,

    /// Slab of tasks that are currently running.
    active: Slab<Waker>,

    /// Is someone listening to the mainstream queue?
    is_mainstream_listening: bool,
}

impl<'a, T: Default + ThreadId + Send + Sync + 'static> Default for Executor<'a, T> {
    fn default() -> Self {
        Self::with_thread_id(T::default())
    }
}

impl<'a> Executor<'a> {
    /// Create a new executor with the default thread ID strategy.
    ///
    /// # Example
    ///
    /// ```
    /// use unsend::executor::Executor;
    ///
    /// let executor = Executor::new();
    /// ```
    pub fn new() -> Self {
        Self::with_thread_id(DefaultThreadId::new())
    }
}

impl<'a, T: ThreadId + Send + Sync + 'static> Executor<'a, T> {
    /// Create a new executor with the given thread ID strategy.
    ///
    /// # Example
    ///
    /// ```
    /// use unsend::executor::{Executor, StdThreadId};
    ///
    /// let executor = Executor::with_thread_id(StdThreadId::new());
    /// ```
    pub fn with_thread_id(thread_id: T) -> Self {
        Self {
            state: Arc::new(State {
                task_queue: ConcurrentQueue::unbounded(),
                mainstream_waker: AtomicWaker::new(),
                origin_thread: thread_id.id(),
                thread_id,
                thread_state: RefCell::new(ManuallyDrop::new(ThreadState {
                    task_queue: VecDeque::new(),
                    thread_waker: Rc::new(Event::new()),
                    active: Slab::new(),
                    is_mainstream_listening: false,
                })),
            }),
            _marker: PhantomData,
        }
    }

    /// Operate on the thread local state.
    fn with_thread_local<R>(&self, f: impl FnOnce(&mut ThreadState) -> R) -> R {
        // SAFETY: Since Executor is !Send, we have to be on the same thread.
        f(&mut self.state.thread_state.borrow_mut())
    }

    /// Tell if this executor is empty.
    pub fn is_empty(&self) -> bool {
        self.with_thread_local(|state| state.task_queue.is_empty())
            && self.state.task_queue.is_empty()
    }

    /// Spawn a new future onto the executor.
    ///
    /// # Example
    ///
    /// ```
    /// use unsend::executor::Executor;
    ///
    /// let executor = Executor::new();
    /// let task = executor.spawn(async {
    ///     println!("Hello, world!");
    /// });
    /// ```
    pub fn spawn<O: 'a>(&self, future: impl Future<Output = O> + 'a) -> Task<O> {
        let (runnable, task) = self.with_thread_local(move |state| {
            // Remove the task from the set of active tasks once it finishes.
            let index = state.active.vacant_key();
            let future = {
                let state = self.state.clone();
                async move {
                    // SAFETY: We are still on the origin thread.
                    let _guard = CallOnDrop(move || {
                        let mut thread_state = state.thread_state.borrow_mut();
                        drop(thread_state.active.try_remove(index));
                    });

                    future.await
                }
            };

            // Create the task and insert it into the set of active tasks.
            let (runnable, task) = unsafe { async_task::spawn_unchecked(future, self.schedule()) };
            state.active.insert(runnable.waker());

            (runnable, task)
        });

        runnable.schedule();
        task
    }

    /// Run a task if one is scheduled.
    ///
    /// # Example
    ///
    /// ```
    /// use unsend::executor::Executor;
    ///
    /// let executor = Executor::new();
    /// assert!(!executor.try_tick()); // No tasks are scheduled.
    ///
    /// // Spawn a task.
    /// let task = executor.spawn(async {
    ///     println!("Hello, world!");
    /// });
    ///
    /// assert!(executor.try_tick()); // The task is run.
    /// ```
    pub fn try_tick(&self) -> bool {
        let mut runnable = self.with_thread_local(|state| {
            // Try to run a task from the thread-local queue.
            if let Some(runnable) = state.task_queue.pop_front() {
                // Wake up another runner in case we take a while.
                state.thread_waker.notify(1.tag_with(|| None));
                Some(runnable)
            } else {
                None
            }
        });

        if runnable.is_none() {
            // Try the mainstream queue.
            if let Ok(r) = self.state.task_queue.pop() {
                // Wake up the mainstream runner in case we take a while.
                self.state.mainstream_waker.wake();
                runnable = Some(r);
            }
        }

        match runnable {
            Some(runnable) => {
                runnable.run();
                true
            }
            None => false,
        }
    }

    /// Run a single tick of the executor, waiting for a task to be scheduled if necessary.
    ///
    /// # Example
    ///
    /// ```
    /// use unsend::executor::Executor;
    /// use futures_lite::future;
    ///
    /// let executor = Executor::new();
    /// let task = executor.spawn(async {
    ///     println!("Hello, world!");
    /// });
    /// future::block_on(executor.tick());
    /// ```
    pub async fn tick(&self) {
        // Create a ticker and run a single tick.
        Ticker::new(&self.state).tick().await;
    }

    /// Run a future against the executor.
    ///
    /// # Example
    ///
    /// ```
    /// use unsend::executor::Executor;
    /// use futures_lite::future;
    ///
    /// let executor = Executor::new();
    /// let task = executor.spawn(async { 1 + 2 });
    /// let res = future::block_on(executor.run(async { task.await * 2 }));
    ///
    /// assert_eq!(res, 6);
    /// ```
    pub async fn run<O>(&self, f: impl Future<Output = O>) -> O {
        // A future that polls the executor forever.
        let runner = async move {
            let mut ticker = Ticker::new(&self.state);

            loop {
                ticker.tick().await;
            }
        };

        f.or(runner).await
    }

    /// The scheduler function.
    #[cfg_attr(coverage, no_coverage)]
    fn schedule(&self) -> impl Fn(Runnable) {
        let state = self.state.clone();
        move |runnable| {
            // If we are on the same thread, push to the thread-local queue.
            if let (Some(origin_id), Some(our_id)) = (state.origin_thread, state.thread_id.id()) {
                if origin_id == our_id {
                    let mut thread_state = state.thread_state.borrow_mut();
                    let mut runnable = Some(runnable);

                    // Try to send the runnable directly.
                    thread_state
                        .thread_waker
                        .notify(1.tag_with(|| runnable.take()));

                    // If that didn't take, push to the queue.
                    if let Some(runnable) = runnable {
                        thread_state.task_queue.push_back(runnable);
                    }

                    return;
                }
            }

            // Otherwise, push to the mainstream queue.
            if let Err(e) = state.task_queue.push(runnable) {
                // Don't drop the runnable on this thread; leak it.
                forget(e.into_inner());
                return;
            }

            state.mainstream_waker.wake();
        }
    }
}

/// The state of a future trying to tick the executor.
struct Ticker<'a, T> {
    /// The state of the executor.
    state: &'a State<T>,

    /// Are we the mainstream runner?
    is_mainstream: bool,

    /// This type is not `Send` or `Sync`.
    _marker: PhantomData<*const ()>,
}

impl<'a, T: ThreadId + Send + Sync + 'static> Ticker<'a, T> {
    /// Create a new ticker from the state.
    fn new(state: &'a State<T>) -> Self {
        Self {
            state,
            is_mainstream: false,
            _marker: PhantomData,
        }
    }

    /// Run a single tick of the executor, waiting for a task to be scheduled if necessary.
    async fn tick(&mut self) {
        // Create a listener.
        let listener = {
            // SAFETY: We are still on the origin thread.
            let thread_state = self.state.thread_state.borrow_mut();
            EventListenerRc::new(thread_state.thread_waker.clone())
        };

        futures_lite::pin!(listener);

        loop {
            // Try to pop from the thread local state.
            {
                // SAFETY: We are still on the origin thread.
                let mut thread_state = self.state.thread_state.borrow_mut();

                if let Some(runnable) = thread_state.task_queue.pop_front() {
                    // Wake up another runner in case we take a while.
                    thread_state.thread_waker.notify(1.tag_with(|| None));

                    // Run the runnable.
                    drop(thread_state);
                    runnable.run();

                    return;
                }

                // Acquire the mainstream runner if it hasn't been acquired yet.
                if !thread_state.is_mainstream_listening {
                    thread_state.is_mainstream_listening = true;
                    self.is_mainstream = true;
                }
            }

            let runnable = {
                if self.is_mainstream {
                    // If we are the mainstream, wait in tandem.
                    let mainstream_runnable = futures_lite::future::poll_fn(|cx| {
                        let mut waker_set = false;

                        loop {
                            // Try to pop from the mainstream queue.
                            if let Ok(runnable) = self.state.task_queue.pop() {
                                // Remove ourselves from the mainstream queue if necessary.
                                if waker_set {
                                    self.state.mainstream_waker.take();
                                }

                                return Poll::Ready(Some(runnable));
                            }

                            // Register our interest in the mainstream queue.
                            if !waker_set {
                                self.state.mainstream_waker.register(cx.waker());
                                waker_set = true;
                                continue;
                            }

                            // Begin the wait.
                            return Poll::Pending;
                        }
                    });

                    (&mut listener).or(mainstream_runnable).await
                } else {
                    // Just wait on the listener.
                    (&mut listener).await
                }
            };

            // Wait on the thread waker, and maybe get a runnable.
            if let Some(runnable) = runnable {
                // Run the runnable.
                runnable.run();

                // Poll the listener to see if there are more runnables.
                if let Some(Some(runnable)) = futures_lite::future::poll_once(listener).await {
                    // Run the runnable.
                    runnable.run();
                }

                return;
            }
        }
    }
}

impl<'a, T> Drop for Ticker<'a, T> {
    fn drop(&mut self) {
        // If we are the mainstream runner, wake up another runner.
        if self.is_mainstream {
            let _ = self.state.mainstream_waker.take();

            // SAFETY: We are still on the origin thread.
            let mut thread_state = self.state.thread_state.borrow_mut();
            thread_state.is_mainstream_listening = false;
            thread_state.thread_waker.notify(1.tag_with(|| None));
        }
    }
}

/// A getter for the current thread ID.
///
/// # Safety
///
/// The return value of `id` must be either `None` or a unique value for each thread.
pub unsafe trait ThreadId {
    /// Get the current thread ID, or `None` if it isn't available.
    fn id(&self) -> Option<NonZeroUsize>;
}

unsafe impl<T: ThreadId + ?Sized> ThreadId for &T {
    #[cfg_attr(coverage, no_coverage)]
    fn id(&self) -> Option<NonZeroUsize> {
        (**self).id()
    }
}

unsafe impl<T: ThreadId + ?Sized> ThreadId for &mut T {
    #[cfg_attr(coverage, no_coverage)]
    fn id(&self) -> Option<NonZeroUsize> {
        (**self).id()
    }
}

unsafe impl<T: ThreadId + ?Sized> ThreadId for alloc::boxed::Box<T> {
    #[cfg_attr(coverage, no_coverage)]
    fn id(&self) -> Option<NonZeroUsize> {
        (**self).id()
    }
}

unsafe impl<T: ThreadId + ?Sized> ThreadId for Rc<T> {
    #[cfg_attr(coverage, no_coverage)]
    fn id(&self) -> Option<NonZeroUsize> {
        (**self).id()
    }
}

unsafe impl<T: ThreadId + ?Sized> ThreadId for Arc<T> {
    #[cfg_attr(coverage, no_coverage)]
    fn id(&self) -> Option<NonZeroUsize> {
        (**self).id()
    }
}

/// Get the current thread ID using the standard library.
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct StdThreadId {
    _private: (),
}

#[cfg(feature = "std")]
impl StdThreadId {
    /// Create a new `StdThreadId`.
    #[inline(always)]
    pub fn new() -> Self {
        Self { _private: () }
    }
}

#[cfg(feature = "std")]
unsafe impl ThreadId for StdThreadId {
    fn id(&self) -> Option<NonZeroUsize> {
        std::thread_local! {
            static LOCAL: u8 = 0x03;
        }

        // Convert the address of the thread-local variable to a `usize`.
        LOCAL
            .try_with(|x| {
                // SAFETY: Addresses are always non-zero.
                unsafe { NonZeroUsize::new_unchecked(x as *const _ as usize) }
            })
            .ok()
    }
}

/// The thread ID is not available.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NoThreadId {
    _private: (),
}

impl NoThreadId {
    /// Create a new `NoThreadId`.
    #[inline(always)]
    pub fn new() -> Self {
        Self { _private: () }
    }
}

unsafe impl ThreadId for NoThreadId {
    #[inline(always)]
    fn id(&self) -> Option<NonZeroUsize> {
        None
    }
}

/// The thread ID used by default.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DefaultThreadId {
    #[cfg(feature = "std")]
    inner: StdThreadId,

    #[cfg(not(feature = "std"))]
    inner: NoThreadId,
}

impl DefaultThreadId {
    /// Create a new `DefaultThreadId`.
    #[inline(always)]
    pub fn new() -> Self {
        Self::default()
    }
}

unsafe impl ThreadId for DefaultThreadId {
    #[inline(always)]
    fn id(&self) -> Option<NonZeroUsize> {
        self.inner.id()
    }
}

struct CallOnDrop<F: FnMut()>(F);

impl<F: FnMut()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future;

    #[test]
    fn smoke() {
        future::block_on(async {
            let slot = RefCell::new(0);
            let ex = Executor::new();

            let t1 = ex.spawn(async {
                *slot.borrow_mut() += 1;
                17
            });

            let t2 = ex.spawn(async {
                *slot.borrow_mut() += 2;
                18
            });

            // Wait for the tasks to finish.
            ex.run(async {
                assert_eq!(t1.await, 17);
                assert_eq!(t2.await, 18);
                assert!(ex.is_empty());
                assert_eq!(*slot.borrow(), 3);
            })
            .await;
        });
    }

    #[test]
    fn smoke_no_thread_id() {
        future::block_on(async {
            let slot = RefCell::new(0);
            let ex = Executor::with_thread_id(NoThreadId::new());

            let t1 = ex.spawn(async {
                *slot.borrow_mut() += 1;
                17
            });

            let t2 = ex.spawn(async {
                *slot.borrow_mut() += 2;
                18
            });

            // Wait for the tasks to finish.
            ex.run(async {
                assert_eq!(t1.await, 17);
                assert_eq!(t2.await, 18);
                assert!(ex.is_empty());
                assert_eq!(*slot.borrow(), 3);
            })
            .await;
        });
    }

    #[cfg(feature = "std")]
    #[test]
    fn try_tick() {
        use std::thread;

        future::block_on(async {
            let ex = Executor::new();
            assert!(!ex.try_tick());

            // Spawn a task that will wake up the executor.
            let task = ex.spawn({
                let mut polls_left = 5;
                async move {
                    while polls_left > 0 {
                        future::yield_now().await;
                        polls_left -= 1;
                    }
                }
            });

            ex.run(async {
                // Poll the task once.
                assert!(ex.try_tick());

                // Poll again.
                assert!(ex.try_tick());

                // Send to another thread and poll until we're done.
                thread::spawn(move || {
                    future::block_on(task);
                });

                // Poll until we're done.
                assert!(ex.try_tick());

                ex.tick().await;

                while !ex.is_empty() {
                    ex.tick().await;
                }
            })
            .await;
        })
    }

    #[test]
    fn default_smoke() {
        let _: Executor<'_, NoThreadId> = Executor::default();
    }
}
