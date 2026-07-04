use std::time::Duration;

mod types;
mod discover;
mod pool;
mod format;
mod snapshot;
pub mod fresh;

pub use types::{CookieSource, CooldownEntry, CookiePoolSnapshot};
pub use pool::CookiePool;
pub use format::format_no_cookie_available;
pub use snapshot::save_snapshot;

const DEFAULT_FIREFOX_ROOT: &str = "/home/mahdi/.mozilla/firefox";
/// Cooldown applied to a rate-limited cookie (programmatic / manual commands).
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Safety-net cooldown set when a cookie is rate-limited and queued for auto-refresh.
/// Cookie stays out of the pool until refresh finishes and `remove_from_cooldown` is called;
/// this duration is just a fallback in case the refresh task crashes.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(4 * 60 * 60);
const DEFAULT_CACHE_ROOT: &str = "cookie_profiles_cache";
