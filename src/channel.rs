// SPDX-License-Identifier: LGPL-3.0-or-later OR MPL-2.0
// This file is a part of `unsend`.
//
// `unsend` is free software: you can redistribute it and/or modify it under the
// terms of either:
//
// * GNU Lesser General Public License as published by the Free Software Foundation, either
//   version 3 of the License, or (at your option) any later version.
// * Mozilla Public License as published by the Mozilla Foundation, version 2.
//
// `unsend` is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
// PURPOSE. See the GNU Lesser General Public License or the Mozilla Public License for more
// details.
//
// You should have received a copy of the GNU Lesser General Public License and the Mozilla
// Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>.

//! An MPMC channel.

use crate::{Event, EventListener, IntoNotification};

use alloc::collections::VecDeque;
use alloc::rc::Rc;

use core::cell::{Cell, RefCell};
use core::fmt;
use core::pin::Pin;

struct Channel<T> {
    /// The underlying data.
    data: RefCell<VecDeque<T>>,

    /// Is the channel closed?
    closed: Cell<bool>,

    /// The number of senders.
    senders: Cell<usize>,

    /// The number of receivers.
    receivers: Cell<usize>,

    /// The event for waiting for new items.
    event: Event<Option<T>>,
}

/// A sender for an MPMC channel.
pub struct Sender<T> {
    /// The origin channel.
    channel: Rc<Channel<T>>,
}

/// A receiver for an MPMC channel.
pub struct Receiver<T> {
    /// The origin channel.
    channel: Rc<Channel<T>>,
}

/// Create a new MPMC channel.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let channel = Rc::new(Channel {
        data: RefCell::new(VecDeque::new()),
        senders: Cell::new(1),
        receivers: Cell::new(1),
        closed: Cell::new(false),
        event: Event::new(),
    });

    (
        Sender {
            channel: channel.clone(),
        },
        Receiver { channel },
    )
}

impl<T> Sender<T> {
    /// Send an item.
    pub fn send(&self, item: T) -> Result<(), ChannelClosed> {
        if self.channel.closed.get() {
            return Err(ChannelClosed { _private: () });
        }

        let mut item = Some(item);

        // Try to send the event directly.
        self.channel.event.notify(1.tag_with(|| item.take()));

        // If the event was not sent, push the item to the queue.
        if let Some(item) = item {
            self.channel.data.borrow_mut().push_back(item);
        }

        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        let new_senders = self.channel.senders.get() + 1;
        self.channel.senders.set(new_senders);

        Sender {
            channel: self.channel.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let new_senders = self.channel.senders.get() - 1;
        self.channel.senders.set(new_senders);

        if new_senders == 0 {
            self.channel.closed.set(true);
            self.channel
                .event
                .notify(core::usize::MAX.tag_with(|| None));
        }
    }
}

impl<T> Receiver<T> {
    /// Try to receive an item.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        // Try to receive an item from the queue.
        self.channel.data.borrow_mut().pop_front().ok_or_else(|| {
            if self.channel.closed.get() {
                TryRecvError::Closed
            } else {
                TryRecvError::Empty
            }
        })
    }

