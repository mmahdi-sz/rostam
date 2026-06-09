use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::SendDocumentParams,
};

use crate::i18n::tf;

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
    let raw_path = dir.join("thumb_raw");
    let jpg_path = dir.join("thumb.jpg");

    let resp = match reqwest::get(url).await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => { log_trace(trace_id, "thumb_http_error", &format!("status={}", r.status())); return None; }
        Err(e) => { log_trace(trace_id, "thumb_fetch_failed", &e.to_string()); return None; }
    };
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => { log_trace(trace_id, "thumb_bytes_failed", &e.to_string()); return None; }
    };
    if tokio::fs::write(&raw_path, &bytes).await.is_err() {
        log_trace(trace_id, "thumb_write_failed", url);
        return None;
    }
    log_trace(trace_id, "thumb_fetched", &format!("bytes={} raw={}", bytes.len(), raw_path.display()));

    // YouTube often returns WebP; convert to JPEG so Telegram accepts it as a thumbnail.
    let ffmpeg_out = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-i", &raw_path.to_string_lossy(),
               "-vf", "scale=320:-1", "-q:v", "2",
               &jpg_path.to_string_lossy()])
        .output()
        .await;

    match ffmpeg_out {
        Ok(out) if out.status.success() => {
            log_trace(trace_id, "thumb_converted", &jpg_path.display().to_string());
            Some(jpg_path.to_string_lossy().into_owned())
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            log_trace(trace_id, "thumb_convert_failed", &format!("ffmpeg: {stderr}"));
            None
        }
        Err(e) => { log_trace(trace_id, "thumb_convert_spawn_failed", &e.to_string()); None }
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

/// Finds subtitle files (.srt/.vtt) produced by yt-dlp in `dir` and sends each
/// to the user as a document. Used in SubtitleMode::File. Returns how many were sent.
pub async fn send_subtitle_files(
    api: &Bot,
    dir: &std::path::Path,
    chat_id: i64,
    video_title: &str,
    trace_id: u64,
) -> usize {
    let mut sent = 0usize;
    let Ok(entries) = std::fs::read_dir(dir) else { return 0; };
    let mut subs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "srt" || e == "vtt"
                })
                .unwrap_or(false)
        })
        .collect();
    subs.sort();
    for sub_path in &subs {
        let fname = sub_path.file_name().and_then(|n| n.to_str()).unwrap_or("subtitle");
        // Try to surface the language tag in the caption (e.g. "video.fa.srt" -> "fa").
        let lang = sub_path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit('.').next())
            .unwrap_or("")
            .to_string();
        let caption = tf("youtube.download.subtitle_caption", &[
            ("title", video_title), ("lang", &lang),
        ]);
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(FileUpload::InputFile(InputFile { path: sub_path.clone() }))
            .caption(caption)
            .build();
        match api.send_document(&params).await {
            Ok(_) => {
                sent += 1;
                log_trace(trace_id, "subtitle_file_sent", &format!("file={fname} lang={lang}"));
            }
            Err(e) => log_trace(trace_id, "subtitle_file_failed", &format!("file={fname} err={e}")),
        }
    }
    log_trace(trace_id, "subtitle_files_done", &format!("sent={sent} found={}", subs.len()));
    sent
}
