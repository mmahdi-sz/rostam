//! Per-user cancellation registry for active Spotify download jobs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static ACTIVE_SPOTIFY_JOBS: OnceLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = OnceLock::new();

fn active_spotify_jobs() -> &'static Mutex<HashMap<i64, Arc<AtomicBool>>> {
    ACTIVE_SPOTIFY_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_spotify_cancel(user_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    crate::sync_util::lock_or_recover(active_spotify_jobs()).insert(user_id, flag.clone());
    flag
}

pub fn unregister_spotify_cancel(user_id: i64) {
    crate::sync_util::lock_or_recover(active_spotify_jobs()).remove(&user_id);
}

pub fn cancel_spotify_job(user_id: i64) -> bool {
    if let Some(flag) = crate::sync_util::lock_or_recover(active_spotify_jobs()).remove(&user_id) {
        flag.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

pub struct SpotifyUnregisterGuard(pub i64);

impl Drop for SpotifyUnregisterGuard {
    fn drop(&mut self) {
        unregister_spotify_cancel(self.0);
    }
}
