//! An executor for a program.

use crate::notify::{Listener, Notify};
use crate::once_cell::OnceCell;
use crate::queue::Queue;
use crate::task::{spawn_unchecked, Detached, Runnable, Task};

use alloc::rc::Rc;
use alloc::sync::Arc;

use core::cell::{RefCell, UnsafeCell};
use core::future::Future;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use slab::Slab;

/// An executor that runs tasks in sequence.
pub struct Executor<'a> {
    /// Inner state.
    inner: OnceCell<Rc<State>>,

    /// Capture the invariant runtime.
    _phantom: PhantomData<UnsafeCell<&'a ()>>,
}

struct State {
    /// The queue of tasks.
    tasks: Queue<Runnable>,

    /// The signal for when a task has been pushed to the queue.
    task_pushed: Notify<Runnable>,

    /// Every currently running task.
    running: RefCell<Slab<RunningTask>>,
}

/// A single task running on this executor.
struct RunningTask {
    /// The waker for the task.
    waker: Waker,

    /// The detached handle for this task, if it is detached.
    handle: Option<Rc<RefCell<DetachedHandle>>>,
}

struct DetachedHandle {
    /// The detached handle for this task.
    detached: Detached,

    /// The signal for when the task is ready to be rescheduled.
    signal: Arc<Signal>,

    /// The waker to use as a context when polling the `detached` future.
    waker: Waker,
}

impl Drop for Executor<'_> {
    fn drop(&mut self) {
        // Run all of the wakers to ensure that all tasks are dropped.
        if let Some(state) = self.inner.get() {
            state.running.borrow_mut().drain().for_each(|task| {
                task.waker.wake();
            });

            // Drain the queue.
            state.tasks.clear();
        }
    }
}

impl<'a> Executor<'a> {
    /// Create a new `Executor`.
    pub const fn new() -> Self {
        Self {
            inner: OnceCell::new(),
            _phantom: PhantomData,
        }
    }

    /// Create a new `Executor` with the given initial capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: OnceCell::from(Rc::new(State {
                task_pushed: Notify::new(),
                tasks: Queue::new(capacity),
                running: RefCell::new(Slab::new()),
            })),
            _phantom: PhantomData,
        }
    }

    /// Returns `true` if there are no finished tasks.
    pub fn is_empty(&self) -> bool {
        self.state().tasks.is_empty()
    }

    /// Spawn a new task on the executor.
    pub fn spawn<F>(&self, future: F) -> Task<F::Output>
    where
        F: Future + 'a,
        F::Output: 'a,
    {
        self.spawn_returns_id(future).0
    }

    /// Spawn a new daemon task on the executor.
    pub fn spawn_detached(&self, future: impl Future + 'a) {
        let (task, task_key) = self.spawn_returns_id(future);

        // Create a signal and a waker to poll it.
        let signal = Arc::new(Signal(AtomicBool::new(false)));
        let waker = signal.clone().into_waker();

        // Insert the new detached handle.
        let handle = self.state().running.borrow_mut()[task_key]
            .handle
            .insert(Rc::new(RefCell::new(DetachedHandle {
                detached: task.detach(),
                signal,
                waker,
            })))
            .clone();

        // Poll the detached handle once to register the new waker.
        handle.borrow_mut().poll();
    }

    fn spawn_returns_id<F>(&self, future: F) -> (Task<F::Output>, usize)
    where
        F: Future + 'a,
        F::Output: 'a,
    {
        // Wrap the future such that it deregisters itself once it completes.
        let task_key = self.state().running.borrow().vacant_key();
        let future = {
            let state = self.state().clone();

            async move {
                // Deregister this task once we're done.
                let _guard = CallOnDrop(move || {
                    state.running.borrow_mut().remove(task_key);
                });

                // Run the task.
                future.await
            }
        };

        // SAFETY: We prevent the waker/future from outliving the executor.
        let (runnable, task) = unsafe { spawn_unchecked(future, self.schedule()) };

        // Insert the task into our active list.
        self.state().running.borrow_mut().insert(RunningTask {
            waker: runnable.waker(),
            handle: None,
        });

        // Schedule the task.
        runnable.schedule();
        (task, task_key)
    }

    /// Poll all of the detached tasks on the executor.
    fn poll_detached(&self) {
        if let Some(state) = self.inner.get() {
            let raised_tasks = state
                .running
                .borrow()
                .iter()
                .filter_map(|(_, task)| task.handle.as_ref().cloned())
                .filter(|task| task.borrow().signal.raised())
                .collect::<Vec<_>>();

            // Poll all of the signaled tasks.
            for task in raised_tasks {
                task.borrow_mut().poll();
            }
        }
    }

    /// Run a future on this executor.
    pub fn run<'this, F: Future>(&'this self, future: F) -> Run<'this, 'a, F> {
        Run {
            executor: self,
            listener: None,
            future,
        }
    }

    /// Try to run a task on the executor.
    ///
    /// Returns `true` if there was a task waiting.
    pub fn try_tick(&self) -> bool {
        let mut tried_detached = false;

        loop {
            // Try to pop a task from the queue.
            if let Some(task) = self.state().tasks.pop() {
                task.run();
                return true;
            }

            // If we haven't tried to poll the detached tasks yet, do so now.
            if !tried_detached {
                tried_detached = true;
                self.poll_detached();
                continue;
            }

            // Otherwise, there are no tasks waiting.
            return false;
        }
    }

    /// Run a task on the executor.
    ///
    /// If there isn't a task yet, waits until there is one.
    pub async fn tick(&self) {
        // See if there is a task waiting.
        if self.try_tick() {
            return;
        }

        // Otherwise, wait for a task to be pushed.
        let listener = self.state().task_pushed.listen();
        let task = listener.await;
        task.run();
    }

    /// Get the scheduling function for this executor.
    fn schedule(&self) -> impl Fn(Runnable) {
        let state = self.state().clone();
        move |runnable| {
            // Try to send the task directly to the runner.
            if let Err(runnable) = state.notify(runnable) {
                // Otherwise, push the task to the queue.
                state.tasks.push(runnable);
            }
        }
    }

    /// Get a reference to the inner state.
    fn state(&self) -> &Rc<State> {
        self.inner.get_or_init(|| {
            Rc::new(State {
                tasks: Queue::new(0),
                task_pushed: Notify::new(),
                running: RefCell::new(Slab::new()),
            })
        })
    }
}

