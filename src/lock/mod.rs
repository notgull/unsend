//! Asynchronous locking primitives.

mod barrier;
mod mutex;
mod once_cell;
mod rwlock;
mod semaphore;

pub use barrier::{Barrier, BarrierWaitResult};
pub use mutex::{Mutex, MutexGuard};
pub use once_cell::OnceCell;
pub use rwlock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
pub use semaphore::{Semaphore, SemaphoreGuard};

#[cfg(feature = "alloc")]
pub use semaphore::SemaphoreGuardRc;

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future;

    #[test]
    fn test_mutex() {
        future::block_on(async {
            let mutex = Mutex::new(0);
            let mut guard = mutex.lock().await;
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = mutex.lock().await;

            // If we try to lock the mutex again, it will block.
            assert!(mutex.try_lock().is_none());
            assert!(future::poll_once(mutex.lock()).await.is_none());

            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_mutex_try_lock() {
        future::block_on(async {
            let mutex = Mutex::new(0);
            let mut guard = mutex.try_lock().unwrap();
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = mutex.try_lock().unwrap();
            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_mutex_try_lock_fail() {
        future::block_on(async {
            let mutex = Mutex::new(0);
            let mut guard = mutex.lock().await;
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = mutex.lock().await;
            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_mutex_lock_after_drop() {
        future::block_on(async {
            let mutex = Mutex::new(0);
            let mut guard = mutex.lock().await;
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = mutex.lock().await;
            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_mutex_lock_after_drop_try_lock() {
        future::block_on(async {
            let mutex = Mutex::new(0);
            let mut guard = mutex.try_lock().unwrap();
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = mutex.try_lock().unwrap();
            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_rwlock() {
        future::block_on(async {
            let rwlock = RwLock::new(0);
            let mut guard = rwlock.write().await;
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = rwlock.read().await;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = rwlock.write().await;
            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_rwlock_try_read() {
        future::block_on(async {
            let rwlock = RwLock::new(0);
            let mut guard = rwlock.write().await;
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = rwlock.try_read().unwrap();
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = rwlock.write().await;
            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_rwlock_try_write() {
        future::block_on(async {
            let rwlock = RwLock::new(0);
            let mut guard = rwlock.write().await;
            *guard += 1;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = rwlock.read().await;
            assert_eq!(*guard, 1);
            drop(guard);
            let guard = rwlock.try_write().unwrap();
            assert_eq!(*guard, 1);
        });
    }

    #[test]
    fn test_semaphore() {
        future::block_on(async {
            let semaphore = Semaphore::new(1);
            let _guard = semaphore.acquire().await;

            // If we try to acquire the semaphore again, it will block.
            assert!(semaphore.try_acquire().is_none());
            assert!(future::poll_once(semaphore.acquire()).await.is_none());
        });
    }
}
