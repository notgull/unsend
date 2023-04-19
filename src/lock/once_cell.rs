//! Value that can only be written once.

use crate::{Event, EventListener, IntoNotification};

use core::cell::{Cell, UnsafeCell};
use core::convert::Infallible;
use core::future::Future;
use core::pin::Pin;

/// A value that can only be written once.
pub struct OnceCell<T> {
    /// The event for writing to the cell.
    active: Event<()>,

    /// The event for the cell to be ready.
    passive: Event<()>,

    /// There is a future that is writing to the cell.
    writing: Cell<bool>,

    /// The underlying data.
    data: UnsafeCell<Option<T>>,
}

impl<T> OnceCell<T> {
    /// Creates a new `OnceCell`.
    pub const fn new() -> OnceCell<T> {
        OnceCell {
            active: Event::new(),
            passive: Event::new(),
            writing: Cell::new(false),
            data: UnsafeCell::new(None),
        }
    }

    /// Gets the value of the cell mutably.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        unsafe { &mut *self.data.get() }.as_mut()
    }

    /// Take the value out of the cell.
    pub fn take(&mut self) -> Option<T> {
        unsafe { &mut *self.data.get() }.take()
    }

    /// Get the value of the cell.
    pub fn get(&self) -> Option<&T> {
        unsafe { &*self.data.get() }.as_ref()
    }

    /// Set the value of the cell.
    pub async fn set(&self, value: T) -> Result<(), T> {
        let mut value = Some(value);
        self.get_or_init(async { value.take().unwrap() }).await;

        match value {
            Some(value) => Err(value),
            None => Ok(()),
        }
    }

    /// Get the value of the cell or try to initialize it.
    pub async fn get_or_try_init<E>(
        &self,
        setter: impl Future<Output = Result<T, E>>,
    ) -> Result<&T, E> {
        struct UnwriteOnDrop<'a, T> {
            cell: &'a OnceCell<T>,
        }

        impl<T> Drop for UnwriteOnDrop<'_, T> {
            fn drop(&mut self) {
                self.cell.writing.set(false);
                self.cell.active.notify(1);
            }
        }

        let mut listener = EventListener::new(&self.active);
        let mut setter = Some(setter);

        {
            let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

            loop {
                // Try to get the value.
                if let Some(value) = self.get() {
                    return Ok(value);
                }

                // If someone is already writing to the cell, wait for them to finish.
                if self.writing.replace(true) {
                    listener.as_mut().await;
                    continue;
                }

                // We now have exclusive access to the cell, try to write to it.
                let guard = UnwriteOnDrop { cell: self };
                match setter.take().unwrap().await {
                    Ok(data) => {
                        // Store the data and wake up all listeners.
                        unsafe {
                            *self.data.get() = Some(data);
                        }

                        self.passive.notify(core::usize::MAX.additional());
                        self.active.notify(core::usize::MAX.additional());

                        // Return the value.
                        return Ok(self.get().unwrap());
                    }

                    Err(e) => {
                        // Drop the value and wake up all listeners.
                        drop(guard);
                        return Err(e);
                    }
                }
            }
        }
    }

    /// Get the value of the cell or initialize it.
    pub async fn get_or_init(&self, setter: impl Future<Output = T>) -> &T {
        match self
            .get_or_try_init(async move { Ok::<T, Infallible>(setter.await) })
            .await
        {
            Ok(value) => value,
            Err(e) => match e {},
        }
    }

    /// Wait for the cell to be ready.
    pub async fn wait(&self) {
        let mut listener = EventListener::new(&self.passive);
        let mut listener = unsafe { Pin::new_unchecked(&mut listener) };

        while self.get().is_none() {
            listener.as_mut().await;
        }
    }
}

impl<T> From<T> for OnceCell<T> {
    fn from(value: T) -> OnceCell<T> {
        OnceCell {
            active: Event::new(),
            passive: Event::new(),
            writing: Cell::new(false),
            data: UnsafeCell::new(Some(value)),
        }
    }
}
