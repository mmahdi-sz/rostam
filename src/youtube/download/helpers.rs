use std::path::PathBuf;

use super::super::trace::log_trace;

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
    let path = dir.join("thumb.jpg");
    match reqwest::get(url).await {
        Ok(resp) if resp.status().is_success() => {
            match resp.bytes().await {
                Ok(bytes) => {
                    if tokio::fs::write(&path, &bytes).await.is_ok() {
                        log_trace(
                            trace_id,
                            "thumb_fetched",
                            &format!("bytes={} path={}", bytes.len(), path.display()),
                        );
                        Some(path.to_string_lossy().into_owned())
                    } else {
                        log_trace(trace_id, "thumb_write_failed", url);
                        None
                    }
                }
                Err(e) => { log_trace(trace_id, "thumb_bytes_failed", &e.to_string()); None }
            }
        }
        Ok(resp) => { log_trace(trace_id, "thumb_http_error", &format!("status={}", resp.status())); None }
        Err(e) => { log_trace(trace_id, "thumb_fetch_failed", &e.to_string()); None }
    }
}

pub fn quality_label_for(height: u32) -> String {
    let key = format!("youtube.quality.buttons.{height}");
    let label = crate::i18n::t(&key);
    if label.starts_with('!') {
        format!("{height}p")
    } else {
        label
    }
}
