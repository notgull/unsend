//! Event handlers.

use core::cell::{Cell, RefCell, UnsafeCell};
use core::future::Future;
use core::marker::PhantomPinned;
use core::mem;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

use __private::NotificationSealed;

/// Wait for an event to occur.
pub struct Event<T>(RefCell<Inner<T>>);

impl<T> Event<T> {
    /// Create a new event.
    pub const fn new() -> Self {
        Self(RefCell::new(Inner {
            head: None,
            tail: None,
            first: None,
            notified: 0,
            len: 0,
        }))
    }

    /// Create a new listener for this event.
    #[cfg(feature = "alloc")]
    #[cold]
    pub fn listen(&self) -> Pin<alloc::boxed::Box<EventListener<'_, T>>> {
        let mut listener = alloc::boxed::Box::pin(EventListener::new(self));
        listener.as_mut().insert();
        listener
    }

    /// Notify this event.
    #[inline]
    pub fn notify(&self, notify: impl IntoNotification<Tag = T>) -> usize {
        let notify = notify.into_notification();
        let is_additional = notify.additional();
        let mut inner = self.0.borrow_mut();

        if is_additional {
            // If there are no events, return.
            if inner.len == 0 {
                return 0;
            }
        } else {
            // If there aren't enough events, return.
            if inner.notified > notify.count() {
                return 0;
            }
        }

        // Notify the event.
        inner.notify(notify)
    }
}

/// A listener for an event.
pub struct EventListener<'a, T> {
    /// The event that this listener is listening to.
    event: &'a Event<T>,

    /// Is this listener in the linked list?
    in_list: bool,

    /// The entry for this listener.
    entry: UnsafeCell<Entry<T>>,

    /// This listener should not be moved after being pinned.
    _pin: PhantomPinned,
}

impl<'a, T> EventListener<'a, T> {
    /// Create a new event listener.
    pub const fn new(event: &'a Event<T>) -> Self {
        Self {
            event,
            in_list: false,
            entry: UnsafeCell::new(Entry {
                next: Cell::new(None),
                prev: Cell::new(None),
                state: Cell::new(State::Created),
            }),
            _pin: PhantomPinned,
        }
    }

    /// Insert this listener into the linked list.
    #[cold]
    pub fn insert(self: Pin<&mut Self>) {
        let mut inner = self.event.0.borrow_mut();

        // SAFETY: We've locked the inner state, so we can safely access the entry.
        let entry = unsafe { &mut *self.entry.get() };
        *entry = Entry {
            next: Cell::new(None),
            prev: Cell::new(inner.tail),
            state: Cell::new(State::Created),
        };
        let entry = unsafe { &*self.entry.get() };

        // Set the next pointer of the previous entry.
        match mem::replace(&mut inner.tail, Some(entry.into())) {
            None => inner.head = Some(entry.into()),
            Some(t) => unsafe { t.as_ref().next.set(Some(entry.into())) },
        }

        // If there are no unnotified entries, this is the first one.
        if inner.first.is_none() {
            inner.first = inner.tail;
        }

        // Increment the number of entries.
        inner.len += 1;
        
        unsafe {
            self.get_unchecked_mut().in_list = true;
        }
    }

    /// Remove this listener from the linked list.
    fn remove(self: Pin<&mut Self>, propagate: bool) -> Option<T> {
        let mut inner = self.event.0.borrow_mut();

        // SAFETY: We've locked the inner state, so we can safely access the entry.
        let entry = unsafe { &*self.entry.get() };
        let prev = entry.prev.get();
        let next = entry.next.get();

        // Unlink from the previous entry.
        match prev {
            None => inner.head = next,
            Some(p) => unsafe {
                p.as_ref().next.set(next);
            },
        }

        // Unlink from the next entry.
        match next {
            None => inner.tail = prev,
            Some(n) => unsafe {
                n.as_ref().prev.set(prev);
            },
        }

        // If this was the first unnotified entry, update the next pointer.
        if inner.first == Some(entry.into()) {
            inner.first = next;
        }

        // Entry is now unlinked, so we can now take it out.
        let entry = mem::replace(
            unsafe { &mut *self.entry.get() },
            Entry {
                next: Cell::new(None),
                prev: Cell::new(None),
                state: Cell::new(State::Created),
            },
        )
        .state
        .into_inner();

        // Decrement the number of entries.
        inner.len -= 1;
        unsafe {
            self.get_unchecked_mut().in_list = false;
        }

        match entry {
            State::Notified(tag, additional) => {
                // If this entry was notified, decrement the number of notified entries.
                inner.notified -= 1;

                if propagate {
                    inner.notify(SingleNotify {
                        additional,
                        tag: Some(tag)
                    });

                    None
                } else {
                    Some(tag)
                }
            }

            _ => None,
        }
    }

    /// Registers this entry into the linked list.
    fn register(self: Pin<&mut Self>, waker: &Waker) -> RegisterResult<T> {
        let mut _inner = self.event.0.borrow_mut();

        // SAFETY: We've locked the inner state, so we can safely access the entry.
        let entry = unsafe { &*self.entry.get() };

        if !self.in_list {
            return RegisterResult::NotInserted;
        }

        // Take out the state and check it.
        match entry.state.replace(State::Created) {
            State::Notified(tag, additional) => {
                // We have been notified, remove the listener.
                entry.state.set(State::Notified(tag, additional));
                let tag = self.remove(false).unwrap();
                RegisterResult::Notified(tag)
            }

            State::Waiting(task) => {
                // Only replace the task if it's different.
                entry.state.set(State::Waiting({
                    if !task.will_wake(waker) {
                        waker.clone()
                    } else {
                        task
                    }
                }));

                RegisterResult::Registered
            }

            _ => {
                // We have not been notified, so we can register the task.
                entry.state.set(State::Waiting(waker.clone()));
                RegisterResult::Registered
            }
        }
    }
}

