use std::path::PathBuf;

use crate::youtube::trace::log_trace;

#[allow(unused_imports)]
pub use super::cpu::*;
pub use super::notice::*;
pub use super::sanitize::*;
pub use super::subtitle::*;

pub fn pick_largest_file(dir: &std::path::Path) -> Option<String> {
    let mut best: Option<(u64, PathBuf)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                let size = meta.len();
                if best.as_ref().map(|(s, _)| size > *s).unwrap_or(true) {
                    best = Some((size, entry.path()));
                }
            }
        }
    }
    best.map(|(_, p)| p.to_string_lossy().into_owned())
}

pub async fn cleanup_dir(dir: &std::path::Path, trace_id: u64) {
    match tokio::fs::remove_dir_all(dir).await {
        Ok(_) => log_trace(trace_id, "cleanup_ok", &dir.display().to_string()),
        Err(e) => log_trace(trace_id, "cleanup_failed", &e.to_string()),
    }
}

pub async fn fetch_thumbnail(
    url: &Option<String>,
    dir: &std::path::Path,
    trace_id: u64,
) -> Option<String> {
    let url = url.as_deref()?;
    let raw_path = dir.join("thumb_raw");
    let jpg_path = dir.join("thumb.jpg");

    let resp = match reqwest::get(url).await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            log_trace(
                trace_id,
                "thumb_http_error",
                &format!("status={}", r.status()),
            );
            return None;
        }
        Err(e) => {
            log_trace(trace_id, "thumb_fetch_failed", &e.to_string());
            return None;
        }
    };
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            log_trace(trace_id, "thumb_bytes_failed", &e.to_string());
            return None;
        }
    };
    if tokio::fs::write(&raw_path, &bytes).await.is_err() {
        log_trace(trace_id, "thumb_write_failed", url);
        return None;
    }
    log_trace(
        trace_id,
        "thumb_fetched",
        &format!("bytes={} raw={}", bytes.len(), raw_path.display()),
    );

    // YouTube often returns WebP; convert to JPEG so Telegram accepts it as a thumbnail.
    let ffmpeg_out = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            &raw_path.to_string_lossy(),
            "-vf",
            "scale=320:-1",
            "-q:v",
            "2",
            &jpg_path.to_string_lossy(),
        ])
        .output()
        .await;

    match ffmpeg_out {
        Ok(out) if out.status.success() => {
            log_trace(trace_id, "thumb_converted", &jpg_path.display().to_string());
            Some(jpg_path.to_string_lossy().into_owned())
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            log_trace(
                trace_id,
                "thumb_convert_failed",
                &format!("ffmpeg: {stderr}"),
            );
            None
        }
        Err(e) => {
            log_trace(trace_id, "thumb_convert_spawn_failed", &e.to_string());
            None
        }
    }
}
