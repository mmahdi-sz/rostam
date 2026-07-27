use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Helper to safely acquire a std::sync::Mutex lock, recovering from poison if a thread panicked.
pub fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Helper to safely acquire a std::sync::RwLock read guard, recovering from poison if a thread panicked.
pub fn read_or_recover<T>(rw: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    rw.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Helper to safely acquire a std::sync::RwLock write guard, recovering from poison if a thread panicked.
pub fn write_or_recover<T>(rw: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    rw.write().unwrap_or_else(|poisoned| poisoned.into_inner())
}
