//! A thread-unsafe runtime for thread-unsafe people.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod channel;
pub mod lock;

mod event;
#[cfg(feature = "std")]
mod recursive_mutex;

pub use event::{Event, EventListener, IntoNotification, Notification};
#[cfg(feature = "std")]
pub use recursive_mutex::{GlobalContext, LockContext};

/// The context for a thread-unsafe runtime.
///
/// # Safety
///
/// This must ensure that only one thread at a time can access the runtime.
pub unsafe trait Context {
    /// Lock the context.
    fn lock(&self);

    /// Unlock the context.
    ///
    /// # Safety
    ///
    /// This can only be called after `lock` has succeeded.
    unsafe fn unlock(&self);
}

unsafe impl<C: Context + ?Sized> Context for &C {
    fn lock(&self) {
        (**self).lock()
    }

    unsafe fn unlock(&self) {
        (**self).unlock()
    }
}

#[cfg(feature = "alloc")]
unsafe impl<C: Context + ?Sized> Context for alloc::boxed::Box<C> {
    fn lock(&self) {
        (**self).lock()
    }

    unsafe fn unlock(&self) {
        (**self).unlock()
    }
}

#[cfg(feature = "alloc")]
unsafe impl<C: Context + ?Sized> Context for alloc::rc::Rc<C> {
    fn lock(&self) {
        (**self).lock()
    }

    unsafe fn unlock(&self) {
        (**self).unlock()
    }
}

#[cfg(feature = "alloc")]
unsafe impl<C: Context + ?Sized> Context for alloc::sync::Arc<C> {
    fn lock(&self) {
        (**self).lock()
    }

    unsafe fn unlock(&self) {
        (**self).unlock()
    }
}
