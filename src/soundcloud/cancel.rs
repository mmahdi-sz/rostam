//! Per-user cancellation registry for active SoundCloud download jobs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static ACTIVE_SOUNDCLOUD_JOBS: OnceLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = OnceLock::new();

fn active_soundcloud_jobs() -> &'static Mutex<HashMap<i64, Arc<AtomicBool>>> {
    ACTIVE_SOUNDCLOUD_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_soundcloud_cancel(user_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    crate::sync_util::lock_or_recover(active_soundcloud_jobs()).insert(user_id, flag.clone());
    flag
}

pub fn unregister_soundcloud_cancel(user_id: i64) {
    crate::sync_util::lock_or_recover(active_soundcloud_jobs()).remove(&user_id);
}

pub fn cancel_soundcloud_job(user_id: i64) -> bool {
    if let Some(flag) = crate::sync_util::lock_or_recover(active_soundcloud_jobs()).remove(&user_id)
    {
        flag.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

pub struct SoundcloudUnregisterGuard(pub i64);

impl Drop for SoundcloudUnregisterGuard {
    fn drop(&mut self) {
        unregister_soundcloud_cancel(self.0);
    }
}
