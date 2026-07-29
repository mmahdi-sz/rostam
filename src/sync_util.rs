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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_or_recover_normal_and_poison() {
        let m = Mutex::new(42);
        {
            let mut guard = lock_or_recover(&m);
            *guard = 100;
        }
        assert_eq!(*lock_or_recover(&m), 100);

        // Simulate poison
        let _ = std::panic::catch_unwind(|| {
            let _guard = m.lock().unwrap();
            panic!("test panic for poison");
        });

        assert!(m.is_poisoned());
        let recovered = lock_or_recover(&m);
        assert_eq!(*recovered, 100);
    }

    #[test]
    fn test_rwlock_or_recover() {
        let rw = RwLock::new("hello");
        {
            let mut w = write_or_recover(&rw);
            *w = "world";
        }
        {
            let r = read_or_recover(&rw);
            assert_eq!(*r, "world");
        }
    }
}
