//! Locks that outlive a panic. A thread that panics while holding a lock poisons it;
//! what haze keeps behind its locks is plain data that stays usable, so the lock is
//! taken anyway instead of spreading the panic to every other thread.

use std::sync::{Mutex, MutexGuard, PoisonError};

pub trait Lock<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> Lock<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