impl<T> Future for EventListener<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.register(cx.waker()) {
            RegisterResult::NotInserted => panic!("listener not inserted"),
            RegisterResult::Notified(tag) => Poll::Ready(tag),
            RegisterResult::Registered => Poll::Pending,
        }
    }
}

pub trait Notification: NotificationSealed {}

struct SingleNotify<T> {
    additional: bool,
    tag: Option<T>,
}

impl<T> NotificationSealed for SingleNotify<T> {
    type Tag = T;

    fn count(&self) -> usize {
        1
    }

    fn additional(&self) -> bool {
        self.additional
    }

    fn tag(&mut self) -> Self::Tag {
        self.tag.take().unwrap()
    }
}

impl<T> Notification for SingleNotify<T> {}

pub trait IntoNotification {
    type Tag;
    type Notify: Notification<Tag = Self::Tag>;

    fn into_notification(self) -> Self::Notify;
}

impl<N: Notification> IntoNotification for N {
    type Tag = N::Tag;
    type Notify = N;

    fn into_notification(self) -> Self::Notify {
        self
    }
}

struct Inner<T> {
    /// The head of the linked list of entries.
    head: Option<NonNull<Entry<T>>>,

    /// The tail of the linked list of entries.
    tail: Option<NonNull<Entry<T>>>,

    /// The first non-notified entry in the linked list.
    first: Option<NonNull<Entry<T>>>,

    /// Number of entries that are currently notified.
    notified: usize,

    /// Total number of entries.
    len: usize,
}

impl<T> Inner<T> {
    #[cold]
    fn notify(&mut self, mut notify: impl Notification<Tag = T>) -> usize {
        let is_additional = notify.additional();
        let mut count = notify.count();

        if !is_additional {
            // Make sure we're not notifiying more than we have.
            if count <= self.notified {
                return 0;
            }
            count -= self.notified;
        }

        let mut notified = 0;
        while count > 0 {
            count -= 1;

            // Notify the first entry.
            match self.first {
                None => break,

                Some(e) => {
                    // Get the entry.
                    let entry = unsafe { e.as_ref() };
                    self.first = entry.next.get();
                    notified += 1;

                    // Set state to `Notified` and notify.
                    if let State::Waiting(wake) = entry
                        .state
                        .replace(State::Notified(notify.tag(), is_additional))
                    {
                        wake.wake();
                    }

                    // Bump the notified count.
                    self.notified += 1;
                }
            }
        }
        notified
    }
}

struct Entry<T> {
    /// Pointer to the next entry in the linked list.
    next: Cell<Option<NonNull<Entry<T>>>>,

    /// Pointer to the previous entry in the linked list.
    prev: Cell<Option<NonNull<Entry<T>>>>,

    /// The state of this entry.
    state: Cell<State<T>>,
}

enum State<T> {
    /// The entry was just created.
    Created,

    /// The entry is waiting for an event.
    Waiting(Waker),

    /// The entry has been notified with this tag.
    Notified(T, bool),
}

enum RegisterResult<T> {
    NotInserted,
    Registered,
    Notified(T)
}

#[doc(hidden)]
pub struct Notify(usize);

impl NotificationSealed for Notify {
    type Tag = ();

    fn additional(&self) -> bool {
        false
    }

    fn count(&self) -> usize {
        self.0
    }

    fn tag(&mut self) -> Self::Tag {
    }
}
impl Notification for Notify {}

/// Make a notification use additional notifications.
#[doc(hidden)]
pub struct Additional<N>(N);

impl<N: Notification> NotificationSealed for Additional<N> {
    type Tag = N::Tag;

    fn additional(&self) -> bool {
        true
    }

    fn count(&self) -> usize {
        self.0.count()
    }

    fn tag(&mut self) -> Self::Tag {
        self.0.tag()
    }
}
impl<N: Notification> Notification for Additional<N> {}

/// Notification that uses a tag.
#[doc(hidden)]
pub struct Tag<N, T> {
    inner: N,
    tag: T,
}

impl<N: Notification, T: Clone> NotificationSealed for Tag<N, T> {
    type Tag = T;

    fn additional(&self) -> bool {
        self.inner.additional()
    }

    fn count(&self) -> usize {
        self.inner.count()
    }

    fn tag(&mut self) -> Self::Tag {
        self.tag.clone()
    }
}
impl<N: Notification, T: Clone> Notification for Tag<N, T> {}

/// Notification that uses a tagging function.
#[doc(hidden)]
pub struct TagWith<N, F> {
    inner: N,
    tag_fn: F,
}

impl<N: Notification, F: FnMut() -> T, T: Clone> NotificationSealed for TagWith<N, F> {
    type Tag = T;

    fn additional(&self) -> bool {
        self.inner.additional()
    }

    fn count(&self) -> usize {
        self.inner.count()
    }

    fn tag(&mut self) -> Self::Tag {
        (self.tag_fn)()
    }
}
impl<N: Notification, F: FnMut() -> T, T: Clone> Notification for TagWith<N, F> {}

mod __private {
    #[doc(hidden)]
    pub trait NotificationSealed {
        type Tag;

        fn count(&self) -> usize;
        fn additional(&self) -> bool;
        fn tag(&mut self) -> Self::Tag;
    }
}
