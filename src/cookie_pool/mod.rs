use std::time::Duration;

mod discover;
mod format;
pub mod fresh;
mod pool;
mod snapshot;
mod types;

pub use format::format_no_cookie_available;
pub use pool::CookiePool;
pub use snapshot::save_snapshot;
pub use types::{CookiePoolSnapshot, CookieSource, CooldownEntry};

/// Cooldown applied to a rate-limited cookie (programmatic / manual commands).
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Safety-net cooldown set when a cookie is rate-limited and queued for auto-refresh.
/// Cookie stays out of the pool until refresh finishes and `remove_from_cooldown` is called;
/// this duration is just a fallback in case the refresh task crashes.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(4 * 60 * 60);
const DEFAULT_CACHE_ROOT: &str = "cookie_profiles_cache";
