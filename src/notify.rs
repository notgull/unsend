//! Thread-unsafe notification mechanism.

use core::cell::{Cell, RefCell, UnsafeCell};
use core::fmt;
use core::future::Future;
use core::marker::PhantomPinned;
use core::mem;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

/// A thread-unsafe notification mechanism, similar to `Event` from `event-listener`.
#[derive(Debug)]
pub(crate) struct Notify<T>(RefCell<List<T>>);

/// Waiting on a `Notify`.
#[derive(Debug)]
pub(crate) struct Listener<'a, T> {
    /// The `Notify` being waited on.
    notify: &'a Notify<T>,

    /// The entry in the linked list.
    ///
    /// This is `None` when this entry has not been registered.
    entry: Option<UnsafeCell<Entry<T>>>,

    /// This is a `!Unpin` type.
    _pinned: PhantomPinned,
}

impl<T> Notify<T> {
    /// Create a new `Notify`.
    pub(crate) const fn new() -> Self {
        Self(RefCell::new(List {
            len: 0,
            notified: 0,
            head: None,
            tail: None,
            start: None,
        }))
    }

    /// Create a new `Listener`.
    pub(crate) fn listen(&self) -> Listener<'_, T> {
        Listener {
            notify: self,
            entry: None,
            _pinned: PhantomPinned,
        }
    }

    /// Notify `n` previously registered listeners.
    #[inline]
    pub(crate) fn notify(&self, mut count: usize, factory: impl FnMut() -> T) -> usize {
        let mut guard = self.0.borrow_mut();

        // Only notify if there are any entries to notify.
        if guard.len <= guard.notified {
            return 0;
        }

        count -= guard.notified;

        // Notify the first `count` entries.
        guard.notify(count, factory, false)
    }

    /// Notify `n` registered listeners.
    #[inline]
    pub(crate) fn notify_additional(&self, count: usize, factory: impl FnMut() -> T) -> usize {
        let mut guard = self.0.borrow_mut();

        // Only notify if there are any entries to notify.
        if guard.len <= guard.notified {
            return 0;
        }

        // Notify the first `count` entries.
        guard.notify(count, factory, true)
    }
}

impl<T> Default for Notify<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Listener<'_, T> {
    /// Pin projection.
    #[allow(clippy::type_complexity)]
    fn project(self: Pin<&mut Self>) -> (&Notify<T>, Pin<&mut Option<UnsafeCell<Entry<T>>>>) {
        // SAFETY: This is a valid pin projection.
        unsafe {
            let this = self.get_unchecked_mut();
            (this.notify, Pin::new_unchecked(&mut this.entry))
        }
    }

    /// Remove the entry from the linked list.
    unsafe fn remove_entry(self: Pin<&mut Self>) -> Option<State<T>> {
        // SAFETY: This is a valid pin projection.
        let (notify, mut entry) = self.project();

        // Borrow a guard to the linked list.
        let mut guard = notify.0.borrow_mut();

        let entry_ref = match entry.as_mut().as_pin_mut() {
            Some(entry) => entry,
            None => return None,
        };

        // Get a pointer to the entry.
        let entry_ptr = unsafe { NonNull::new_unchecked(entry_ref.get_unchecked_mut().get()) };

        // Remove the entry from the linked list.
        guard.remove(entry_ptr);

        // Now that it's removed we can safely take it out.
        unsafe {
            entry
                .get_unchecked_mut()
                .take()
                .map(|t| t.into_inner().state.into_inner())
        }
    }
}

impl<T> Future for Listener<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (notify, mut entry) = self.project();

        // Borrow a guard to the linked list.
        let mut guard = notify.0.borrow_mut();

        // If we aren't inserted, return.
        let mut entry_ptr = match entry.as_mut().as_pin_mut() {
            Some(entry) => unsafe { NonNull::new_unchecked(entry.get_unchecked_mut().get()) },
            None => {
                // Create a new entry.
                entry.as_mut().set(Some(UnsafeCell::new(Entry {
                    state: Cell::new(State::Initialized),
                    prev: Cell::new(guard.tail),
                    next: Cell::new(None),
                })));

                // Get a pointer to this new entry.
                let entry = unsafe {
                    NonNull::new_unchecked(
                        entry
                            .as_mut()
                            .as_pin_mut()
                            .unwrap()
                            .get_unchecked_mut()
                            .get(),
                    )
                };

                // Add this entry to the linked list.
                match mem::replace(&mut guard.tail, Some(entry)) {
                    Some(mut old_tail) => {
                        // SAFETY: The entry is valid.
                        let old_tail = unsafe { old_tail.as_mut() };
                        old_tail.next.set(Some(entry));
                    }

                    None => guard.head = Some(entry),
                }

                // If we don't have a non-notified entry, set this entry as the non-notified entry.
                if guard.start.is_none() {
                    guard.start = Some(entry);
                }

                guard.len += 1;

                entry
            }
        };

        // Since we have the guard, we can freely insert the state into the etnry.
        let entry_ref = unsafe { entry_ptr.as_mut() };

        match entry_ref.state.replace(State::Hole) {
            State::Initialized => {
                // If the entry is initialized, we need to register the waker.
                entry_ref.state.set(State::Waiting(cx.waker().clone()));
                Poll::Pending
            }
            State::Waiting(waker) => {
                if cx.waker().will_wake(&waker) {
                    // If the waker is the same, we don't need to do anything.
                    entry_ref.state.set(State::Waiting(waker));
                    Poll::Pending
                } else {
                    // If the waker is different, we need to update it.
                    entry_ref.state.set(State::Waiting(cx.waker().clone()));
                    Poll::Pending
                }
            }
            State::Notified { result, .. } => {
                // If the entry is notified, we can return the result.
                Poll::Ready(result)
            }
            State::Hole => panic!("listener in hole state"),
        }
    }
}