impl State {
    /// Wake up a task.
    fn notify(&self, runnable: Runnable) -> Result<(), Runnable> {
        let mut runnable = Some(runnable);
        self.task_pushed.notify(1, || runnable.take().unwrap());

        match runnable {
            Some(runnable) => Err(runnable),
            None => Ok(()),
        }
    }
}

impl DetachedHandle {
    /// Poll this `DetachedHandle`.
    fn poll(&mut self) {
        let _ = Pin::new(&mut self.detached).poll(&mut Context::from_waker(&self.waker));
    }
}

pin_project_lite::pin_project! {
    /// A future that runs a task on an executor.
    pub struct Run<'a, 'b, F> {
        // The reference to the executor.
        executor: &'a Executor<'b>,

        // The listener waiting for a task to be pushed.
        #[pin]
        listener: Option<Listener<'a, Runnable>>,

        // The future we're running with the executor.
        #[pin]
        future: F,
    }
}

impl<F: Future> Future for Run<'_, '_, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // Poll the future to see if it's ready.
        if let Poll::Ready(value) = this.future.poll(cx) {
            return Poll::Ready(value);
        }

        // If we have a listener, see if it's ready.
        if let Some(listener) = this.listener.as_mut().as_pin_mut() {
            let runnable = ready!(listener.poll(cx));
            this.listener.set(None);
            runnable.run();
        }

        // Run futures from the executor. Only run a limited number to prevent starvation.
        for _ in 0..200 {
            // Get the next task.
            let runnable = this.executor.state().tasks.pop();

            match runnable {
                Some(runnable) => {
                    runnable.run();
                }
                None => {
                    // Try polling the detached tasks.
                    this.executor.poll_detached();
                    if let Some(task) = this.executor.state().tasks.pop() {
                        task.run();
                        continue;
                    }

                    // No tasks left, so we need to wait for a task to be pushed.
                    this.listener
                        .set(Some(this.executor.state().task_pushed.listen()));
                    let runnable = ready!(this.listener.as_mut().as_pin_mut().unwrap().poll(cx));
                    this.listener.set(None);
                    runnable.run();
                }
            }
        }

        Poll::Pending
    }
}

struct Signal(AtomicBool);

impl Signal {
    /// Tell if the signal has been raised.
    fn raised(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }

    /// Create a new waker from an `Arc`.
    fn into_waker(self: Arc<Self>) -> Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);

        unsafe fn wake(this: *const ()) {
            let this = Arc::from_raw(this as *const Signal);
            this.0.store(true, Ordering::Release);
        }

        unsafe fn wake_by_ref(this: *const ()) {
            let this = ManuallyDrop::new(Arc::from_raw(this as *const Signal));
            this.0.store(true, Ordering::Release);
        }

        unsafe fn clone(this: *const ()) -> RawWaker {
            let this = ManuallyDrop::new(Arc::from_raw(this as *const Signal));

            RawWaker::new(Arc::into_raw((*this).clone()) as *const (), &VTABLE)
        }

        unsafe fn drop(this: *const ()) {
            core::mem::drop(Arc::from_raw(this as *const Signal));
        }

        unsafe { Waker::from_raw(RawWaker::new(Arc::into_raw(self) as *const (), &VTABLE)) }
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
    use futures_lite::future::block_on;

    #[test]
    fn test_executor() {
        let mut data = 11;

        {
            let executor = Executor::new();

            // Spawn a task.
            let task = executor.spawn({
                let data = &mut data;
                async move {
                    *data = 32;
                }
            });

            // Run the executor.
            block_on(executor.run(async {
                // Wait for the task to finish.
                task.await;
            }));
        }

        assert_eq!(data, 32);
    }
}
