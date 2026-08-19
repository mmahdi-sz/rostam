//! Generic job registration, cancellation token registry, and RAII unregister guard.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::sync_util::lock_or_recover;

/// Generic per-key job cancellation registry.
pub struct JobRegistry<K: Eq + Hash + Clone + 'static = i64, V: Clone + 'static = Arc<AtomicBool>> {
    jobs: Mutex<HashMap<K, V>>,
}

impl<K: Eq + Hash + Clone + 'static, V: Clone + 'static> Default for JobRegistry<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash + Clone + 'static, V: Clone + 'static> JobRegistry<K, V> {
    /// Creates a new empty job registry.
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }

    /// Unregisters and returns the entry for the specified key.
    pub fn unregister(&self, key: &K) -> Option<V> {
        lock_or_recover(&self.jobs).remove(key)
    }

    /// Checks if a job is currently active for the given key.
    pub fn is_active(&self, key: &K) -> bool {
        lock_or_recover(&self.jobs).contains_key(key)
    }

    /// Retrieves a clone of the entry if active.
    pub fn get(&self, key: &K) -> Option<V> {
        lock_or_recover(&self.jobs).get(key).cloned()
    }

    /// Clears all entries from the registry.
    pub fn clear(&self) {
        lock_or_recover(&self.jobs).clear();
    }

    /// Inserts a custom token into the registry.
    pub fn register_custom(&self, key: K, val: V) {
        lock_or_recover(&self.jobs).insert(key, val);
    }

    /// Creates an RAII guard for the given key that will unregister it upon drop.
    pub fn guard(&'static self, key: K) -> JobGuard<K, V> {
        JobGuard {
            registry: self,
            key,
            disarmed: false,
        }
    }
}

// ── Specialized implementation for standard AtomicBool cancellation tokens ──

impl<K: Eq + Hash + Clone + 'static> JobRegistry<K, Arc<AtomicBool>> {
    /// Registers a new cancel flag for `key` and returns the Arc<AtomicBool>.
    pub fn register(&self, key: K) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.register_custom(key, flag.clone());
        flag
    }

    /// Registers a new cancel flag for `key` and returns both the flag and an RAII unregister guard.
    pub fn register_with_guard(
        &'static self,
        key: K,
    ) -> (Arc<AtomicBool>, JobGuard<K, Arc<AtomicBool>>) {
        let flag = self.register(key.clone());
        let guard = self.guard(key);
        (flag, guard)
    }

    /// Registers a pre-existing cancel flag for `key` and returns an RAII unregister guard.
    pub fn register_flag(
        &'static self,
        key: K,
        flag: Arc<AtomicBool>,
    ) -> JobGuard<K, Arc<AtomicBool>> {
        self.register_custom(key.clone(), flag);
        self.guard(key)
    }

    /// Signals cancellation by setting the flag to true (using SeqCst) and returns whether an active job was found.
    pub fn cancel(&self, key: &K) -> bool {
        if let Some(flag) = lock_or_recover(&self.jobs).remove(key) {
            flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
}

// ── Specialized implementation for tokio::sync::Notify cancellation tokens ──

impl<K: Eq + Hash + Clone + 'static> JobRegistry<K, Arc<tokio::sync::Notify>> {
    /// Registers a new Notify token for `key` and returns the Arc<Notify>.
    pub fn register_notify(&self, key: K) -> Arc<tokio::sync::Notify> {
        let notify = Arc::new(tokio::sync::Notify::new());
        self.register_custom(key, notify.clone());
        notify
    }

    /// Registers a new Notify token for `key` and returns both the token and an RAII unregister guard.
    pub fn register_notify_with_guard(
        &'static self,
        key: K,
    ) -> (Arc<tokio::sync::Notify>, JobGuard<K, Arc<tokio::sync::Notify>>) {
        let notify = self.register_notify(key.clone());
        let guard = self.guard(key);
        (notify, guard)
    }

    /// Signals cancellation by calling notify_one() and removing the entry. Returns whether an active job was found.
    pub fn cancel_notify(&self, key: &K) -> bool {
        if let Some(notify) = self.unregister(key) {
            notify.notify_one();
            true
        } else {
            false
        }
    }
}

/// RAII Guard that unregisters the job from the registry when dropped.
pub struct JobGuard<K: Eq + Hash + Clone + 'static = i64, V: Clone + 'static = Arc<AtomicBool>> {
    registry: &'static JobRegistry<K, V>,
    key: K,
    disarmed: bool,
}

