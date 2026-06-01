use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DEFAULT_FIREFOX_ROOT: &str = "/home/mahdi/.mozilla/firefox";
const DEFAULT_MAX_COOKIES: usize = 20;
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(20 * 60 * 60);
const DEFAULT_CACHE_ROOT: &str = "cookie_profiles_cache";

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
        available_cookies = materialize_profiles_cache(
            Path::new(DEFAULT_CACHE_ROOT),
            available_cookies,
        );

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

fn materialize_profiles_cache(
    cache_root: &Path,
    sources: Vec<CookieSource>,
) -> Vec<CookieSource> {
    if cache_root.exists() {
        if let Err(error) = fs::remove_dir_all(cache_root) {
            eprintln!(
                "failed to clear cookie cache at {}: {error}",
                cache_root.display()
            );
        }
    }
    if let Err(error) = fs::create_dir_all(cache_root) {
        eprintln!(
            "failed to create cookie cache at {}: {error}",
            cache_root.display()
        );
        return sources;
    }

    let mut copied = Vec::with_capacity(sources.len());

    for source in sources {
        let dest_profile = cache_root.join(&source.id);
        if let Err(error) = fs::create_dir_all(&dest_profile) {
            eprintln!(
                "failed to create cache dir {}: {error}",
                dest_profile.display()
            );
            continue;
        }

        let entries = match fs::read_dir(&source.profile_dir) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "failed to read profile dir {}: {error}",
                    source.profile_dir.display()
                );
                continue;
            }
        };

        let mut copied_any = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if !name_str.starts_with("cookies.sqlite") {
                continue;
            }
            let src = entry.path();
            let dst = dest_profile.join(name_str);
            match fs::copy(&src, &dst) {
                Ok(_) => copied_any = true,
                Err(error) => eprintln!(
                    "failed to copy {} to {}: {error}",
                    src.display(),
                    dst.display()
                ),
            }
        }

        if !copied_any {
            continue;
        }

        copied.push(CookieSource {
            id: source.id,
            profile_name: source.profile_name,
            cookies_sqlite: dest_profile.join("cookies.sqlite"),
            profile_dir: dest_profile,
        });
    }

    copied
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

use crate::database::postgresql::PostgresDatabase;
use crate::i18n::{t, tf};

pub fn format_cookie_status(status: &CookiePoolStatus) -> String {
    let last_used = status.last_used_cookie.as_deref().unwrap_or("-");
    let wait = status
        .next_available_in
        .map(format_duration)
        .unwrap_or_else(|| "-".to_owned());
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
    let wait = status
        .next_available_in
        .map(format_duration)
        .unwrap_or_else(|| "20h".to_owned());
    tf("cookie.none_available", &[("wait", &wait)])
}

pub fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

pub async fn save_snapshot(database: &Option<PostgresDatabase>, cookie_pool: &mut CookiePool) {
    let Some(db) = database else { return };
    if let Err(error) = db.save_snapshot(&cookie_pool.snapshot()).await {
        eprintln!("failed to save cookie pool snapshot: {error}");
    }
}
