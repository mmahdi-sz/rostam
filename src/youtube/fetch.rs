use std::collections::HashSet;

use tokio::process::Command;

use super::types::{FetchError, VideoInfo};

pub async fn fetch_video_info(
    url: &str,
    yt_dlp_browser_spec: &str,
) -> Result<VideoInfo, FetchError> {
    let output = Command::new("yt-dlp")
        .arg("--cookies-from-browser")
        .arg(yt_dlp_browser_spec)
        .arg("--dump-single-json")
        .arg("--no-download")
        .arg("--no-warnings")
        .arg("--no-playlist")
        .arg("--ignore-no-formats-error")
        .arg(url)
        .output()
        .await
        .map_err(|e| FetchError::Other(format!("failed to spawn yt-dlp: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("http error 429") || lower.contains("too many requests") {
            return Err(FetchError::RateLimited);
        }
        if lower.contains("no such table: moz_cookies")
            || lower.contains("database is locked")
            || lower.contains("could not find cookies")
            || lower.contains("unable to open database file")
            || lower.contains("no cookies found")
        {
            return Err(FetchError::BadCookie(
                stderr.lines().last().unwrap_or("").to_string(),
            ));
        }
        return Err(FetchError::Other(format!(
            "yt-dlp exited with status {}: {}",
            output.status,
            stderr.lines().last().unwrap_or("").to_string()
        )));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| FetchError::Other(format!("failed to parse yt-dlp json: {e}")))?;

    let title = json.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let channel = json
        .get("channel")
        .or_else(|| json.get("uploader"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let duration = json.get("duration").and_then(|v| v.as_f64()).map(|d| d as u64);
    let view_count = json.get("view_count").and_then(|v| v.as_u64());
    let like_count = json.get("like_count").and_then(|v| v.as_u64());
    let upload_date = json.get("upload_date").and_then(|v| v.as_str()).map(|s| s.to_string());
    let thumbnail = json.get("thumbnail").and_then(|v| v.as_str()).map(|s| s.to_string());
    let webpage_url = json
        .get("webpage_url")
        .and_then(|v| v.as_str())
        .unwrap_or(url)
        .to_string();
    let description = json
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty());
    let available_heights = extract_available_heights(&json);

    Ok(VideoInfo {
        title,
        channel,
        duration,
        view_count,
        like_count,
        upload_date,
        thumbnail,
        webpage_url,
        description,
        available_heights,
    })
}

fn extract_available_heights(json: &serde_json::Value) -> Vec<u32> {
    let mut heights = HashSet::new();
    if let Some(formats) = json.get("formats").and_then(|v| v.as_array()) {
        for format in formats {
            let Some(height) = format.get("height").and_then(|v| v.as_u64()) else {
                continue;
            };
            if format.get("vcodec").and_then(|v| v.as_str()) == Some("none") {
                continue;
            }
            if height <= u32::MAX as u64 {
                heights.insert(height as u32);
            }
        }
    }
    let mut heights: Vec<u32> = heights.into_iter().collect();
    heights.sort_unstable();
    heights
}