impl<T> Drop for Listener<'_, T> {
    fn drop(&mut self) {
        unsafe {
            let notify = self.notify;
            let state = Pin::new_unchecked(self).remove_entry();

            if let Some(State::Notified { result, additional }) = state {
                let mut result = Some(result);
                if additional {
                    notify.notify_additional(1, move || result.take().unwrap());
                } else {
                    notify.notify(1, move || result.take().unwrap());
                }
            }
        }
    }
}

#[derive(Debug)]
struct List<T> {
    /// The number of entries in the linked list.
    len: usize,

    /// The number of notified entries in the linked list.
    notified: usize,

    /// The head of the linked list.
    head: Option<NonNull<Entry<T>>>,

    /// The tail of the linked list.
    tail: Option<NonNull<Entry<T>>>,

    /// The first non-notified entry in the linked list.
    start: Option<NonNull<Entry<T>>>,
}

impl<T> List<T> {
    /// Remove an entry from the list.
    unsafe fn remove(&mut self, mut entry_ptr: NonNull<Entry<T>>) {
        std::println!("remove");

        // SAFETY: The entry is valid.
        let entry = unsafe { entry_ptr.as_mut() };

        // Get the previous and next entries.
        let prev = entry.prev.get();
        let next = entry.next.get();

        // Update the previous entry.
        if let Some(mut prev) = prev {
            // SAFETY: The entry is valid.
            let prev = unsafe { prev.as_mut() };
            prev.next.set(next);
        } else {
            self.head = next;
        }

        // Update the next entry.
        if let Some(mut next) = next {
            // SAFETY: The entry is valid.
            let next = unsafe { next.as_mut() };
            next.prev.set(prev);
        } else {
            self.tail = prev;
        }

        if self.start == Some(entry_ptr) {
            self.start = next;
        }

        self.len -= 1;
        if entry.state.replace(State::Initialized).is_notified() {
            self.notified -= 1;
        }
    }

    /// Notify `n` entries in the linked list.
    #[cold]
    fn notify(&mut self, mut n: usize, mut factory: impl FnMut() -> T, additional: bool) -> usize {
        let mut current = self.start;
        let mut notified = 0;

        while n > 0 {
            // Get the current entry.
            let cursor = match current {
                Some(mut current) => unsafe { current.as_mut() },
                None => break,
            };

            // Notify this entry.
            let new_state = State::Notified {
                result: factory(),
                additional,
            };

            if let State::Waiting(waker) = cursor.state.replace(new_state) {
                waker.wake();
            }

            notified += 1;
            n -= 1;
            current = cursor.next.get();
        }

        self.notified += notified;
        self.start = current;

        notified
    }
}

/// An entry in the linked list of listeners.
struct Entry<T> {
    /// The previous entry in the list.
    prev: Cell<Option<NonNull<Entry<T>>>>,

    /// The next entry in the list.
    next: Cell<Option<NonNull<Entry<T>>>>,

    /// The current state of the list.
    state: Cell<State<T>>,
}

impl<T> fmt::Debug for Entry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Entry { .. }")
    }
}

/// The state of a linked list entry.
enum State<T> {
    /// The entry has just been created.
    Initialized,

    /// The entry is waiting on this waker.
    Waiting(Waker),

    /// The entry has been notified.
    Notified {
        /// The result of the notification.
        result: T,

        /// Whether the notification was an additional notification.
        additional: bool,
    },

    /// Empty hole.
    Hole,
}

impl<T> State<T> {
    #[allow(clippy::match_like_matches_macro)]
    fn is_notified(&self) -> bool {
        match self {
            State::Notified { .. } | State::Hole => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Notify;
    use core::task::{Context, Poll};
    use futures_lite::future::{self, Future};

    #[test]
    fn smoke() {
        future::block_on(async {
            let waker = future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
            let mut cx = Context::from_waker(&waker);

            let notify = Notify::new();

            {
                let listener = notify.listen();
                futures_lite::pin!(listener);

                assert!(listener.as_mut().poll(&mut cx).is_pending());
                notify.notify(1, || 42);
                assert_eq!(listener.poll(&mut cx), Poll::Ready(42));
            }
        });
    }
}
