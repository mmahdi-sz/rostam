//! Per-user cancellation registry for active SoundCloud download jobs.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};

use crate::common::job::JobRegistry;

static ACTIVE_SOUNDCLOUD_JOBS: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

pub fn register_soundcloud_cancel(user_id: i64) -> Arc<AtomicBool> {
    ACTIVE_SOUNDCLOUD_JOBS.register(user_id)
}

pub fn unregister_soundcloud_cancel(user_id: i64) {
    ACTIVE_SOUNDCLOUD_JOBS.unregister(&user_id);
}

pub fn cancel_soundcloud_job(user_id: i64) -> bool {
    ACTIVE_SOUNDCLOUD_JOBS.cancel(&user_id)
}

pub struct SoundcloudUnregisterGuard(pub i64);

impl Drop for SoundcloudUnregisterGuard {
    fn drop(&mut self) {
        unregister_soundcloud_cancel(self.0);
    }
}
