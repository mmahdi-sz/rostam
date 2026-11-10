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

/// Reads the delivered codec and remuxes H.264 into Telegram's MP4 container.
pub async fn prepare_video_output(
    path: &str,
    trace_id: u64,
) -> anyhow::Result<(String, crate::common::ffmpeg::MediaMetadata)> {
    let mut output_path = path.to_string();
    let mut meta = crate::common::ffmpeg::probe_metadata(std::path::Path::new(path)).await?;
    if meta.codec == "h264" && !path.ends_with(".mp4") {
        let output = std::path::Path::new(path).with_extension("telegram.mp4");
        log_trace(trace_id, "output_remux", "h264 => mp4");
        let result = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-i",
                path,
                "-map",
                "0:v:0",
                "-map",
                "0:a?",
                "-map",
                "0:s?",
                "-c",
                "copy",
                "-c:s",
                "mov_text",
                "-movflags",
                "+faststart",
            ])
            .arg(&output)
            .kill_on_drop(true)
            .output()
            .await?;
        anyhow::ensure!(
            result.status.success(),
            "H.264 MP4 remux failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        meta = crate::common::ffmpeg::probe_metadata(&output).await?;
        output_path = output.to_string_lossy().into_owned();
    }
    log_trace(
        trace_id,
        "output_probed",
        &format!(
            "codec={} height={} bitrate={} path={} => ok",
            meta.codec, meta.height, meta.bitrate, meta.filename
        ),
    );
    Ok((output_path, meta))
}

pub fn output_codec_label(codec: &str) -> String {
    let key = if codec == "hevc" { "h265" } else { codec };
    crate::youtube::types::VideoCodec::from_key(key)
        .map(|codec| crate::i18n::t(codec.label_key()))
        .unwrap_or_else(|| codec.to_string())
}