impl<K: Eq + Hash + Clone + 'static, V: Clone + 'static> JobGuard<K, V> {
    /// Disarms the guard so it will not unregister on drop.
    pub fn disarm(mut self) {
        self.disarmed = true;
    }

    /// Returns a reference to the registered key.
    pub fn key(&self) -> &K {
        &self.key
    }
}

impl<K: Eq + Hash + Clone + 'static, V: Clone + 'static> Drop for JobGuard<K, V> {
    fn drop(&mut self) {
        if !self.disarmed {
            self.registry.unregister(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, LazyLock};

    static TEST_REGISTRY: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

    #[test]
    fn test_register_and_cancel() {
        let user_id = 100_001;
        let (flag, _guard) = TEST_REGISTRY.register_with_guard(user_id);
        assert!(TEST_REGISTRY.is_active(&user_id));
        assert!(!flag.load(Ordering::SeqCst));

        let cancelled = TEST_REGISTRY.cancel(&user_id);
        assert!(cancelled);
        assert!(flag.load(Ordering::SeqCst));
        assert!(!TEST_REGISTRY.is_active(&user_id));

        // Cancelling again returns false
        assert!(!TEST_REGISTRY.cancel(&user_id));
    }

    #[test]
    fn test_raii_guard_drop() {
        let user_id = 100_002;
        {
            let (flag, _guard) = TEST_REGISTRY.register_with_guard(user_id);
            assert!(TEST_REGISTRY.is_active(&user_id));
            assert!(!flag.load(Ordering::SeqCst));
        }
        // Dropping guard unregisters key automatically
        assert!(!TEST_REGISTRY.is_active(&user_id));
    }

    #[test]
    fn test_guard_disarm() {
        let user_id = 100_003;
        {
            let (_flag, guard) = TEST_REGISTRY.register_with_guard(user_id);
            assert!(TEST_REGISTRY.is_active(&user_id));
            guard.disarm();
        }
        // Guard was disarmed, so key remains active until explicitly removed
        assert!(TEST_REGISTRY.is_active(&user_id));
        assert!(TEST_REGISTRY.unregister(&user_id).is_some());
        assert!(!TEST_REGISTRY.is_active(&user_id));
    }

    #[test]
    fn test_poison_recovery() {
        let reg = JobRegistry::<i64>::new();
        let _ = std::panic::catch_unwind(|| {
            let _guard = reg.jobs.lock().unwrap();
            panic!("simulate panic while holding lock");
        });
        assert!(reg.jobs.is_poisoned());

        // Registry should still operate normally due to lock_or_recover
        let user_id = 100_004;
        let flag = Arc::new(AtomicBool::new(false));
        reg.register_custom(user_id, flag.clone());
        assert!(reg.is_active(&user_id));
        assert!(reg.cancel(&user_id));
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn test_concurrent_multi_user() {
        use std::thread;

        let mut handles = Vec::new();
        for i in 0..20 {
            handles.push(thread::spawn(move || {
                let user_id = 200_000 + i;
                let (flag, guard) = TEST_REGISTRY.register_with_guard(user_id);
                assert!(TEST_REGISTRY.is_active(&user_id));
                if i % 2 == 0 {
                    assert!(TEST_REGISTRY.cancel(&user_id));
                    assert!(flag.load(Ordering::SeqCst));
                }
                drop(guard);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        for i in 0..20 {
            let user_id = 200_000 + i;
            assert!(!TEST_REGISTRY.is_active(&user_id));
        }
    }

    #[tokio::test]
    async fn test_notify_job_registry() {
        static NOTIFY_REG: LazyLock<JobRegistry<u64, Arc<tokio::sync::Notify>>> =
            LazyLock::new(JobRegistry::new);

        let req_id = 500_001u64;
        let notify = NOTIFY_REG.register_notify(req_id);
        assert!(NOTIFY_REG.is_active(&req_id));

        // Cancellation should trigger notify_one
        let mut notified_fut = std::pin::pin!(notify.notified());
        let cancelled = NOTIFY_REG.cancel_notify(&req_id);
        assert!(cancelled);
        assert!(!NOTIFY_REG.is_active(&req_id));

        // Ensure notify fired
        tokio::time::timeout(std::time::Duration::from_millis(50), notified_fut.as_mut())
            .await
            .expect("notify must fire on cancel");

        // Test guard drop unregisters
        let req_id_2 = 500_002u64;
        {
            let (_n2, _guard) = NOTIFY_REG.register_notify_with_guard(req_id_2);
            assert!(NOTIFY_REG.is_active(&req_id_2));
        }
        assert!(!NOTIFY_REG.is_active(&req_id_2));
    }
}
