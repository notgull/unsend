//! Task abstraction.
//!
//! Note that parts of the task have to use atomics, since wakers are expected to be `Send` and `Sync`.
//! However, parts that are used only by the `Task` and `Runnable` are always accessed from the same
//! thread, so they can use the normal unsynchronous primitives.

use atomic_waker::AtomicWaker;

use alloc::boxed::Box;

use core::cell::{Cell, RefCell};
use core::future::Future;
use core::marker::PhantomData;
use core::mem::{forget, MaybeUninit};
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// Spawn a new task.
///
/// # Safety
///
/// todo
pub unsafe fn spawn_unchecked<Fut, Sched>(
    future: Fut,
    schedule: Sched,
) -> (Runnable, Task<Fut::Output>)
where
    Fut: Future,
    Sched: Fn(Runnable),
{
    // Allocate the task.
    let inner = {
        let boxed = Box::new(TaskInner {
            shared: SharedState {
                // We start with two references: the task and the runnable.
                refcount: AtomicUsize::new(2 << REFCOUNT_SHIFT),
                task_waker: AtomicWaker::new(),
                vtable: &TaskInner::<Fut, Fut::Output, Sched>::TASK_VTABLE,
                cancelled: Cell::new(false),
                completed: Cell::new(false),
                scheduled: Cell::new(false),
            },
            data: TaskData {
                state: RefCell::new(TaskState::<Fut, Fut::Output>::Running(future)),
                schedule,
            },
        });

        NonNull::from(Box::leak(boxed))
    };

    // Create the task.
    let task = Task {
        inner: Detached {
            inner: Some(inner.cast()),
        },
        _generic: PhantomData,
    };

    // Create the runnable.
    let runnable = Runnable {
        inner: inner.cast(),
    };

    (runnable, task)
}

/// Spawn a static task.
pub fn spawn<Fut, Sched>(future: Fut, schedule: Sched) -> (Runnable, Task<Fut::Output>)
where
    Fut: Future + 'static,
    Fut::Output: 'static,
    Sched: Fn(Runnable) + 'static,
{
    unsafe { spawn_unchecked(future, schedule) }
}

/// The handle for a task.
pub struct Task<T> {
    /// The inner task.
    inner: Detached,

    /// The captured output.
    _generic: PhantomData<T>,
}

impl<T> Task<T> {
    /// Poll the task for its output.
    fn poll_task(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let inner = self.inner.inner.unwrap();
        let header = unsafe { inner.cast::<SharedState>().as_ref() };

        // If the task is cancelled and not scheduled, return `Poll::Ready(None)`.
        if header.cancelled.get() && !header.scheduled.get() {
            return Poll::Ready(None);
        }

        // Register our waker.
        header.task_waker.register(cx.waker());

        // If the output is available, return it.
        if header.completed.get() {
            let mut slot = MaybeUninit::<T>::uninit();
            if unsafe {
                (header.vtable.output)(inner, NonNull::new_unchecked(slot.as_mut_ptr()).cast())
            } {
                return Poll::Ready(Some(unsafe { slot.assume_init() }));
            }
        }

        // If the task needs to be rescheduled, do so now.
        if header.refcount.fetch_and(!NEEDS_SCHEDULE, Ordering::AcqRel) & NEEDS_SCHEDULE != 0 {
            unsafe {
                (header.vtable.schedule)(inner);
            }
        }

        // Wait for the output to become available.
        Poll::Pending
    }

    /// A fallible task that returns `None` if the waker is dropped.
    pub fn fallible(self) -> FallibleTask<T> {
        FallibleTask(self)
    }

    /// Detach this task and let it run in the background.
    pub fn detach(self) -> Detached {
        let inner = unsafe { core::ptr::read(&self.inner) };
        forget(self);
        inner
    }

    /// Cancel the task.
    ///
    /// This waits for one last time for the task to be scheduled, and then cancels it.
    pub async fn cancel(mut self) -> Option<T> {
        self.set_cancelled();
        self.fallible().await
    }

    fn set_cancelled(&mut self) {
        self.inner.set_cancelled();
    }
}

