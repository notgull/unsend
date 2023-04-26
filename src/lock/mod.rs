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
    fn test_mutex_get_mut_into_inner() {
        let mut mutex = Mutex::<i32>::default();
        *mutex.get_mut() = 3;
        assert_eq!(mutex.into_inner(), 3);
    }

    #[test]
    fn test_mutex_lock_await() {
        future::block_on(async {
            let mutex = Mutex::new(0);
            let mut guard = mutex.lock().await;
            *guard += 1;

            let lock2 = mutex.lock();
            futures_lite::pin!(lock2);

            assert!(future::poll_once(&mut lock2).await.is_none());
            drop(guard);
            assert!(future::poll_once(&mut lock2).await.is_some());
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
    fn test_rwlock_get_mut() {
        let mut rwlock = RwLock::<i32>::default();
        *rwlock.get_mut() = 3;
        assert_eq!(rwlock.into_inner(), 3);
    }

    #[test]
    fn test_rwlock_read_write_await() {
        future::block_on(async {
            let rwlock = RwLock::new(0);
            let guard = rwlock.write().await;
            assert_eq!(*guard, 0);

            let read2 = rwlock.read();
            futures_lite::pin!(read2);

            assert!(future::poll_once(&mut read2).await.is_none());
            drop(guard);
            assert!(future::poll_once(&mut read2).await.is_some());

            let guard = rwlock.read().await;
            assert_eq!(*guard, 0);

            let write2 = rwlock.write();
            futures_lite::pin!(write2);

            assert!(future::poll_once(&mut write2).await.is_none());
            drop(guard);
            assert!(future::poll_once(&mut write2).await.is_some());
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

    #[test]
    fn test_semaphore_await() {
        future::block_on(async {
            let semaphore = Semaphore::new(1);
            let _guard = semaphore.acquire().await;

            // If we try to acquire the semaphore again, it will block.
            assert!(semaphore.try_acquire().is_none());

            let acquire = semaphore.acquire();
            futures_lite::pin!(acquire);

            assert!(future::poll_once(&mut acquire).await.is_none());
            drop(_guard);
            assert!(future::poll_once(&mut acquire).await.is_some());
        });
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn test_semaphore_rc() {
        use alloc::rc::Rc;

        future::block_on(async {
            let semaphore = Rc::new(Semaphore::new(1));
            let _guard = semaphore.clone().acquire_rc().await;

            // If we try to acquire the semaphore again, it will block.
            assert!(semaphore.clone().try_acquire_rc().is_none());
            assert!(future::poll_once(semaphore.acquire()).await.is_none());
        });
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn test_semaphore_rc_await() {
        use alloc::rc::Rc;

        future::block_on(async {
            let semaphore = Rc::new(Semaphore::new(1));
            let _guard = semaphore.clone().acquire_rc().await;

            // If we try to acquire the semaphore again, it will block.
            assert!(semaphore.clone().try_acquire_rc().is_none());

            let acquire = semaphore.acquire_rc();
            futures_lite::pin!(acquire);

            assert!(future::poll_once(&mut acquire).await.is_none());
            drop(_guard);
            assert!(future::poll_once(&mut acquire).await.is_some());
        });
    }

    #[test]
    fn once_cell_smoke() {
        future::block_on(async {
            let cell = OnceCell::<i32>::new();
            assert!(cell.get().is_none());

            let value = cell.get_or_init(async { 5 }).await;
            assert_eq!(*value, 5);
            assert_eq!(cell.get().unwrap(), &5);
        });
    }

    #[test]
    fn once_cell_get_mut() {
        let mut cell = OnceCell::<i32>::from(5);
        assert!(cell.get_mut().is_some());
        assert_eq!(cell.take(), Some(5));
    }

    #[test]
    fn once_cell_wait() {
        future::block_on(async {
            let cell = OnceCell::<i32>::new();

            let waiter = cell.wait();
            futures_lite::pin!(waiter);

            assert!(future::poll_once(&mut waiter).await.is_none());
            cell.set(5).await.unwrap();
            assert!(future::poll_once(&mut waiter).await.is_some());
        });
    }
}
