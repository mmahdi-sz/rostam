use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DEFAULT_FIREFOX_ROOT: &str = "/home/mahdi/.mozilla/firefox";
const DEFAULT_MAX_COOKIES: usize = 20;
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(20 * 60 * 60);

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
pub struct CookiePool {
    available_cookies: Vec<CookieSource>,
    last_used_cookie: Option<String>,
    cooldown_list: Vec<CooldownEntry>,
    cooldown: Duration,
    random_counter: u64,
}

#[derive(Debug)]
pub struct CookiePoolStatus {
    pub available_cookies: usize,
    pub selectable_cookies: usize,
    pub cooldown_cookies: usize,
    pub last_used_cookie: Option<String>,
    pub next_available_in: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct CookiePoolSnapshot {
    pub available_cookies: Vec<CookieSource>,
    pub last_used_cookie: Option<String>,
    pub cooldown_list: Vec<CooldownEntry>,
}

impl CookiePool {
    pub fn from_default_firefox() -> Self {
        Self::from_firefox_root(DEFAULT_FIREFOX_ROOT)
    }

    pub fn from_firefox_root(root: impl AsRef<Path>) -> Self {
        let mut available_cookies = discover_firefox_cookies(root.as_ref());
        available_cookies.sort_by(|left, right| left.id.cmp(&right.id));
        available_cookies.truncate(DEFAULT_MAX_COOKIES);

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
            .map(|cookie| cookie.id.as_str())
            .collect::<HashSet<_>>();

        self.last_used_cookie = last_used_cookie
            .filter(|cookie_id| available_ids.contains(cookie_id.as_str()));
        self.cooldown_list = cooldown_list
            .into_iter()
            .filter(|entry| available_ids.contains(entry.cookie_id.as_str()))
            .take(DEFAULT_MAX_COOKIES)
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

    pub fn mark_rate_limited(&mut self, cookie_id: &str) -> bool {
        self.cleanup_expired_cooldowns();

        if self.cooldown_list.iter().any(|entry| entry.cookie_id == cookie_id) {
            return false;
        }

        if self.cooldown_list.len() >= DEFAULT_MAX_COOKIES {
            return false;
        }

        self.cooldown_list.push(CooldownEntry {
            cookie_id: cookie_id.to_owned(),
            expire_at: SystemTime::now() + self.cooldown,
        });

        true
    }

    pub fn mark_last_rate_limited(&mut self) -> Option<bool> {
        let cookie_id = self.last_used_cookie.clone()?;
        Some(self.mark_rate_limited(&cookie_id))
    }

    fn selectable_indexes(&self) -> Vec<usize> {
        let cooldown_ids = self
            .cooldown_list
            .iter()
            .map(|entry| entry.cookie_id.as_str())
            .collect::<HashSet<_>>();

        self.available_cookies
            .iter()
            .enumerate()
            .filter_map(|(index, cookie)| {
                let is_last = self.last_used_cookie.as_deref() == Some(cookie.id.as_str());
                let is_cooling_down = cooldown_ids.contains(cookie.id.as_str());

                (!is_last && !is_cooling_down).then_some(index)
            })
            .collect()
    }

    fn cleanup_expired_cooldowns(&mut self) {
        let now = SystemTime::now();
        self.cooldown_list.retain(|entry| entry.expire_at > now);
    }

    fn next_available_in(&self) -> Option<Duration> {
        self.cooldown_list
            .iter()
            .filter_map(|entry| entry.expire_at.duration_since(SystemTime::now()).ok())
            .min()
    }

    fn random_index(&mut self, len: usize) -> usize {
        self.random_counter = self.random_counter.wrapping_add(1);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);

        (nanos ^ self.random_counter.rotate_left(13)) as usize % len
    }
}

fn discover_firefox_cookies(root: &Path) -> Vec<CookieSource> {
    let mut cookies = discover_from_profiles_ini(root);

    if cookies.is_empty() {
        cookies = discover_from_profile_dirs(root);
    }

    cookies
}

fn discover_from_profiles_ini(root: &Path) -> Vec<CookieSource> {
    let profiles_ini = root.join("profiles.ini");
    let Ok(contents) = fs::read_to_string(profiles_ini) else {
        return Vec::new();
    };

    let mut profiles = Vec::new();
    let mut name = String::new();
    let mut path = String::new();
    let mut is_relative = true;

    for line in contents.lines().map(str::trim).chain([""]) {
        if line.starts_with('[') {
            push_profile(root, &mut profiles, &name, &path, is_relative);
            name.clear();
            path.clear();
            is_relative = true;
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key {
            "Name" => name = value.to_owned(),
            "Path" => path = value.to_owned(),
            "IsRelative" => is_relative = value != "0",
            _ => {}
        }
    }

    profiles
}

fn push_profile(
    root: &Path,
    profiles: &mut Vec<CookieSource>,
    name: &str,
    profile_path: &str,
    is_relative: bool,
) {
    if profile_path.is_empty() {
        return;
    }

    let profile_dir = if is_relative {
        root.join(profile_path)
    } else {
        PathBuf::from(profile_path)
    };

    let cookies_sqlite = profile_dir.join("cookies.sqlite");

    if !cookies_sqlite.is_file() {
        return;
    }

    let id = profile_dir
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or(profile_path)
        .to_owned();

    profiles.push(CookieSource {
        profile_name: if name.is_empty() {
            id.clone()
        } else {
            name.to_owned()
        },
        id,
        profile_dir,
        cookies_sqlite,
    });
}

fn discover_from_profile_dirs(root: &Path) -> Vec<CookieSource> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("cookies.sqlite").is_file())
        .filter_map(|profile_dir| {
            let id = profile_dir.file_name()?.to_str()?.to_owned();

            Some(CookieSource {
                profile_name: id.clone(),
                cookies_sqlite: profile_dir.join("cookies.sqlite"),
                profile_dir,
                id,
            })
        })
        .collect()
}
