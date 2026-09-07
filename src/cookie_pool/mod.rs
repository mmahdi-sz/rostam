//! YouTube cookie pool management.
//!
//! Manages a rotating pool of Firefox-profile cookies for yt-dlp.
//! Handles per-cookie rate-limit state, fresh-cookie detection, and pool selection.
//! Background refresher: [`modules::spawn_cookie_refresher`].

use std::time::Duration;

mod discover;
mod format;
pub mod fresh;
mod pool;
mod snapshot;
mod types;

pub use pool::CookiePool;
pub use snapshot::save_snapshot;
pub use types::{CookiePoolSnapshot, CookieSource, CooldownEntry};

/// Cooldown applied to a rate-limited cookie (programmatic / manual commands).
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Safety-net cooldown set when a cookie is rate-limited and queued for auto-refresh.
/// Cookie stays out of the pool until refresh finishes and `remove_from_cooldown` is called;
/// this duration is just a fallback in case the refresh task crashes.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(60 * 60);
const DEFAULT_CACHE_ROOT: &str = "cookie_profiles_cache";

use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

static GLOBAL_COOKIE_POOL: OnceLock<Arc<Mutex<CookiePool>>> = OnceLock::new();

pub fn set_global_cookie_pool(pool: Arc<Mutex<CookiePool>>) {
    let _ = GLOBAL_COOKIE_POOL.set(pool);
}

pub async fn get_global_cookie_spec() -> Option<String> {
    let pool_arc = GLOBAL_COOKIE_POOL.get()?;
    let mut pool = pool_arc.lock().await;
    pool.next_cookie().map(|c| c.yt_dlp_browser_spec)
}
