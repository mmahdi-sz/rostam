//! Fetch SoundCloud track metadata using `yt-dlp`.

use anyhow::{Context, anyhow};
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct SoundcloudTrackMeta {
    pub title: String,
    pub artist: String,
    pub duration_secs: u64,
    pub thumbnail_url: Option<String>,
    pub release_date: Option<String>,
}

const DUMP_TIMEOUT: Duration = Duration::from_secs(30);
/// Flat playlist dumps of a few hundred tracks take far longer than one track.
const SET_DUMP_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub struct SoundcloudSet {
    pub title: String,
    /// Per-track permalinks — flat entries carry no title, so each track's
    /// metadata is fetched at download time by the normal single-track path.
    pub track_urls: Vec<String>,
}

/// List the tracks of a SoundCloud set (`/{artist}/sets/{name}`).
///
/// `--flat-playlist` keeps this one HTTP round-trip instead of one per track.
pub async fn fetch_soundcloud_set(trace_id: u64, url: &str) -> anyhow::Result<SoundcloudSet> {
    log_ev!("sc", trace_id, "fetch_set_enter", "url" => url);

    let child = Command::new("yt-dlp")
        .arg("--dump-single-json")
        .arg("--flat-playlist")
        .arg("--no-download")
        .arg("--no-warnings")
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Failed to spawn yt-dlp for SoundCloud set dump")?;

    let output = match tokio::time::timeout(SET_DUMP_TIMEOUT, child.wait_with_output()).await {
        Ok(res) => res.context("Failed to wait on yt-dlp process")?,
        Err(_) => {
            log_ev!("sc", trace_id, "fetch_set_fail", "err" => "timeout");
            return Err(anyhow!(
                "yt-dlp timed out after {}s",
                SET_DUMP_TIMEOUT.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first = stderr.lines().next().unwrap_or("").to_string();
        log_ev!("sc", trace_id, "fetch_set_fail", "err" => &first);
        return Err(anyhow!("yt-dlp exited with error: {first}"));
    }

    let json_val: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse yt-dlp JSON for SoundCloud set")?;

    let title = json_val
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("SoundCloud Set")
        .to_string();

    let track_urls: Vec<String> = json_val
        .get("entries")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e.get("url").and_then(|v| v.as_str()).map(String::from))
        .collect();

    if track_urls.is_empty() {
        log_ev!("sc", trace_id, "fetch_set_fail", "err" => "no entries");
        return Err(anyhow!("SoundCloud set has no playable tracks"));
    }

    log_ev!("sc", trace_id, "fetch_set_ok", "title" => &title, "tracks" => track_urls.len());
    Ok(SoundcloudSet { title, track_urls })
}

pub async fn fetch_soundcloud_meta(
    trace_id: u64,
    url: &str,
) -> anyhow::Result<SoundcloudTrackMeta> {
    log_ev!("sc", trace_id, "fetch_meta_enter", "url" => url);

    let child = Command::new("yt-dlp")
        .arg("--dump-single-json")
        .arg("--no-download")
        .arg("--no-warnings")
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Failed to spawn yt-dlp for SoundCloud metadata dump")?;

    let output = match tokio::time::timeout(DUMP_TIMEOUT, child.wait_with_output()).await {
        Ok(res) => res.context("Failed to wait on yt-dlp process")?,
        Err(_) => {
            return Err(anyhow!(
                "yt-dlp timed out after {}s",
                DUMP_TIMEOUT.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log_ev!("sc", trace_id, "fetch_meta_fail", "err" => stderr.lines().next().unwrap_or(""));
        return Err(anyhow!(
            "yt-dlp exited with error: {}",
            stderr.lines().next().unwrap_or("")
        ));
    }

    let json_val: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse yt-dlp JSON output for SoundCloud track")?;

    let title = json_val
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled Track")
        .to_string();

    let artist = json_val
        .get("uploader")
        .or_else(|| json_val.get("artist"))
        .or_else(|| json_val.get("creator"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown Artist")
        .to_string();

    let duration_secs = json_val
        .get("duration")
        .and_then(|v| v.as_f64())
        .map(|d| d as u64)
        .unwrap_or(0);

    let raw_thumb = json_val
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let thumbnail_url = raw_thumb.map(|url| normalize_sc_artwork(&url));

    let raw_date = json_val
        .get("release_date")
        .or_else(|| json_val.get("upload_date"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let release_date = raw_date.map(|d| {
        if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) {
            format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8])
        } else {
            d
        }
    });

    log_ev!(
        "sc",
        trace_id,
        "fetch_meta_ok",
        "title" => &title,
        "artist" => &artist,
        "duration" => duration_secs
    );

    Ok(SoundcloudTrackMeta {
        title,
        artist,
        duration_secs,
        thumbnail_url,
        release_date,
    })
}

/// yt-dlp reports the `-original.jpg` artwork (often >1MB), which Telegram
/// silently drops as an audio thumbnail (cap: 320px / 200KB) — so the track
/// shows up with no/stale cover. Rewrite *any* sndcdn size suffix to t500x500.
/// ponytail: plain suffix surgery, no regex — sndcdn URLs are always
/// `artworks-<id>-<size>.<ext>`.
fn normalize_sc_artwork(url: &str) -> String {
    if !url.contains("sndcdn.com/artworks-") {
        return url.to_string();
    }
    let Some((stem, ext)) = url.rsplit_once('.') else {
        return url.to_string();
    };
    match stem.rsplit_once('-') {
        Some((base, _size)) => format!("{base}-t500x500.{ext}"),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_sc_artwork;

    #[test]
    fn rewrites_any_size_suffix() {
        for size in ["original", "large", "t300x300", "t500x500", "crop"] {
            assert_eq!(
                normalize_sc_artwork(&format!("https://i1.sndcdn.com/artworks-abc-{size}.jpg")),
                "https://i1.sndcdn.com/artworks-abc-t500x500.jpg"
            );
        }
    }

    #[test]
    fn leaves_foreign_urls_alone() {
        let other = "https://example.com/cover-original.jpg";
        assert_eq!(normalize_sc_artwork(other), other);
    }
}
