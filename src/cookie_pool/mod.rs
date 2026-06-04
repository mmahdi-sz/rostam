use std::time::Duration;

mod types;
mod discover;
mod pool;
mod format;
mod snapshot;

pub use types::{CookieSource, CooldownEntry, SelectedCookie, CookiePoolStatus, CookiePoolSnapshot};
pub use pool::CookiePool;
pub use format::{format_cookie_status, format_selected_cookie, format_no_cookie_available, format_duration};
pub use snapshot::save_snapshot;

const DEFAULT_FIREFOX_ROOT: &str = "/home/mahdi/.mozilla/firefox";
const DEFAULT_MAX_COOKIES: usize = 20;
/// Cooldown applied to a rate-limited cookie (programmatic / manual commands).
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Safety-net cooldown set when a cookie is rate-limited and queued for auto-refresh.
/// Cookie stays out of the pool until refresh finishes and `remove_from_cooldown` is called;
/// this duration is just a fallback in case the refresh task crashes.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(4 * 60 * 60);
const DEFAULT_CACHE_ROOT: &str = "cookie_profiles_cache";
