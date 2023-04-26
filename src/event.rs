// SPDX-License-Identifier: LGPL-3.0-or-later OR MPL-2.0
// This file is a part of `unsend`.
//
// `unsend` is free software: you can redistribute it and/or modify it under the
// terms of either:
//
// * GNU Lesser General Public License as published by the Free Software Foundation, either
//   version 3 of the License, or (at your option) any later version.
// * Mozilla Public License as published by the Mozilla Foundation, version 2.
// * The Patron License (https://github.com/notgull/unsend/blob/main/LICENSE-PATRON.md)
//   for sponsors and contributors, who can ignore the copyleft provisions of the above licenses
//   for this project.
//
// `unsend` is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
// PURPOSE. See the GNU Lesser General Public License or the Mozilla Public License for more
// details.
//
// You should have received a copy of the GNU Lesser General Public License and the Mozilla
// Public License along with `unsend`. If not, see <https://www.gnu.org/licenses/>.

//! Event handlers.

use core::borrow::Borrow;
use core::cell::{Cell, RefCell, UnsafeCell};
use core::future::Future;
use core::marker::PhantomPinned;
use core::mem;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

#[cfg(feature = "alloc")]
use alloc::rc::Rc;

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
        alloc::boxed::Box::pin(EventListener::new(self))
    }

    /// Notify this event.
    #[inline]
    pub fn notify(&self, notify: impl IntoNotification<Tag = T>) -> usize {
        let notify = notify.into_notification();
        let is_additional = notify.is_additional();
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

pin_project_lite::pin_project! {
    /// A listener for an event.
    pub struct EventListener<'a, T> {
        #[pin]
        listener: Listener<T, &'a Event<T>>,
    }
}

impl<'a, T> EventListener<'a, T> {
    /// Create a new event listener.
    #[inline]
    pub const fn new(event: &'a Event<T>) -> Self {
        Self {
            listener: Listener::new(event),
        }
    }
}

impl<T> Future for EventListener<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().listener.poll(cx)
    }
}

pin_project_lite::pin_project! {
    /// A listener for an event over an `Rc`.
    #[cfg(feature = "alloc")]
    pub struct EventListenerRc<T> {
        #[pin]
        listener: Listener<T, Rc<Event<T>>>,
    }
}

#[cfg(feature = "alloc")]
impl<T> EventListenerRc<T> {
    /// Create a new event listener.
    #[inline]
    pub fn new(event: Rc<Event<T>>) -> Self {
        Self {
            listener: Listener::new(event),
        }
    }
}

#[cfg(feature = "alloc")]
impl<T> Future for EventListenerRc<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().listener.poll(cx)
    }
}

/// A listener for an event.
struct Listener<T, B: Borrow<Event<T>> + Clone> {
    /// The event that this listener is listening to.
    event: B,

    /// Is this listener in the linked list?
    in_list: bool,

    /// The entry for this listener.
    entry: UnsafeCell<Entry<T>>,

    /// This listener should not be moved after being pinned.
    _pin: PhantomPinned,
}

