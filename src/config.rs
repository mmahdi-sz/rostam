use std::sync::OnceLock;
use std::{env, fs};

static BOT_USERNAME: OnceLock<String> = OnceLock::new();

pub fn set_bot_username(username: String) {
    let _ = BOT_USERNAME.set(username);
}

pub fn bot_username() -> &'static str {
    BOT_USERNAME.get().map(|s| s.as_str()).unwrap_or("")
}

pub fn bot_token() -> Result<String, Box<dyn std::error::Error>> {
    config_value("BOT_TOKEN")
        .ok_or_else(|| "BOT_TOKEN is not set in .env, /etc/default/abc, or process env".into())
}

pub fn admin_user_id() -> Option<i64> {
    config_value("ADMIN_USER_ID")?.parse().ok()
}

pub fn bot_api_base_url() -> Option<String> {
    config_value("BOT_API_BASE_URL")
}

pub fn dev_mode() -> bool {
    config_value("DEV_MODE")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

pub fn cookie_refresh_enabled() -> bool {
    config_value("COOKIE_REFRESH_ENABLED")
        .map(|v| !matches!(v.to_lowercase().as_str(), "0" | "false" | "no"))
        .unwrap_or(true)
}

/// Redis connection URL (shared between dev and production so cookie freshness
/// keys are not duplicated). Defaults to local Redis.
pub fn redis_url() -> String {
    config_value("REDIS_URL").unwrap_or_else(|| "redis://127.0.0.1:6379".to_string())
}

/// How long a freshly-refreshed cookie is considered fresh (Redis TTL). Default 36h.
pub fn cookie_fresh_ttl_secs() -> u64 {
    config_value("COOKIE_FRESH_TTL_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(36 * 3600)
}

/// How often the cookie worker wakes up to look for stale profiles. Default 10 min.
pub fn cookie_worker_interval_secs() -> u64 {
    config_value("COOKIE_WORKER_INTERVAL_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10 * 60)
}

/// Refresh lock TTL — held while a profile is being refreshed so dev and prod
/// never open the same Firefox profile at once. Also acts as the back-off window
/// after a failed refresh (lock is left to expire instead of being released).
/// Default 30 min (> the ~10 min Firefox warm-up).
pub fn cookie_refresh_lock_ttl_secs() -> u64 {
    config_value("COOKIE_REFRESH_LOCK_TTL_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30 * 60)
}

/// Short label identifying this deployment (used as the Redis refresh-lock owner
/// for diagnostics). Falls back to dev/prod based on DEV_MODE.
pub fn env_label() -> String {
    config_value("ENV_LABEL").unwrap_or_else(|| if dev_mode() { "dev".into() } else { "prod".into() })
}

/// Max accepted PDF size for the compression feature. Default 2GB.
pub fn pdf_compress_max_bytes() -> u64 {
    config_value("PDF_COMPRESS_MAX_MB")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(2048) * 1024 * 1024
}

/// Hard timeout for the `gs` subprocess — killed cleanly past this. Default 30 min.
pub fn pdf_compress_timeout_secs() -> u64 {
    config_value("PDF_COMPRESS_TIMEOUT_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30 * 60)
}

/// ipinfo.io API token — source omitted from the IP lookup card when unset.
pub fn ipinfo_token() -> Option<String> {
    config_value("IPINFO_TOKEN")
}

/// AbuseIPDB API key — source omitted from the IP lookup card when unset.
pub fn abuseipdb_key() -> Option<String> {
    config_value("ABUSEIPDB_KEY")
}

/// How often the IP lookup background task refreshes the cloud/Tor/Spamhaus
/// static lists. Default 6h.
pub fn ip_lists_refresh_secs() -> u64 {
    config_value("IP_LISTS_REFRESH_SECS")
        .and_then(|v| v.parse().ok())
        .unwrap_or(6 * 3600)
}

/// Where YouTube downloads are staged before upload. Defaults to a path relative
/// to the working directory so dev and prod never share a directory (each systemd
/// unit's WorkingDirectory differs) unless explicitly pointed at the same place.
pub fn youtube_download_root() -> String {
    config_value("YOUTUBE_DOWNLOAD_ROOT").unwrap_or_else(|| "downloads/yt".to_string())
}

/// Where the `surge` direct-link downloader tool stages files.
pub fn surge_downloads_root() -> String {
    config_value("SURGE_DOWNLOADS_ROOT").unwrap_or_else(|| "downloads/surge".to_string())
}

/// host:port of the running `surge server` daemon.
pub fn surge_host() -> String {
    config_value("SURGE_HOST").unwrap_or_else(|| "127.0.0.1:1700".to_string())
}

/// Root directory containing Firefox profiles used for cookie discovery
/// (`~/.mozilla/firefox` equivalent). Falls back to `$HOME/.mozilla/firefox`
/// when unset; if `$HOME` is also unset, cookie discovery finds no profiles.
pub fn firefox_profiles_root() -> Option<String> {
    config_value("FIREFOX_PROFILES_ROOT")
        .or_else(|| env::var("HOME").ok().map(|home| format!("{home}/.mozilla/firefox")))
}

/// Path to the `deno` binary passed to yt-dlp's `--js-runtimes` flag. Defaults
/// to bare `deno`, letting yt-dlp resolve it via PATH.
pub fn deno_path() -> String {
    config_value("DENO_PATH").unwrap_or_else(|| "deno".to_string())
}

/// OS user the cookie-refresher spawns Firefox as (via `sudo -u <user>`). When
/// unset, Firefox is spawned directly as the bot's own user instead of via sudo.
pub fn cookie_refresh_os_user() -> Option<String> {
    config_value("COOKIE_REFRESH_OS_USER")
}

/// X display the cookie-refresher opens Firefox on. Default `:0`.
pub fn cookie_refresh_display() -> String {
    config_value("COOKIE_REFRESH_DISPLAY").unwrap_or_else(|| ":0".to_string())
}

/// `XDG_RUNTIME_DIR` passed to the spawned Firefox process. Left unset (and
/// omitted from the child's env) when not configured.
pub fn cookie_refresh_xdg_runtime_dir() -> Option<String> {
    config_value("COOKIE_REFRESH_XDG_RUNTIME_DIR")
}

pub fn config_value(key: &str) -> Option<String> {
    value_from_env_file(".env", key)
        .or_else(|| value_from_env_file("/etc/default/abc", key))
        .or_else(|| env::var(key).ok())
}

fn value_from_env_file(path: &str, target_key: &str) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    contents.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        if key.trim() != target_key {
            return None;
        }
        let token = unquote_env_value(value.trim());
        if token.is_empty() { None } else { Some(token.to_owned()) }
    })
}

fn unquote_env_value(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}
