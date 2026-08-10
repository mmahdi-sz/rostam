//! Shared pipeline infrastructure for Photo & Video Magic Studio media editing tools.
//!
//! Provides RAII cleanup guards, active job cancellation registries, and shared
//! job progress tracking for future Studio tools (crop, watermark, format conversion, trim).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// RAII guard for temporary directories created during Studio operations.
/// Guarantees directory removal on drop across all exit paths (normal return, error, cancel, panic).
#[derive(Debug)]
pub struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Global registry of active Studio jobs, mapping `user_id -> cancel_flag`.
static ACTIVE_STUDIO_JOBS: LazyLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Registers a cancel flag for a user's Studio job.
pub fn register_active_job(user_id: i64, cancel_flag: Arc<AtomicBool>) {
    if let Ok(mut jobs) = ACTIVE_STUDIO_JOBS.lock() {
        jobs.insert(user_id, cancel_flag);
    }
}

/// Removes and returns the cancel flag for a user's Studio job.
pub fn remove_active_job(user_id: i64) -> Option<Arc<AtomicBool>> {
    if let Ok(mut jobs) = ACTIVE_STUDIO_JOBS.lock() {
        jobs.remove(&user_id)
    } else {
        None
    }
}

/// Signals cancellation for a user's active Studio job.
pub fn cancel_active_job(user_id: i64) -> bool {
    if let Ok(mut jobs) = ACTIVE_STUDIO_JOBS.lock() {
        if let Some(flag) = jobs.remove(&user_id) {
            flag.store(true, Ordering::Relaxed);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temp_dir_guard_removes_dir() {
        let temp = std::env::temp_dir().join(format!("studio_test_guard_{}", rand_id()));
        std::fs::create_dir_all(&temp).unwrap();
        assert!(temp.exists());

        {
            let _guard = TempDirGuard::new(temp.clone());
        }

        assert!(!temp.exists());
    }

    #[test]
    fn test_active_job_registry() {
        let uid = 987654321;
        let flag = Arc::new(AtomicBool::new(false));
        register_active_job(uid, flag.clone());

        assert!(!flag.load(Ordering::Relaxed));
        let cancelled = cancel_active_job(uid);
        assert!(cancelled);
        assert!(flag.load(Ordering::Relaxed));
        assert!(remove_active_job(uid).is_none());
    }

    fn rand_id() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(12345)
    }
}
