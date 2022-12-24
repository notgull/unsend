//! A multiple-producer multiple-consumer channel.

use crate::notify::{Listener, Notify};
use crate::queue::Queue;

use alloc::rc::Rc;

use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use pin_project_lite::pin_project;

/// Create a new MPMC channel with the given initial capacity.
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let channel = Rc::new(Channel {
        queue: Queue::new(capacity),
        senders: Cell::new(1),
        receivers: Cell::new(1),
        send_open: Notify::new(),
    });

    (
        Sender {
            channel: channel.clone(),
        },
        Receiver { channel },
    )
}

struct Channel<T> {
    /// The concurrent queue.
    queue: Queue<T>,

    /// The number of senders.
    senders: Cell<usize>,

    /// The number of receivers.
    receivers: Cell<usize>,

    /// The signal for when a sender is sending.
    ///
    /// We can pass the value through this signal to avoid the queue.
    send_open: Notify<Option<T>>,
}

/// The sender for the MPMC channel.
pub struct Sender<T> {
    /// The channel.
    channel: Rc<Channel<T>>,
}

impl<T> Sender<T> {
    /// Send a value to the channel.
    pub fn send(&self, value: T) -> Result<(), T> {
        // If the channel is closed, return the value.
        if self.channel.receivers.get() == 0 {
            return Err(value);
        }

        // If there is a receiver waiting, send the value through the signal.
        let mut value = Some(value);
        self.channel.send_open.notify(1, || value.take());

        if let Some(value) = value {
            // Otherwise, try to push the value into the queue.
            self.channel.queue.push(value);
        }

        Ok(())
    }

    /// Get the current capacity of the channel.
    pub fn capacity(&self) -> usize {
        self.channel.queue.capacity()
    }

    /// Get the current length of the channel.
    pub fn len(&self) -> usize {
        self.channel.queue.len()
    }

    /// Tell whether the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.channel.queue.is_empty()
    }

    /// Get the current sender count.
    pub fn senders(&self) -> usize {
        self.channel.senders.get()
    }

    /// Get the current receiver count.
    pub fn receivers(&self) -> usize {
        self.channel.receivers.get()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // Decrement the sender count.
        self.channel.senders.set(self.channel.senders.get() - 1);

        // If there are no more senders, close the channel.
        if self.channel.senders.get() == 0 {
            self.channel.send_open.notify(core::usize::MAX, || None);
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        // Increment the sender count.
        self.channel.senders.set(self.channel.senders.get() + 1);

        Self {
            channel: self.channel.clone(),
        }
    }
}

/// The receiver for the MPMC channel.
pub struct Receiver<T> {
    /// The channel.
    channel: Rc<Channel<T>>,
}

impl<T> Receiver<T> {
    /// Try to receive a value from the channel.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        // If the channel is closed, return an error.
        if self.channel.senders.get() == 0 {
            return Err(TryRecvError::Closed);
        }

        // Try to receive a value from the queue.
        match self.channel.queue.pop() {
            Some(value) => Ok(value),
            None => Err(TryRecvError::Empty),
        }
    }

    /// Receive a value from the channel.
    pub fn recv(&self) -> Recv<'_, T> {
        Recv {
            channel: self,
            listener: None,
        }
    }

    /// Get the current capacity of the channel.
    pub fn capacity(&self) -> usize {
        self.channel.queue.capacity()
    }

    /// Get the current length of the channel.
    pub fn len(&self) -> usize {
        self.channel.queue.len()
    }

    /// Tell whether the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.channel.queue.is_empty()
    }

    /// Get the current sender count.
    pub fn senders(&self) -> usize {
        self.channel.senders.get()
    }

    /// Get the current receiver count.
    pub fn receivers(&self) -> usize {
        self.channel.receivers.get()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        // Decrement the receiver count.
        self.channel.receivers.set(self.channel.receivers.get() - 1);

        // If there are no more receivers, close the channel.
        if self.channel.receivers.get() == 0 {
            self.channel.queue.clear();
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        // Increment the receiver count.
        self.channel.receivers.set(self.channel.receivers.get() + 1);

        Self {
            channel: self.channel.clone(),
        }
    }
}

pin_project! {
    /// A future for receiving a value from the channel.
    pub struct Recv<'a, T> {
        // The channel.
        channel: &'a Receiver<T>,

        // The listener for the receive signal.
        #[pin]
        listener: Option<Listener<'a, Option<T>>>,
    }
}

impl<'a, T> Future for Recv<'a, T> {
    type Output = Result<T, ChannelClosed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // If we're in the middle of receiving a value, poll that.
        if let Some(listener) = this.listener.as_mut().as_pin_mut() {
            // If the channel is closed, return an error.
            if this.channel.channel.senders.get() == 0 {
                return Poll::Ready(Err(ChannelClosed));
            }

            if let Some(value) = ready!(listener.poll(cx)) {
                return Poll::Ready(Ok(value));
            }
        }

        // Try to pop from the queue.
        if let Some(value) = this.channel.channel.queue.pop() {
            return Poll::Ready(Ok(value));
        }

        // If the channel is closed, return an error.
        if this.channel.channel.senders.get() == 0 {
            return Poll::Ready(Err(ChannelClosed));
        }

        // Otherwise, register the waker.
        this.listener
            .as_mut()
            .set(Some(this.channel.channel.send_open.listen()));
        Poll::Pending
    }
}

/// The error from a receive operation.
#[derive(Debug)]
pub struct ChannelClosed;

/// The error resulting from a receive operation.
#[derive(Debug)]
pub enum TryRecvError {
    /// The channel is closed.
    Closed,

    /// The channel is empty.
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::prelude::*;

    #[test]
    fn send_recv() {
        let (sender, receiver) = channel(1);

        sender.send(1).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), 1);
    }

    #[test]
    fn send_recv_many() {
        let (sender, receiver) = channel(1);

        futures_lite::future::block_on(async move {
            for i in 0..100 {
                sender.send(i).unwrap();
                assert_eq!(receiver.recv().await.unwrap(), i);
            }
        });
    }

    #[test]
    fn send_or_recv() {
        let (sender, receiver) = channel(1);

        futures_lite::future::block_on(async move {
            let recv_future = async move {
                let val = receiver.recv().await.unwrap();
                assert_eq!(val, 1);
            };

            let send_future = async move {
                sender.send(1).unwrap();
            };

            recv_future.or(send_future).await;
        });
    }

    #[test]
    fn recv_closed() {
        let (sender, receiver) = channel(1);

        futures_lite::future::block_on(async move {
            sender.send(1).unwrap();
            drop(sender);
            assert_eq!(receiver.recv().await.unwrap(), 1);
            assert!(receiver.recv().await.is_err());
        });
    }

    #[test]
    fn send_closed() {
        let (sender, receiver) = channel(1);

        futures_lite::future::block_on(async move {
            drop(receiver);
            assert!(sender.send(1).is_err());
        });
    }
}