impl<T> Unpin for Task<T> {}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut().poll_task(cx) {
            Poll::Ready(Some(output)) => Poll::Ready(output),
            Poll::Ready(None) => panic!("task was cancelled"),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A task that can fail.
pub struct FallibleTask<T>(Task<T>);

impl<T> FallibleTask<T> {
    /// Cancel the task.
    pub async fn cancel(self) -> Option<T> {
        self.0.cancel().await
    }
}

impl<T> Future for FallibleTask<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().0.poll_task(cx)
    }
}

/// A detached task.
pub struct Detached {
    /// The inner task.
    inner: Option<NonNull<()>>,
}

impl Detached {
    /// Cancel the task and return the inner task pointer.
    fn set_cancelled(&mut self) {
        if let Some(inner) = self.inner {
            // Cancel the task.
            let header = unsafe { inner.cast::<SharedState>().as_ref() };
            if header.cancelled.replace(true) {
                return;
            }

            // If the task is not scheduled, schedule it to run one last time.
            if !header.scheduled.get() {
                unsafe {
                    (header.vtable.schedule)(inner);
                }
            }
        }
    }
}

impl Unpin for Detached {}

impl Future for Detached {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.inner.unwrap();
        let header = unsafe { inner.cast::<SharedState>().as_ref() };

        // If the task is cancelled and not scheduled, return `Poll::Ready(None)`.
        if header.cancelled.get() && !header.scheduled.get() {
            return Poll::Ready(());
        }

        // Register our waker.
        header.task_waker.register(cx.waker());

        // If the task needs to be rescheduled, do so now.
        if header.refcount.fetch_and(!NEEDS_SCHEDULE, Ordering::AcqRel) & NEEDS_SCHEDULE != 0 {
            unsafe {
                (header.vtable.schedule)(inner);
            }
        }

        // Wait for the output to become available.
        Poll::Pending
    }
}

impl Drop for Detached {
    fn drop(&mut self) {
        self.set_cancelled();

        if let Some(inner) = self.inner.take() {
            // Cancel and drop the task.
            unsafe { (inner.cast::<SharedState>().as_ref().vtable.drop_ref)(inner.as_ptr() as _) };
        }
    }
}

/// The runnable for a task.
pub struct Runnable {
    /// The inner runnable.
    inner: NonNull<()>,
}

impl Runnable {
    /// Run the task and drop it after.
    ///
    /// Returns `true` if the task's waker was called during running, meaning that the task is immediately
    /// ready to run again.
    pub fn run(self) -> bool {
        let header = unsafe { self.inner.cast::<SharedState>().as_ref() };

        // Run the task.
        header.scheduled.set(false);
        unsafe { (header.vtable.run)(self.inner) };

        // See if we need to be rescheduled.
        let needs_reschedule = header.refcount.load(Ordering::Relaxed) & NEEDS_SCHEDULE != 0;

        // Drop the task reference without cancelling it.
        unsafe { (header.vtable.drop_ref)(self.inner.as_ptr() as _) };
        forget(self);

        needs_reschedule
    }

    /// Schedule the task to run.
    pub fn schedule(self) {
        let header = unsafe { self.inner.cast::<SharedState>().as_ref() };

        // Schedule the task.
        unsafe { (header.vtable.schedule)(self.inner) };

        // Drop the task reference without cancelling it.
        unsafe { (header.vtable.drop_ref)(self.inner.as_ptr() as _) };
        forget(self);
    }

    /// Create a waker that can be used to schedule the task.
    pub fn waker(&self) -> Waker {
        unsafe {
            let raw =
                (self.inner.cast::<SharedState>().as_ref().vtable.waker)(self.inner.as_ptr() as _);
            Waker::from_raw(raw)
        }
    }
}

impl Drop for Runnable {
    fn drop(&mut self) {
        // Cancel and drop the task.
        let header = unsafe { self.inner.cast::<SharedState>().as_ref() };

        header.cancelled.set(true);
        unsafe { (header.vtable.drop_ref)(self.inner.as_ptr() as _) };
    }
}

