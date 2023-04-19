//! A recursive indicator that the thread is locked.

use super::Context;

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::{Condvar, Mutex, Once};
use std::thread::{current, ThreadId};

fn thread_id() -> ThreadId {
    thread_local! {
        static ID: ThreadId = current().id();
    }

    ID.try_with(|id| *id).unwrap_or_else(|_| current().id())
}

/// A recursive indicator that the thread is locked.
pub struct LockContext {
    /// Mutex containing the state.
    state: Mutex<State>,

    /// Condition variable used to notify the thread.
    condvar: Condvar,
}

/// The state of the lock.
struct State {
    /// The thread ID of the thread that owns the lock.
    owner: Option<ThreadId>,

    /// The number of times the lock has been acquired by this thread.
    count: usize,
}

impl LockContext {
    /// Create a new lock context.
    pub fn new() -> LockContext {
        LockContext {
            state: Mutex::new(State {
                owner: None,
                count: 0,
            }),
            condvar: Condvar::new(),
        }
    }
}

impl Default for LockContext {
    fn default() -> LockContext {
        LockContext::new()
    }
}

unsafe impl Context for LockContext {
    fn lock(&self) {
        let mut state = self.state.lock().unwrap();

        if state.owner == Some(thread_id()) {
            state.count += 1;
        } else {
            while state.owner.is_some() {
                state = self.condvar.wait(state).unwrap();
            }

            state.owner = Some(thread_id());
            state.count = 1;
        }
    }

    unsafe fn unlock(&self) {
        let mut state = self.state.lock().unwrap();

        if cfg!(debug_assertions) && state.owner != Some(thread_id()) {
            panic!("attempted to unlock a lock that is not locked by the current thread");
        }

        state.count -= 1;

        if state.count == 0 {
            state.owner = None;
            self.condvar.notify_one();
        }
    }
}

/// The global lock context.
#[derive(Clone)]
pub struct GlobalContext {
    inner: &'static LockContext,
}

impl GlobalContext {
    /// Get a reference to the global lock context.
    pub fn new() -> GlobalContext {
        GlobalContext {
            inner: GLOBAL.get(),
        }
    }
}

impl Default for GlobalContext {
    fn default() -> GlobalContext {
        GlobalContext::new()
    }
}

unsafe impl Context for GlobalContext {
    fn lock(&self) {
        self.inner.lock();
    }

    unsafe fn unlock(&self) {
        self.inner.unlock();
    }
}

struct GlobalInner {
    context: UnsafeCell<MaybeUninit<LockContext>>,
    once: Once,
}

impl GlobalInner {
    const fn new() -> GlobalInner {
        GlobalInner {
            context: UnsafeCell::new(MaybeUninit::uninit()),
            once: Once::new(),
        }
    }

    fn get(&self) -> &LockContext {
        self.once.call_once(|| unsafe {
            self.context
                .get()
                .write(MaybeUninit::new(LockContext::new()));
        });

        unsafe { &*(self.context.get() as *const _ as *const LockContext) }
    }
}

unsafe impl Sync for GlobalInner {}

static GLOBAL: GlobalInner = GlobalInner::new();

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::thread::{sleep, spawn};
    use std::time::Duration;

    #[test]
    fn lock_context() {
        let context = LockContext::new();

        context.lock();
        context.lock();
        unsafe {
            context.unlock();
            context.unlock();
        }
    }

    #[test]
    fn blocks_other_thread() {
        let context = Arc::new(LockContext::new());

        context.lock();

        let handle = spawn({
            let context = context.clone();
            move || {
                context.lock();
                unsafe {
                    context.unlock();
                }
            }
        });

        sleep(Duration::from_millis(100));

        unsafe {
            context.unlock();
        }

        handle.join().unwrap();
    }
}
