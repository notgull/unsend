//! A thread-unsafe async runtime for thread-unsafe people.

#![cfg_attr(not(feature = "std"), no_std)]

macro_rules! abort {
    ($($tt: tt)*) => {
        crate::abort_with_message(format_args!($($tt)*))
    };
}

/// Short circuit for an async operation.
#[macro_export]
macro_rules! ready {
    ($expr: expr) => {{
        match ($expr) {
            ::core::task::Poll::Ready(t) => t,
            ::core::task::Poll::Pending => return ::core::task::Poll::Pending,
        }
    }};
}

extern crate alloc;

mod notify;
mod once_cell;
mod queue;

pub mod executor;
pub mod lock;
pub mod mpmc;
pub mod oneshot;
pub mod spin_on;
pub mod task;

use core::fmt;

/// Abort the current program.
fn abort() -> ! {
    #[cfg(feature = "std")]
    std::process::abort();

    #[cfg(not(feature = "std"))]
    {
        // Preform an abort by panicking while panicking.
        struct Abort;

        impl Drop for Abort {
            fn drop(&mut self) {
                panic!("Panic while panicking to abort");
            }
        }

        let _abort = Abort;
        panic!("Panic while panicking to abort");
    }
}

/// Abort the current program with a message.
fn abort_with_message(message: fmt::Arguments<'_>) -> ! {
    /// The logger can panic, so abort if it does.
    struct AbortOnDrop;

    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            abort();
        }
    }

    let _abort = AbortOnDrop;
    log::error!("Aborting: {}", message);
    abort();
}