#[repr(C)]
struct TaskInner<Fut, Out, Sched> {
    /// The state shared between the task and its wakers.
    ///
    /// This must come first so that a pointer cast can access it.
    shared: SharedState,

    /// The data for the task.
    data: TaskData<Fut, Out, Sched>,
}

/// Shared state for a task.
struct SharedState {
    /// The current reference count.
    ///
    /// The refcount is actually shifted over by two. If the task needs to be rescheduled,
    /// the lowest bit is set to 1.
    refcount: AtomicUsize,

    /// The waker for this task.
    ///
    /// This needs to be atomic because the scheduling waker accesses it.
    task_waker: AtomicWaker,

    /// Virtual call table for this task.
    vtable: &'static Vtable,

    // -- Anything below this line is not accessed by the waker. --
    /// Whether or not the task has been cancelled.
    cancelled: Cell<bool>,

    /// Whether or not the task is currently scheduled.
    scheduled: Cell<bool>,

    /// Whether or not the task was previously completed.
    completed: Cell<bool>,
}

/// The data for a task.
struct TaskData<Fut, Out, Sched> {
    /// Current state of the task.
    state: RefCell<TaskState<Fut, Out>>,

    /// The scheduler function.
    schedule: Sched,
}

const REFCOUNT_SHIFT: usize = 1;
const NEEDS_SCHEDULE: usize = 1;

unsafe impl<Fut, Out, Sched> Send for TaskInner<Fut, Out, Sched> {}
unsafe impl<Fut, Out, Sched> Sync for TaskInner<Fut, Out, Sched> {}

impl<Fut, Sched> TaskInner<Fut, Fut::Output, Sched>
where
    Fut: Future,
    Sched: Fn(Runnable),
{
}

enum TaskState<Fut, Out> {
    /// Task is still running.
    Running(Fut),

    /// Task has completed.
    Completed(Option<Out>),
}

/// Virtual call table for task operations.
struct Vtable {
    /// Run the task's runnable.
    run: unsafe fn(NonNull<()>),

    /// Schedule the task's runnable.
    schedule: unsafe fn(NonNull<()>),

    /// Create a new waker for the task.
    waker: unsafe fn(*const ()) -> RawWaker,

    /// Read the output to a slot in memory.
    output: unsafe fn(NonNull<()>, NonNull<()>) -> bool,

    /// Drop a reference to the task.
    drop_ref: unsafe fn(*const ()),
}