impl<T, B: Borrow<Event<T>> + Clone> Listener<T, B> {
    /// Create a new event listener.
    const fn new(event: B) -> Self {
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
    fn insert(self: Pin<&mut Self>) {
        let evt = self.event.clone();
        let mut inner = evt.borrow().0.borrow_mut();

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
        let evt = self.event.clone();
        let mut inner = evt.borrow().0.borrow_mut();

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
                        tag: Some(tag),
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
    /// 
    /// # Safety
    /// 
    /// We must be inserted into the linked list.
    unsafe fn register(self: Pin<&mut Self>, waker: &Waker) -> RegisterResult<T> {
        let inner = self.event.borrow().0.borrow_mut();

        // SAFETY: We've locked the inner state, so we can safely access the entry.
        let entry = unsafe { &*self.entry.get() };

        // Take out the state and check it.
        match entry.state.replace(State::Created) {
            State::Notified(tag, additional) => {
                // We have been notified, remove the listener.
                entry.state.set(State::Notified(tag, additional));
                drop(inner);
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

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if !self.in_list {
            self.as_mut().insert();
        }

        // SAFETY: We are in the list.
        match unsafe { self.register(cx.waker()) } {
            RegisterResult::Notified(tag) => Poll::Ready(tag),
            RegisterResult::Registered => Poll::Pending,
        }
    }
}

impl<T, B: Borrow<Event<T>> + Clone> Drop for Listener<T, B> {
    fn drop(&mut self) {
        if self.in_list {
            unsafe {
                Pin::new_unchecked(self).remove(true);
            }
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

    fn is_additional(&self) -> bool {
        self.additional
    }

    fn next_tag(&mut self) -> Self::Tag {
        self.tag.take().unwrap()
    }
}

impl<T> Notification for SingleNotify<T> {}

pub trait IntoNotification: Sized {
    type Tag;
    type Notify: Notification<Tag = Self::Tag>;

    fn into_notification(self) -> Self::Notify;
    fn additional(self) -> Additional<Self::Notify> {
        Additional(self.into_notification())
    }
    fn tag<T: Clone>(self, tag: T) -> Tag<Self::Notify, T> {
        Tag {
            inner: self.into_notification(),
            tag,
        }
    }
    fn tag_with<F, T>(self, f: F) -> TagWith<Self::Notify, F>
    where
        F: FnMut() -> T,
    {
        TagWith {
            inner: self.into_notification(),
            tag_fn: f,
        }
    }
}

macro_rules! notify_int {
    ($($ty:ty)*) => {$(
        impl IntoNotification for $ty {
            type Tag = ();
            type Notify = Notify;

            #[allow(unused_comparisons)]
            fn into_notification(self) -> Self::Notify {
                if self < 0 {
                    panic!("negative notification count");
                }

                Notify(self as usize)
            }
        }
    )*};
}

notify_int! {
    u8 u16 u32 u64 u128 usize
    i8 i16 i32 i64 i128 isize
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
        let is_additional = notify.is_additional();
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
                        .replace(State::Notified(notify.next_tag(), is_additional))
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
    Registered,
    Notified(T),
}

#[doc(hidden)]
pub struct Notify(usize);

impl NotificationSealed for Notify {
    type Tag = ();

    fn is_additional(&self) -> bool {
        false
    }

    fn count(&self) -> usize {
        self.0
    }

    fn next_tag(&mut self) -> Self::Tag {}
}
impl Notification for Notify {}

/// Make a notification use additional notifications.
#[doc(hidden)]
pub struct Additional<N>(N);

impl<N: Notification> NotificationSealed for Additional<N> {
    type Tag = N::Tag;

    fn is_additional(&self) -> bool {
        true
    }

    fn count(&self) -> usize {
        self.0.count()
    }

    fn next_tag(&mut self) -> Self::Tag {
        self.0.next_tag()
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

    fn is_additional(&self) -> bool {
        self.inner.is_additional()
    }

    fn count(&self) -> usize {
        self.inner.count()
    }

    fn next_tag(&mut self) -> Self::Tag {
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

impl<N: Notification, F: FnMut() -> T, T> NotificationSealed for TagWith<N, F> {
    type Tag = T;

    fn is_additional(&self) -> bool {
        self.inner.is_additional()
    }

    fn count(&self) -> usize {
        self.inner.count()
    }

    fn next_tag(&mut self) -> Self::Tag {
        (self.tag_fn)()
    }
}
impl<N: Notification, F: FnMut() -> T, T> Notification for TagWith<N, F> {}

mod __private {
    #[doc(hidden)]
    pub trait NotificationSealed {
        type Tag;

        fn count(&self) -> usize;
        fn is_additional(&self) -> bool;
        fn next_tag(&mut self) -> Self::Tag;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future;
    use waker_fn::waker_fn;

    use std::sync::{Arc, Mutex};
    use std::task::{Context, Wake, Waker};

    fn is_notified(listener: Pin<&mut EventListener<'_, ()>>) -> bool {
        future::block_on(future::poll_once(listener)).is_some()
    }

    fn is_notified_rc(listener: Pin<&mut EventListenerRc<()>>) -> bool {
        future::block_on(future::poll_once(listener)).is_some()
    }

    struct ListWaker {
        notified: Arc<Mutex<Vec<usize>>>,
        index: usize,
    }

    impl Wake for ListWaker {
        fn wake(self: Arc<Self>) {
            self.notified.lock().unwrap().push(self.index);
        }
    }

    #[test]
    fn notify() {
        let event = Event::<()>::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();
        let mut l3 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));

        event.notify(2);
        event.notify(1);

        assert!(is_notified(l1.as_mut()));
        assert!(is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));
    }

    #[test]
    fn notify_additional() {
        let event = Event::<()>::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();
        let mut l3 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));

        event.notify(1.additional());
        event.notify(1);
        event.notify(1.additional());

        assert!(is_notified(l1.as_mut()));
        assert!(is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn notify_rc() {
        let event = Rc::new(Event::<()>::new());

        let l1 = EventListenerRc::new(event.clone());
        let l2 = EventListenerRc::new(event.clone());

        futures_lite::pin!(l1);
        futures_lite::pin!(l2);

        assert!(!is_notified_rc(l1.as_mut()));
        assert!(!is_notified_rc(l2.as_mut()));

        event.notify(1);
        event.notify(1);

        assert!(is_notified_rc(l1.as_mut()));
        assert!(!is_notified_rc(l2.as_mut()));

        event.notify(1);

        assert!(is_notified_rc(l2.as_mut()));
    }

    #[test]
    fn notify_out_of_range() {
        let event = Event::<()>::new();

        assert_eq!(event.notify(1.additional()), 0);

        let mut l1 = event.listen();
        let mut l2 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));

        assert_eq!(event.notify(2), 2);
        assert_eq!(event.notify(1), 0);
    }

    #[test]
    fn change_waker() {
        let v = Arc::new(Mutex::new(0));

        let waker1 = waker_fn::waker_fn({
            let v = v.clone();
            move || *v.lock().unwrap() = 1
        });
        let waker2 = waker_fn::waker_fn({
            let v = v.clone();
            move || *v.lock().unwrap() = 2
        });

        let event = Event::<()>::new();

        let mut l1 = event.listen();

        assert!(l1
            .as_mut()
            .poll(&mut Context::from_waker(&waker1))
            .is_pending(),);

        // Change the waker.
        assert!(l1
            .as_mut()
            .poll(&mut Context::from_waker(&waker2))
            .is_pending(),);

        // Notify the event.
        event.notify(1);

        // The waker should be called.
        assert_eq!(*v.lock().unwrap(), 2);
    }

    #[test]
    fn notify_one() {
        let event = Event::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));

        event.notify(1);
        assert!(is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));

        event.notify(1);
        assert!(is_notified(l2.as_mut()));
    }

    #[test]
    fn notify_all() {
        let event = Event::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));

        event.notify(core::usize::MAX);
        assert!(is_notified(l1.as_mut()));
        assert!(is_notified(l2.as_mut()));
    }

