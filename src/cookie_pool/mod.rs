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
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(20 * 60 * 60);
const DEFAULT_CACHE_ROOT: &str = "cookie_profiles_cache";