impl<Fut, Sched> TaskInner<Fut, Fut::Output, Sched>
where
    Fut: Future,
    Sched: Fn(Runnable),
{
    const RAW_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::waker,
        Self::wake,
        Self::wake_by_ref,
        Self::decrement_ref,
    );

    const TASK_VTABLE: Vtable = Vtable {
        run: Self::run,
        schedule: Self::schedule,
        waker: Self::waker,
        output: Self::output,
        drop_ref: Self::decrement_ref,
    };

    /// Run the task once.
    unsafe fn run(this: NonNull<()>) {
        let task = this.cast::<Self>().as_ref();

        // Get a reference to the future.
        let mut state = task.data.state.borrow_mut();
        let fut = match &mut *state {
            TaskState::Running(fut) => fut,
            TaskState::Completed(_) => panic!("Tried to run a completed task"),
        };

        // SAFETY: `fut` is already pinned on the heap.
        let mut fut = unsafe { Pin::new_unchecked(fut) };
        let waker =
            unsafe { Waker::from_raw(RawWaker::new(this.as_ptr(), &Self::RAW_WAKER_VTABLE)) };
        let mut context = Context::from_waker(&waker);

        // Poll the future once.
        let result = fut.as_mut().poll(&mut context);

        if let Poll::Ready(result) = result {
            // The task has completed.
            task.shared.completed.set(true);
            *state = TaskState::Completed(Some(result));
            task.shared.task_waker.wake();
        }
    }

    /// Schedule the task to run.
    unsafe fn schedule(this: NonNull<()>) {
        Self::schedule_inner(this, true);
    }

    /// Schedule the task, incrementing the count if necessary.
    unsafe fn schedule_inner(this: NonNull<()>, increment: bool) {
        let task = this.cast::<Self>().as_ref();

        // If the task has already been scheduled, don't schedule it again.
        if task.shared.scheduled.replace(true) {
            return;
        }

        // If the task has already completed, don't schedule it.
        if task.shared.completed.get() {
            return;
        }

        // Schedule the task.
        if increment {
            Self::increment_ref(this);
        }
        (task.data.schedule)(Runnable { inner: this });
    }

    /// Read the inner value out of the task.
    unsafe fn output(this: NonNull<()>, output: NonNull<()>) -> bool {
        let task = this.cast::<Self>().as_ref();
        let output = output.cast::<Fut::Output>();

        let mut guard = task.data.state.borrow_mut();
        match &mut *guard {
            TaskState::Running(_) => false,
            TaskState::Completed(result) => {
                if let Some(result) = result.take() {
                    output.as_ptr().write(result);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Create a new waker for this task.
    unsafe fn waker(this: *const ()) -> RawWaker {
        Self::increment_ref(NonNull::new_unchecked(this as _));
        RawWaker::new(this, &Self::RAW_WAKER_VTABLE)
    }

    /// Wake the task by reference.
    unsafe fn wake_by_ref(this: *const ()) {
        let task = &*(this as *const Self);
        task.shared
            .refcount
            .fetch_or(NEEDS_SCHEDULE, Ordering::AcqRel);
        task.shared.task_waker.wake();
    }

    /// Wake the task.
    unsafe fn wake(this: *const ()) {
        Self::wake_by_ref(this);
        Self::decrement_ref(this);
    }

    unsafe fn increment_ref(this: NonNull<()>) {
        let header = this.cast::<SharedState>().as_ref();

        // Abort on potential overflow.
        let new_count = header.refcount.fetch_add(REFCOUNT_SHIFT, Ordering::Relaxed);
        if new_count > core::isize::MAX as usize {
            abort!("Task refcount overflow");
        }
    }

    /// Decrement the reference count on the task.
    unsafe fn decrement_ref(this: *const ()) {
        let header = &*(this as *const SharedState);
        let new_count = header.refcount.fetch_sub(REFCOUNT_SHIFT, Ordering::Release);

        if new_count < REFCOUNT_SHIFT {
            // Needs to be dropped.
            Self::destroy(NonNull::new_unchecked(this as _));
        }
    }

    /// Destroy this task.
    unsafe fn destroy(this: NonNull<()>) {
        let task = this.cast::<Self>().as_ptr();
        drop(Box::from_raw(task));
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
    use crate::mpmc;
    use core::marker::PhantomData;
    use futures_lite::{future, prelude::*};

    struct SimpleExecutor<'a> {
        runnables: mpmc::Receiver<Runnable>,
        sender: mpmc::Sender<Runnable>,
        _lifetime: PhantomData<&'a mut &'a ()>,
    }

    impl<'a> SimpleExecutor<'a> {
        fn new() -> Self {
            let (sender, runnables) = mpmc::channel(12);
            Self {
                runnables,
                sender,
                _lifetime: PhantomData,
            }
        }

        fn spawn<R>(&self, future: impl Future<Output = R>) -> Task<R> {
            let sender = self.sender.clone();
            let (runnable, task) = unsafe {
                spawn_unchecked(future, move |runnable| {
                    sender.send(runnable).ok();
                })
            };

            runnable.schedule();
            task
        }

        async fn run<R>(&self, future: impl Future<Output = R>) -> R {
            let run_future = future::poll_fn(|_| loop {
                if let Ok(runnable) = self.runnables.try_recv() {
                    runnable.run();
                } else {
                    return Poll::Pending;
                }
            });

            future.or(run_future).await
        }
    }

    #[test]
    fn smoke() {
        let executor = SimpleExecutor::new();
        let task = executor.spawn(async { 42 });
        future::block_on(async {
            assert_eq!(executor.run(task).await, 42);
        });
    }
}
