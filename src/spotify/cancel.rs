//! Per-user cancellation registry for active Spotify download jobs.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};

use crate::common::job::JobRegistry;

static ACTIVE_SPOTIFY_JOBS: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

pub fn register_spotify_cancel(user_id: i64) -> Arc<AtomicBool> {
    ACTIVE_SPOTIFY_JOBS.register(user_id)
}

pub fn unregister_spotify_cancel(user_id: i64) {
    ACTIVE_SPOTIFY_JOBS.unregister(&user_id);
}

pub fn cancel_spotify_job(user_id: i64) -> bool {
    ACTIVE_SPOTIFY_JOBS.cancel(&user_id)
}

pub struct SpotifyUnregisterGuard(pub i64);

impl Drop for SpotifyUnregisterGuard {
    fn drop(&mut self) {
        unregister_spotify_cancel(self.0);
    }
}
