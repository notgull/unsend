//! Asynchronous locking primitives.

mod mutex;
pub use mutex::{Mutex, MutexGuard};
