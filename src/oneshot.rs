//! A oneshot channel.

use alloc::rc::Rc;

use core::cell::{Cell, UnsafeCell};
use core::future::Future;
use core::marker::PhantomPinned;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::{self, NonNull};
use core::task::{Context, Poll, Waker};

struct Channel<T> {
    /// The inner value.
    value: Cell<Option<T>>,

    /// Has the sender been dropped?
    sender_dropped: Cell<bool>,

    /// The current state of the receiver.
    receiver_state: Cell<ReceiverState<T>>,
}

/// The current state of a receiver.
enum ReceiverState<T> {
    /// The receiver is waiting for a value.
    ///
    /// This is replaced with "Alive" when the sender is dropped, and "Dropped" when the receiver is
    /// ready.
    Waiting {
        /// A slot that can be used to "place" the value, in order to circumvent channel storage.
        slot: NonNull<Option<T>>,

        /// The waker that we need to wake up.
        waker: Waker,
    },

    /// The receiver is alive but not waiting.
    Alive,

    /// The receiver has been dropped.
    Dropped,
}

/// The sender of a oneshot channel.
pub struct Sender<T> {
    /// The channel.
    channel: Rc<Channel<T>>,
}

/// The receiver of a oneshot channel.
pub struct Receiver<T> {
    /// The channel.
    channel: Rc<Channel<T>>,

    /// The slot where we insert the value.
    ///
    /// This can be used to bypass the channel storage.
    slot: UnsafeCell<MaybeUninit<T>>,

    /// Whether or not we've registered the slot in the channel.
    registered: bool,

    /// This cannot be moved after being pinned.
    _pinned: PhantomPinned,
}

/// Create a new oneshot channel.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let channel = Rc::new(Channel {
        value: Cell::new(None),
        sender_dropped: Cell::new(false),
        receiver_state: Cell::new(ReceiverState::Alive),
    });

    let sender = Sender {
        channel: channel.clone(),
    };

    let receiver = Receiver {
        channel,
        slot: UnsafeCell::new(MaybeUninit::uninit()),
        registered: false,
        _pinned: PhantomPinned,
    };

    (sender, receiver)
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.channel.sender_dropped.set(true);

        // If there is a waiting receiver, wake it.
        if let ReceiverState::Waiting { waker, .. } =
            self.channel.receiver_state.replace(ReceiverState::Alive)
        {
            waker.wake();
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let state = self.channel.receiver_state.replace(ReceiverState::Dropped);

        if self.registered {
            // See if the slot needs to be cleared.
            if let ReceiverState::Dropped = state {
                unsafe {
                    ptr::drop_in_place(self.slot.get() as *mut T);
                }
            }
        }
    }
}

impl<T> Sender<T> {
    /// Send a value to the receiver.
    pub fn send(self, value: T) -> Result<(), T> {
        // See if there is a slot to place the value in.
        match self.channel.receiver_state.replace(ReceiverState::Dropped) {
            ReceiverState::Waiting { slot, waker } => {
                // There is a slot, so place the value there.
                unsafe {
                    ptr::write(slot.as_ptr(), Some(value));
                }

                // Wake up the receiver.
                waker.wake();

                Ok(())
            }
            ReceiverState::Alive => {
                // There is no slot, so place the value in the channel.
                self.channel.value.set(Some(value));

                // Receiver is still alive.
                self.channel.receiver_state.set(ReceiverState::Alive);

                Ok(())
            }
            ReceiverState::Dropped => {
                // The receiver has been dropped, so we can't send the value.
                Err(value)
            }
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let (channel, cell, registered) = unsafe {
            let this = self.get_unchecked_mut();
            (&this.channel, &this.slot, &mut this.registered)
        };

        // Check the state if we've been registered.
        if *registered {
            match channel.receiver_state.replace(ReceiverState::Alive) {
                ReceiverState::Waiting { slot, mut waker } => {
                    if !cx.waker().will_wake(&waker) {
                        waker = cx.waker().clone();
                    }

                    // Put it back in.
                    channel
                        .receiver_state
                        .set(ReceiverState::Waiting { slot, waker });
                }
                ReceiverState::Dropped => {
                    // We were notified.
                    *registered = false;
                    let value = unsafe { ptr::read(cell.get()) };
                    channel.receiver_state.set(ReceiverState::Dropped);
                    return Poll::Ready(Some(unsafe { value.assume_init() }));
                }
                ReceiverState::Alive => {
                    // The sender was dropped, just continue on.
                    channel.receiver_state.set(ReceiverState::Waiting {
                        slot: unsafe { NonNull::new_unchecked(cell.get().cast()) },
                        waker: cx.waker().clone(),
                    });
                }
            }
        }

        // See if there is a value in the channel.
        if let Some(value) = channel.value.take() {
            channel.receiver_state.set(ReceiverState::Dropped);
            return Poll::Ready(Some(value));
        }

        // See if the sender has been dropped.
        if channel.sender_dropped.get() {
            channel.receiver_state.set(ReceiverState::Dropped);
            return Poll::Ready(None);
        }

        // Register the slot.
        channel.receiver_state.set(ReceiverState::Waiting {
            slot: unsafe { NonNull::new_unchecked(cell.get().cast()) },
            waker: cx.waker().clone(),
        });
        *registered = true;
        Poll::Pending
    }
}

#[cfg(test)]
mod test {
    use super::channel;

    #[test]
    fn smoke() {
        futures_lite::future::block_on(async {
            let (sender, receiver) = channel::<i32>();

            // Send a value.
            sender.send(42).unwrap();

            // Receive the value.
            assert_eq!(receiver.await.unwrap(), 42);

            // Send and drop the receiver.
            let (sender, receiver) = channel::<i32>();
            drop(receiver);
            assert!(sender.send(36).is_err());

            // Send and drop the sender.
            let (sender, receiver) = channel::<i32>();
            drop(sender);
            assert_eq!(receiver.await, None);
        });
    }
}
