//! A simple reactor that polls a future to completion in a tight loop.

use core::future::Future;
use core::ptr;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// A simple reactor that polls a future to completion in a tight loop.
/// 
/// # Never Use This
/// 
/// This function is provided primarily for completion's sake. It is not intended to be used in
/// production code.
pub fn spin_on<R>(f: impl Future<Output = R>) -> R {
    // Pin the future and create the context.
    pin_utils::pin_mut!(f);
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);

    loop {
        match f.as_mut().poll(&mut context) {
            Poll::Ready(r) => return r,
            Poll::Pending => {
                #[allow(deprecated)]
                core::sync::atomic::spin_loop_hint();
            }
        }
    }
}

/// A waker that does nothing.
fn noop_waker() -> Waker {
    unsafe fn noop(_: *const ()) {}
    unsafe fn new_waker(_: *const ()) -> RawWaker {
        RawWaker::new(
            ptr::null(),
            &RawWakerVTable::new(new_waker, noop, noop, noop),
        )
    }

    unsafe { Waker::from_raw(new_waker(ptr::null())) }
}
