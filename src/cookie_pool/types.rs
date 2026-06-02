use std::{
    path::PathBuf,
    time::SystemTime,
};

#[derive(Clone, Debug)]
pub struct CookieSource {
    pub id: String,
    pub profile_name: String,
    pub profile_dir: PathBuf,
    pub cookies_sqlite: PathBuf,
}

impl CookieSource {
    pub fn yt_dlp_browser_spec(&self) -> String {
        format!("firefox:{}", self.profile_dir.display())
    }
}

#[derive(Clone, Debug)]
pub struct CooldownEntry {
    pub cookie_id: String,
    pub expire_at: SystemTime,
}

#[derive(Clone, Debug)]
pub struct SelectedCookie {
    pub id: String,
    pub profile_name: String,
    pub cookies_file: PathBuf,
    pub yt_dlp_browser_spec: String,
}

#[derive(Debug)]
pub struct CookiePoolStatus {
    pub available_cookies: usize,
    pub selectable_cookies: usize,
    pub cooldown_cookies: usize,
    pub last_used_cookie: Option<String>,
    pub next_available_in: Option<std::time::Duration>,
}

#[derive(Clone, Debug)]
pub struct CookiePoolSnapshot {
    pub available_cookies: Vec<CookieSource>,
    pub last_used_cookie: Option<String>,
    pub cooldown_list: Vec<CooldownEntry>,
}
