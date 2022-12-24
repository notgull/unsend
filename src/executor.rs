//! An executor for a program.

use crate::notify::{Listener, Notify};
use crate::once_cell::OnceCell;
use crate::queue::Queue;
use crate::task::{spawn_unchecked, Runnable, Task};

use alloc::rc::Rc;

use core::cell::UnsafeCell;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

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
        F: Future<Output = ()> + 'a,
        F::Output: 'a,
    {
        // SAFETY: We prevent the waker/future from outliving the executor.
        let (runnable, task) = unsafe { spawn_unchecked(future, self.schedule()) };

        // Schedule the task.
        runnable.schedule();
        task
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
        match self.state().tasks.pop() {
            Some(runnable) => {
                runnable.run();
                true
            }
            None => false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;

    #[test]
    fn test_executor() {
        let executor = Executor::new();

        // Spawn a task.
        let mut data = 11;
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

        assert_eq!(data, 32);
    }
}
