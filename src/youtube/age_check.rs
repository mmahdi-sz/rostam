use std::{
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::Duration,
};

use tokio::sync::Mutex;

use crate::cookie_pool::{CookiePool, CookieSource, SelectedCookie};

use super::{
    fetch::{fetch_video_info, fetch_video_info_with_cookie},
    trace::log_trace,
    types::{FetchError, VideoInfo},
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(48 * 60 * 60);
const MAX_RETRIES: usize = 2;
type Verified = HashMap<String, u64>;

#[cfg(feature = "testapi")]
tokio::task_local! {
    static TEST_VERIFIED: Verified;
}

fn cache_key() -> String {
    format!("youtube:age_verified:v1:{}", crate::config::env_label())
}

async fn read_cache() -> anyhow::Result<Verified> {
    #[cfg(feature = "testapi")]
    if let Ok(verified) = TEST_VERIFIED.try_with(Clone::clone) {
        return Ok(verified);
    }
    let mut conn = crate::force_join::conn::conn().await?;
    let data: Option<String> = redis::cmd("GET")
        .arg(cache_key())
        .query_async(&mut conn)
        .await?;
    Ok(data
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_default())
}

async fn fingerprint(profile: &CookieSource) -> Option<u64> {
    let mut hash = DefaultHasher::new();
    tokio::fs::read(&profile.cookies_sqlite)
        .await
        .ok()?
        .hash(&mut hash);
    match tokio::fs::read(profile.cookies_sqlite.with_extension("sqlite-wal")).await {
        Ok(wal) => wal.hash(&mut hash),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    Some(hash.finish())
}

pub fn spawn(pool: Arc<Mutex<CookiePool>>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let trace = crate::log::next_trace_id();
            if let Err(err) = refresh(trace, &pool).await {
                log_trace(trace, "age_check_refresh_failed", &err.to_string());
            }
        }
    });
}

async fn refresh(trace: u64, pool: &Mutex<CookiePool>) -> anyhow::Result<()> {
    let urls = tokio::fs::read_to_string("files/age_check_urls.txt").await?;
    let mut reference = None;
    for line in urls.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let urls = super::extract::extract_youtube_urls(line);
        let [url] = urls.as_slice() else { continue };
        if matches!(
            fetch_video_info_with_cookie(trace, url, None).await,
            Err(FetchError::AgeRestricted(_))
        ) {
            reference = Some(url.clone());
            break;
        }
    }
    let url = reference.ok_or_else(|| anyhow::anyhow!("no age-restricted reference available"))?;
    CookiePool::discover_new_profiles(pool).await?;
    let profiles = pool.lock().await.snapshot().available_cookies;
    let mut verified = read_cache().await?;
    verified.retain(|id, _| profiles.iter().any(|profile| profile.id == *id));

    for profile in profiles {
        let Some(version) = fingerprint(&profile).await else {
            continue;
        };
        let result = fetch_video_info(trace, &url, &profile.yt_dlp_browser_spec()).await;
        if fingerprint(&profile).await != Some(version) {
            continue;
        }
        match result {
            Ok(_) => {
                verified.insert(profile.id, version);
            }
            Err(FetchError::AgeRestricted(_)) => {
                verified.remove(&profile.id);
            }
            Err(_) => {}
        }
    }

    let mut conn = crate::force_join::conn::conn().await?;
    redis::cmd("SET")
        .arg(cache_key())
        .arg(serde_json::to_string(&verified)?)
        .query_async::<()>(&mut conn)
        .await?;
    log_trace(
        trace,
        "age_check_refresh_done",
        &format!("verified={}", verified.len()),
    );
    Ok(())
}

pub(super) async fn retry(
    trace: u64,
    url: &str,
    pool: &Mutex<CookiePool>,
    tried: &mut HashSet<String>,
) -> Option<(SelectedCookie, VideoInfo)> {
    let verified = match read_cache().await {
        Ok(verified) => verified,
        Err(err) => {
            log_trace(trace, "age_retry_cache_failed", &err.to_string());
            return None;
        }
    };
    let profiles = pool.lock().await.snapshot().available_cookies;
    let mut eligible = HashSet::new();
    for profile in profiles {
        if let Some(&version) = verified.get(&profile.id) {
            if !tried.contains(&profile.id) && fingerprint(&profile).await == Some(version) {
                eligible.insert(profile.id);
            }
        }
    }

    for _ in 0..MAX_RETRIES {
        let cookie = pool.lock().await.next_age_cookie(tried, &eligible)?;
        tried.insert(cookie.id.clone());
        log_trace(
            trace,
            "age_retry_fetch",
            &format!("attempted_profiles={}", tried.len()),
        );
        match fetch_video_info(trace, url, &cookie.yt_dlp_browser_spec).await {
            Ok(info) => return Some((cookie, info)),
            Err(_) => log_trace(trace, "age_retry_fetch_failed", "=> fail"),
        }
    }
    None
}

#[cfg(feature = "testapi")]
pub(crate) async fn test_retry(trace: u64, succeed: bool) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let dir = crate::common::dir::TempDirGuard::create("test_youtube_age", trace)?;
    let command = dir.path().join("yt-dlp");
    tokio::fs::write(&command, r#"#!/bin/sh
case "$*" in
  *profile-a*) printf '%s\n' 'ERROR: Sign in to confirm your age.' >&2; exit 1 ;;
  *profile-b*) printf '%s\n' '{"title":"Age retry fixture","webpage_url":"https://youtu.be/guTlhQcDtMk","formats":[{"format_id":"18","height":360,"vcodec":"avc1","acodec":"mp4a"}]}' ;;
  *) exit 2 ;;
esac
"#).await?;
    tokio::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).await?;
    let mut profiles = Vec::new();
    for id in ["profile-a", "profile-b"] {
        let path = dir.path().join(id);
        tokio::fs::create_dir(&path).await?;
        let cookies_sqlite = path.join("cookies.sqlite");
        tokio::fs::write(&cookies_sqlite, id).await?;
        profiles.push(CookieSource {
            id: id.into(),
            profile_name: id.into(),
            profile_dir: path.clone(),
            source_profile_dir: path,
            cookies_sqlite,
        });
    }
    let version = fingerprint(&profiles[1])
        .await
        .ok_or_else(|| anyhow::anyhow!("missing fixture"))?;
    let verified = HashMap::from([(
        profiles[1].id.clone(),
        if succeed { version } else { !version },
    )]);
    let mut pool = CookiePool::with_test_profiles(profiles);
    // Keep the initially selected cookie deterministic.
    pool.restore_state(Some("profile-b".into()), Vec::new());
    let pool = Arc::new(Mutex::new(pool));
    let port = std::env::var("TESTAPI_PORT").unwrap_or_else(|_| "14379".into());
    let api = frankenstein::client_reqwest::Bot::new_url(format!(
        "http://127.0.0.1:{port}/botdummy_token"
    ));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    TEST_VERIFIED
        .scope(
            verified,
            super::fetch::TEST_YTDLP.scope(
                command,
                super::handle_youtube_url(
                    &api,
                    12345,
                    1,
                    Some(12345),
                    trace,
                    "https://youtu.be/guTlhQcDtMk",
                    pool.clone(),
                    &None,
                    &tx,
                ),
            ),
        )
        .await?;
    let chosen = pool.lock().await.snapshot().last_used_cookie;
    anyhow::ensure!(
        chosen.as_deref() == Some(if succeed { "profile-b" } else { "profile-a" }),
        "unexpected selected cookie"
    );
    Ok(())
}
