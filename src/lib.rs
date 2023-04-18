//! A thread-unsafe runtime for thread-unsafe people.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod event;

pub use event::{Event, EventListener, IntoNotification, Notification};
