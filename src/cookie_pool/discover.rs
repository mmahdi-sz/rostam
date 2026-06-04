use std::{fs, path::{Path, PathBuf}};

use super::types::CookieSource;

pub(super) fn discover_firefox_cookies(root: &Path) -> Vec<CookieSource> {
    let mut cookies = discover_from_profiles_ini(root);
    if cookies.is_empty() {
        cookies = discover_from_profile_dirs(root);
    }
    cookies
}

pub(super) fn materialize_profiles_cache(
    cache_root: &Path,
    sources: Vec<CookieSource>,
) -> Vec<CookieSource> {
    if cache_root.exists() {
        if let Err(error) = fs::remove_dir_all(cache_root) {
            eprintln!("failed to clear cookie cache at {}: {error}", cache_root.display());
        }
    }
    if let Err(error) = fs::create_dir_all(cache_root) {
        eprintln!("failed to create cookie cache at {}: {error}", cache_root.display());
        return sources;
    }

    let mut copied = Vec::with_capacity(sources.len());

    for source in sources {
        let dest_profile = cache_root.join(&source.id);
        if let Err(error) = fs::create_dir_all(&dest_profile) {
            eprintln!("failed to create cache dir {}: {error}", dest_profile.display());
            continue;
        }

        let entries = match fs::read_dir(&source.profile_dir) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("failed to read profile dir {}: {error}", source.profile_dir.display());
                continue;
            }
        };

        let mut copied_any = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else { continue };
            if !name_str.starts_with("cookies.sqlite") { continue; }
            let src = entry.path();
            let dst = dest_profile.join(name_str);
            match fs::copy(&src, &dst) {
                Ok(_) => copied_any = true,
                Err(error) => eprintln!("failed to copy {} to {}: {error}", src.display(), dst.display()),
            }
        }

        if !copied_any { continue; }

        copied.push(CookieSource {
            id: source.id,
            profile_name: source.profile_name,
            cookies_sqlite: dest_profile.join("cookies.sqlite"),
            source_profile_dir: source.source_profile_dir,
            profile_dir: dest_profile,
        });
    }

    copied
}

fn discover_from_profiles_ini(root: &Path) -> Vec<CookieSource> {
    let profiles_ini = root.join("profiles.ini");
    let Ok(contents) = fs::read_to_string(profiles_ini) else { return Vec::new() };

    let mut profiles = Vec::new();
    let mut name = String::new();
    let mut path = String::new();
    let mut is_relative = true;

    for line in contents.lines().map(str::trim).chain([""]) {
        if line.starts_with('[') {
            push_profile(root, &mut profiles, &name, &path, is_relative);
            name.clear(); path.clear(); is_relative = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match key {
            "Name" => name = value.to_owned(),
            "Path" => path = value.to_owned(),
            "IsRelative" => is_relative = value != "0",
            _ => {}
        }
    }

    profiles
}

fn push_profile(root: &Path, profiles: &mut Vec<CookieSource>, name: &str, profile_path: &str, is_relative: bool) {
    if profile_path.is_empty() { return; }
    let profile_dir = if is_relative { root.join(profile_path) } else { PathBuf::from(profile_path) };
    let cookies_sqlite = profile_dir.join("cookies.sqlite");
    if !cookies_sqlite.is_file() { return; }
    let id = profile_dir.file_name().and_then(|f| f.to_str()).unwrap_or(profile_path).to_owned();
    profiles.push(CookieSource {
        profile_name: if name.is_empty() { id.clone() } else { name.to_owned() },
        source_profile_dir: profile_dir.clone(),
        id, profile_dir, cookies_sqlite,
    });
}

fn discover_from_profile_dirs(root: &Path) -> Vec<CookieSource> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("cookies.sqlite").is_file())
        .filter_map(|profile_dir| {
            let id = profile_dir.file_name()?.to_str()?.to_owned();
            Some(CookieSource {
                profile_name: id.clone(),
                cookies_sqlite: profile_dir.join("cookies.sqlite"),
                source_profile_dir: profile_dir.clone(),
                profile_dir,
                id,
            })
        })
        .collect()
}
