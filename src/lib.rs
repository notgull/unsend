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

//! A thread-unsafe runtime for thread-unsafe people.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(coverage, feature(no_coverage))]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
pub mod channel;
#[cfg(feature = "executor")]
#[cfg_attr(docsrs, doc(cfg(feature = "executor")))]
pub mod executor;
pub mod lock;

mod event;

pub use event::{Event, EventListener, IntoNotification, Notification};

#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
pub use event::EventListenerRc;

mod sync {
    #[cfg(feature = "alloc")]
    pub use alloc::sync::Arc;
    pub use core::sync::atomic;
}
