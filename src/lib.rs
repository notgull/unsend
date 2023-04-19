//! A thread-unsafe runtime for thread-unsafe people.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod channel;
pub mod lock;

mod event;

pub use event::{Event, EventListener, IntoNotification, Notification};
