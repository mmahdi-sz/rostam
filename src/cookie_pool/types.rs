use std::{path::PathBuf, time::SystemTime};

#[derive(Clone, Debug)]
pub struct CookieSource {
    pub id: String,
    pub profile_name: String,
    /// The working copy used by yt-dlp (may be a cache dir after materialize).
    pub profile_dir: PathBuf,
    pub cookies_sqlite: PathBuf,
    /// The real Firefox profile dir on disk (for opening Firefox and login checks).
    pub source_profile_dir: PathBuf,
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
    #[allow(dead_code)]
    pub cookies_file: PathBuf,
    pub yt_dlp_browser_spec: String,
}

#[derive(Debug)]
pub struct CookiePoolStatus {
    pub available_cookies: usize,
    pub selectable_cookies: usize,
    pub cooldown_cookies: usize,
    #[allow(dead_code)]
    pub last_used_cookie: Option<String>,
    pub next_available_in: Option<std::time::Duration>,
}

#[derive(Clone, Debug)]
pub struct CookiePoolSnapshot {
    pub available_cookies: Vec<CookieSource>,
    pub last_used_cookie: Option<String>,
    pub cooldown_list: Vec<CooldownEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cookie_source_browser_spec() {
        let source = CookieSource {
            id: "cookie-1".to_string(),
            profile_name: "profile-1".to_string(),
            profile_dir: PathBuf::from("/tmp/profile1"),
            cookies_sqlite: PathBuf::from("/tmp/profile1/cookies.sqlite"),
            source_profile_dir: PathBuf::from("/home/user/profile1"),
        };
        assert_eq!(source.yt_dlp_browser_spec(), "firefox:/tmp/profile1");
    }
}
