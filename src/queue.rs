//! A concurrent queue.

use alloc::collections::VecDeque;
use core::cell::RefCell;

/// A concurrent queue.
pub(crate) struct Queue<T> {
    // TODO: Use a more efficient algorithm.
    queue: RefCell<VecDeque<T>>,
}

impl<T> Queue<T> {
    /// Creates a new queue with the specified capacity.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            queue: RefCell::new(VecDeque::with_capacity(capacity)),
        }
    }

    /// The capacity of the queue.
    pub(crate) fn capacity(&self) -> usize {
        self.queue.borrow().capacity()
    }

    /// The length of the queue.
    pub(crate) fn len(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Is the queue empty?
    pub(crate) fn is_empty(&self) -> bool {
        self.queue.borrow().is_empty()
    }

    /// Pushes an item into the queue.
    pub(crate) fn push(&self, item: T) {
        self.queue.borrow_mut().push_back(item);
    }

    /// Pops an item from the queue.
    pub(crate) fn pop(&self) -> Option<T> {
        self.queue.borrow_mut().pop_front()
    }

    /// Clear the queue.
    pub(crate) fn clear(&self) {
        self.queue.borrow_mut().clear();
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new(0)
    }
}