    #[test]
    fn drop_notified() {
        let event = Event::<()>::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();
        let mut l3 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));

        event.notify(1);
        drop(l1);

        assert!(is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));
    }

    #[test]
    fn drop_notified2() {
        let event = Event::<()>::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();
        let mut l3 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));

        event.notify(2);
        drop(l1);

        assert!(is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));
    }

    #[test]
    fn drop_notified_additional() {
        let event = Event::<()>::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();
        let mut l3 = event.listen();
        let mut l4 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));
        assert!(!is_notified(l4.as_mut()));

        event.notify(1.additional());
        event.notify(2);

        drop(l1);

        assert!(is_notified(l2.as_mut()));
        assert!(is_notified(l3.as_mut()));
        assert!(!is_notified(l4.as_mut()));
    }

    #[test]
    fn drop_non_notified() {
        let event = Event::<()>::new();

        let mut l1 = event.listen();
        let mut l2 = event.listen();
        let mut l3 = event.listen();

        assert!(!is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));
        assert!(!is_notified(l3.as_mut()));

        event.notify(1);
        drop(l3);
        assert!(is_notified(l1.as_mut()));
        assert!(!is_notified(l2.as_mut()));
    }

    #[test]
    fn notify_all_fair() {
        let event = Event::<()>::new();
        let v = Arc::new(Mutex::new(vec![]));

        let waker1 = Waker::from(Arc::new(ListWaker {
            notified: v.clone(),
            index: 1,
        }));
        let waker2 = Waker::from(Arc::new(ListWaker {
            notified: v.clone(),
            index: 2,
        }));
        let waker3 = Waker::from(Arc::new(ListWaker {
            notified: v.clone(),
            index: 3,
        }));

        let mut l1 = event.listen();
        let mut l2 = event.listen();
        let mut l3 = event.listen();

        assert!(l1
            .as_mut()
            .poll(&mut Context::from_waker(&waker1))
            .is_pending());
        assert!(l2
            .as_mut()
            .poll(&mut Context::from_waker(&waker2))
            .is_pending());
        assert!(l3
            .as_mut()
            .poll(&mut Context::from_waker(&waker3))
            .is_pending());

        event.notify(core::usize::MAX);
        assert_eq!(&*v.lock().unwrap(), &[1, 2, 3]);

        assert!(l1
            .as_mut()
            .poll(&mut Context::from_waker(&waker1))
            .is_ready());
        assert!(l2
            .as_mut()
            .poll(&mut Context::from_waker(&waker2))
            .is_ready());
        assert!(l3
            .as_mut()
            .poll(&mut Context::from_waker(&waker3))
            .is_ready());
    }

    #[test]
    fn notify_tagged() {
        let event = Event::<i32>::new();

        let waker = waker_fn(|| {});

        let mut l1 = event.listen();
        let mut l2 = event.listen();

        // Should not be notified.
        assert!(l1
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        assert!(l2
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());

        // Notify with tags.
        event.notify(1.tag(1));
        event.notify(1.tag(2));

        // Should be notified.
        assert_eq!(
            l1.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Ready(1)
        );
        assert!(l2
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());

        // Notify with tags.
        event.notify(2.tag(13));

        // Should be notified.
        assert_eq!(
            l2.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Ready(13)
        );
    }

    #[test]
    fn notify_tagged_with() {
        let event = Event::<i32>::new();

        let waker = waker_fn(|| {});

        let mut l1 = event.listen();
        let mut l2 = event.listen();

        // Should not be notified.
        assert!(l1
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());
        assert!(l2
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());

        // Notify with tags.
        event.notify(1.tag_with(|| 1));
        event.notify(1.tag_with(|| 2));

        // Should be notified.
        assert_eq!(
            l1.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Ready(1)
        );
        assert!(l2
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending());

        // Notify with tags.
        event.notify(2.tag_with(|| 13));

        // Should be notified.
        assert_eq!(
            l2.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Ready(13)
        );
    }

    macro_rules! negative_test {
        (
            $(
                $tname:ident => $t:ty
            ),*
        ) => {$(
            #[test]
            #[should_panic]
            fn $tname() {
                let event = Event::<()>::new();
                let n: $t = -1;
                event.notify(n);
            }
        )*};
    }

    negative_test! {
        negative_test_i8 => i8,
        negative_test_i16 => i16,
        negative_test_i32 => i32,
        negative_test_i64 => i64,
        negative_test_i128 => i128,
        negative_test_isize => isize
    }
}
