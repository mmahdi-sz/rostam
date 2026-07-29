use std::{
    collections::HashSet,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{
    DEFAULT_CACHE_ROOT, DEFAULT_COOLDOWN, REFRESH_COOLDOWN,
    discover::{discover_firefox_cookies, materialize_profiles_cache},
    types::{CookiePoolSnapshot, CookiePoolStatus, CookieSource, CooldownEntry, SelectedCookie},
};

pub struct CookiePool {
    available_cookies: Vec<CookieSource>,
    last_used_cookie: Option<String>,
    cooldown_list: Vec<CooldownEntry>,
    #[allow(dead_code)]
    cooldown: Duration,
    random_counter: u64,
}

impl CookiePool {
    pub fn from_default_firefox() -> Self {
        match crate::config::firefox_profiles_root() {
            Some(root) => Self::from_firefox_root(root),
            None => Self::from_firefox_root(""),
        }
    }

    pub fn from_firefox_root(root: impl AsRef<Path>) -> Self {
        let mut available_cookies = discover_firefox_cookies(root.as_ref());
        available_cookies.sort_by(|l, r| l.id.cmp(&r.id));
        // No cap on the number of cookies — all discovered profiles are used.
        available_cookies =
            materialize_profiles_cache(Path::new(DEFAULT_CACHE_ROOT), available_cookies);
        Self {
            available_cookies,
            last_used_cookie: None,
            cooldown_list: Vec::new(),
            cooldown: DEFAULT_COOLDOWN,
            random_counter: 0,
        }
    }

    pub fn status(&mut self) -> CookiePoolStatus {
        self.cleanup_expired_cooldowns();
        CookiePoolStatus {
            available_cookies: self.available_cookies.len(),
            selectable_cookies: self.selectable_indexes().len(),
            cooldown_cookies: self.cooldown_list.len(),
            last_used_cookie: self.last_used_cookie.clone(),
            next_available_in: self.next_available_in(),
        }
    }

    pub fn snapshot(&mut self) -> CookiePoolSnapshot {
        self.cleanup_expired_cooldowns();
        CookiePoolSnapshot {
            available_cookies: self.available_cookies.clone(),
            last_used_cookie: self.last_used_cookie.clone(),
            cooldown_list: self.cooldown_list.clone(),
        }
    }

    pub fn restore_state(
        &mut self,
        last_used_cookie: Option<String>,
        cooldown_list: Vec<CooldownEntry>,
    ) {
        let available_ids = self
            .available_cookies
            .iter()
            .map(|c| c.id.as_str())
            .collect::<HashSet<_>>();
        self.last_used_cookie = last_used_cookie.filter(|id| available_ids.contains(id.as_str()));
        self.cooldown_list = cooldown_list
            .into_iter()
            .filter(|e| available_ids.contains(e.cookie_id.as_str()))
            .collect();
        self.cleanup_expired_cooldowns();
    }

    pub fn next_cookie(&mut self) -> Option<SelectedCookie> {
        self.cleanup_expired_cooldowns();
        let selectable = self.selectable_indexes();
        if selectable.is_empty() {
            return None;
        }
        let selected_index = selectable[self.random_index(selectable.len())];
        let selected = self.available_cookies[selected_index].clone();
        let yt_dlp_browser_spec = selected.yt_dlp_browser_spec();
        self.last_used_cookie = Some(selected.id.clone());
        Some(SelectedCookie {
            id: selected.id,
            profile_name: selected.profile_name,
            cookies_file: selected.cookies_sqlite,
            yt_dlp_browser_spec,
        })
    }

    #[allow(dead_code)]
    pub fn mark_rate_limited(&mut self, cookie_id: &str) -> bool {
        self.cleanup_expired_cooldowns();
        if self.cooldown_list.iter().any(|e| e.cookie_id == cookie_id) {
            return false;
        }
        if self.cooldown_list.len() >= self.available_cookies.len() {
            return false;
        }
        self.cooldown_list.push(CooldownEntry {
            cookie_id: cookie_id.to_owned(),
            expire_at: SystemTime::now() + self.cooldown,
        });
        true
    }

    pub fn mark_last_rate_limited(&mut self) -> Option<CookieSource> {
        let cookie_id = self.last_used_cookie.clone()?;
        self.cleanup_expired_cooldowns();
        if self.cooldown_list.iter().any(|e| e.cookie_id == cookie_id) {
            return None;
        }
        if self.cooldown_list.len() >= self.available_cookies.len() {
            return None;
        }
        // Use REFRESH_COOLDOWN as a safety net; the caller is expected to call
        // remove_from_cooldown() explicitly once the auto-refresh finishes.
        self.cooldown_list.push(CooldownEntry {
            cookie_id: cookie_id.clone(),
            expire_at: SystemTime::now() + REFRESH_COOLDOWN,
        });
        self.available_cookies
            .iter()
            .find(|c| c.id == cookie_id)
            .cloned()
    }

    /// Explicitly removes a cookie from cooldown (called after a triggered refresh completes).
    pub fn remove_from_cooldown(&mut self, cookie_id: &str) {
        self.cooldown_list.retain(|e| e.cookie_id != cookie_id);
    }

    fn selectable_indexes(&self) -> Vec<usize> {
        let cooldown_ids = self
            .cooldown_list
            .iter()
            .map(|e| e.cookie_id.as_str())
            .collect::<HashSet<_>>();
        let allow_last = self.available_cookies.len() == 1;
        self.available_cookies
            .iter()
            .enumerate()
            .filter_map(|(index, cookie)| {
                let is_last =
                    !allow_last && self.last_used_cookie.as_deref() == Some(cookie.id.as_str());
                let is_cooling_down = cooldown_ids.contains(cookie.id.as_str());
                (!is_last && !is_cooling_down).then_some(index)
            })
            .collect()
    }

    fn cleanup_expired_cooldowns(&mut self) {
        let now = SystemTime::now();
        self.cooldown_list.retain(|e| e.expire_at > now);
    }

    fn next_available_in(&self) -> Option<Duration> {
        self.cooldown_list
            .iter()
            .filter_map(|e| e.expire_at.duration_since(SystemTime::now()).ok())
            .min()
    }

    fn random_index(&mut self, len: usize) -> usize {
        self.random_counter = self.random_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        (nanos ^ self.random_counter.rotate_left(13)) as usize % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dummy_cookie(id: &str) -> CookieSource {
        CookieSource {
            id: id.to_string(),
            profile_name: format!("Profile {id}"),
            profile_dir: PathBuf::from(format!("/tmp/profile_{id}")),
            cookies_sqlite: PathBuf::from(format!("/tmp/profile_{id}/cookies.sqlite")),
            source_profile_dir: PathBuf::from(format!("/tmp/source_{id}")),
        }
    }

    #[test]
    fn test_single_cookie_pool_selection() {
        let mut pool = CookiePool {
            available_cookies: vec![dummy_cookie("cookie-1")],
            last_used_cookie: None,
            cooldown_list: Vec::new(),
            cooldown: DEFAULT_COOLDOWN,
            random_counter: 0,
        };

        // First call selects cookie-1
        let first = pool.next_cookie();
        assert!(first.is_some());
        assert_eq!(first.unwrap().id, "cookie-1");
        assert_eq!(pool.last_used_cookie.as_deref(), Some("cookie-1"));

        // Second call on single-cookie pool MUST still select cookie-1 (does not return None)
        let second = pool.next_cookie();
        assert!(second.is_some());
        assert_eq!(second.unwrap().id, "cookie-1");
    }

    #[test]
    fn test_multi_cookie_pool_selection() {
        let mut pool = CookiePool {
            available_cookies: vec![dummy_cookie("cookie-1"), dummy_cookie("cookie-2")],
            last_used_cookie: None,
            cooldown_list: Vec::new(),
            cooldown: DEFAULT_COOLDOWN,
            random_counter: 0,
        };

        let _first = pool.next_cookie().unwrap();
        let last_id = pool.last_used_cookie.clone().unwrap();

        // In a multi-cookie pool, the last used cookie should be excluded from selectable indexes
        let selectable = pool.selectable_indexes();
        assert_eq!(selectable.len(), 1);
        assert_ne!(pool.available_cookies[selectable[0]].id, last_id);
    }
}