    /// Wait for a new item.
    pub async fn recv(&self) -> Result<T, ChannelClosed> {
        let mut listener = EventListener::new(&self.channel.event);

        {
            let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

            loop {
                // Wait for a new item.
                if let Some(item) = self.channel.data.borrow_mut().pop_front() {
                    return Ok(item);
                }

                if self.channel.closed.get() {
                    return Err(ChannelClosed { _private: () });
                }

                // Use the listener.
                if let Some(item) = listener.as_mut().await {
                    return Ok(item);
                }
            }
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Receiver<T> {
        let new_receivers = self.channel.receivers.get() + 1;
        self.channel.receivers.set(new_receivers);

        Receiver {
            channel: self.channel.clone(),
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let new_receivers = self.channel.receivers.get() - 1;
        self.channel.receivers.set(new_receivers);

        if new_receivers == 0 {
            self.channel.closed.set(true);
            self.channel
                .event
                .notify(core::usize::MAX.tag_with(|| None));
        }
    }
}

/// The channel has been closed.
#[derive(Debug)]
pub struct ChannelClosed {
    _private: (),
}

impl fmt::Display for ChannelClosed {
    #[cfg_attr(coverage, no_coverage)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "channel closed")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ChannelClosed {}

/// The `try_recv` operation failed.
#[derive(Debug)]
pub enum TryRecvError {
    /// The channel has been closed.
    Closed,

    /// The channel is empty.
    Empty,
}

impl fmt::Display for TryRecvError {
    #[cfg_attr(coverage, no_coverage)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TryRecvError::Closed => write!(f, "channel closed"),
            TryRecvError::Empty => write!(f, "channel empty"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TryRecvError {
    #[cfg_attr(coverage, no_coverage)]
    fn cause(&self) -> Option<&dyn std::error::Error> {
        match self {
            TryRecvError::Closed => Some(&ChannelClosed { _private: () }),
            TryRecvError::Empty => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future;

    #[test]
    fn test_channel() {
        future::block_on(async {
            let (sender, receiver) = channel();

            sender.send(1).unwrap();
            sender.send(2).unwrap();
            sender.send(3).unwrap();

            assert_eq!(receiver.try_recv().unwrap(), 1);
            assert_eq!(receiver.recv().await.unwrap(), 2);
            assert_eq!(receiver.try_recv().unwrap(), 3);
            assert!(receiver.try_recv().is_err());

            drop(sender);

            assert!(receiver.recv().await.is_err());
        });
    }

    #[test]
    fn test_channel_clone() {
        future::block_on(async {
            let (sender, receiver) = channel();

            let sender2 = sender.clone();

            sender.send(1).unwrap();
            sender2.send(2).unwrap();

            assert_eq!(receiver.try_recv().unwrap(), 1);
            assert_eq!(receiver.try_recv().unwrap(), 2);
            assert!(receiver.try_recv().is_err());

            drop(sender);
            drop(sender2);

            assert!(receiver.recv().await.is_err());
        });
    }

    #[test]
    fn test_channel_recv_clone() {
        future::block_on(async {
            let (sender, receiver) = channel();

            let receiver2 = receiver.clone();

            sender.send(1).unwrap();
            sender.send(2).unwrap();

            assert_eq!(receiver.try_recv().unwrap(), 1);
            assert_eq!(receiver2.try_recv().unwrap(), 2);
            assert!(receiver.try_recv().is_err());
            assert!(receiver2.try_recv().is_err());

            drop((receiver, receiver2));

            assert!(sender.send(3).is_err());
        });
    }

    #[test]
    fn test_send_direct() {
        future::block_on(async {
            let (sender, receiver) = channel();

            // Start receiving.
            let recv = receiver.recv();
            futures_lite::pin!(recv);

            // Poll once.
            assert!(future::poll_once(&mut recv).await.is_none());

            // Send an item.
            sender.send(1).unwrap();

            // Poll again.
            assert_eq!(future::poll_once(&mut recv).await.unwrap().ok(), Some(1));
        });
    }

    #[test]
    fn test_recv_and_drop() {
        future::block_on(async {
            let (sender, receiver) = channel::<i32>();

            // Start receiving.
            let recv = receiver.recv();
            futures_lite::pin!(recv);
            let receiver2 = receiver.clone();
            let recv2 = receiver2.recv();
            futures_lite::pin!(recv2);

            // Poll once.
            assert!(future::poll_once(&mut recv).await.is_none());
            assert!(future::poll_once(&mut recv2).await.is_none());

            // Drop the sender.
            drop(sender);

            // Poll again.
            assert!(recv.await.is_err());
            assert!(recv2.await.is_err());
        });
    }

    #[test]
    fn test_channel_drop() {
        future::block_on(async {
            let (sender, receiver) = channel();

            sender.send(1).unwrap();
            sender.send(2).unwrap();
            sender.send(3).unwrap();

            drop(sender);

            assert_eq!(receiver.try_recv().unwrap(), 1);
            assert_eq!(receiver.try_recv().unwrap(), 2);
            assert_eq!(receiver.try_recv().unwrap(), 3);
            assert!(receiver.try_recv().is_err());

            assert!(receiver.recv().await.is_err());
        });
    }
}
