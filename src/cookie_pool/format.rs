use std::time::Duration;

use crate::i18n::{t, tf};

use super::types::{CookiePoolStatus, SelectedCookie};

pub fn format_cookie_status(status: &CookiePoolStatus) -> String {
    let last_used = status.last_used_cookie.as_deref().unwrap_or("-");
    let wait = status.next_available_in.map(format_duration).unwrap_or_else(|| "-".to_owned());
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        t("cookie.status_header"),
        tf("cookie.status_line_available", &[("available", &status.available_cookies.to_string())]),
        tf("cookie.status_line_selectable", &[("selectable", &status.selectable_cookies.to_string())]),
        tf("cookie.status_line_cooldown", &[("cooldown", &status.cooldown_cookies.to_string())]),
        tf("cookie.status_line_last_used", &[("last_used", last_used)]),
        tf("cookie.status_line_next_available", &[("wait", &wait)]),
    )
}

pub fn format_selected_cookie(cookie: &SelectedCookie) -> String {
    tf(
        "cookie.selected",
        &[
            ("id", &cookie.id),
            ("profile", &cookie.profile_name),
            ("file", &cookie.cookies_file.display().to_string()),
            ("spec", &cookie.yt_dlp_browser_spec),
        ],
    )
}

pub fn format_no_cookie_available(status: &CookiePoolStatus) -> String {
    let wait = status.next_available_in.map(format_duration).unwrap_or_else(|| "20h".to_owned());
    tf("cookie.none_available", &[("wait", &wait)])
}

pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 { format!("{hours}h {minutes}m") } else { format!("{minutes}m {seconds}s") }
}
