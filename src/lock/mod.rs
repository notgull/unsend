//! Asynchronous locking primitives.

mod mutex;
pub use mutex::{Mutex, MutexGuard};

mod rwlock;
pub use rwlock::RwLock;
