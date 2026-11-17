use std::path::PathBuf;

use frankenstein::{AsyncTelegramApi, client_reqwest::Bot, methods::EditMessageTextParams};

use crate::i18n::tf;

use super::log_trace;

pub struct TempFileGuard(pub PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

pub async fn download_file(
    api: &Bot,
    file_id: &str,
    trace_id: u64,
) -> crate::error::Result<(Vec<u8>, crate::bot::TransferResult)> {
    let tmp_path = std::env::temp_dir().join(format!("sep_dl_{trace_id}.tmp"));
    let _guard = TempFileGuard(tmp_path.clone());
    let res = crate::bot::download_telegram_file(api, file_id, &tmp_path)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let bytes = std::fs::read(&tmp_path)?;
    log_trace(
        trace_id,
        "http_done",
        &format!("bytes={} speed={}", bytes.len(), res.speed_human()),
    );
    Ok((bytes, res))
}

pub async fn extract_and_prepare_audio(
    video_bytes: &[u8],
    tmp_dir: &std::path::Path,
    message_id: i32,
    chat_id: i64,
    api: &Bot,
    trace_id: u64,
) -> crate::error::Result<Vec<u8>> {
    const MAX_AUDIO_BYTES: u64 = 50 * 1024 * 1024;

    let video_path = tmp_dir.join("input_video");
    std::fs::write(&video_path, video_bytes)?;

    // Extract audio as MP3 at 320kbps.
    let audio_path = tmp_dir.join("extracted.mp3");
    let video_path_str = video_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid UTF-8 path"))?;
    let audio_path_str = audio_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid UTF-8 path"))?;

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            video_path_str,
            "-vn",
            "-acodec",
            "libmp3lame",
            "-b:a",
            "320k",
            audio_path_str,
        ])
        .output()
        .await?;
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg extract failed: {stderr}");
    }

    let audio_size = std::fs::metadata(&audio_path)?.len();
    log_trace(trace_id, "audio_extracted", &format!("size={audio_size}"));

    if audio_size <= MAX_AUDIO_BYTES {
        return Ok(std::fs::read(&audio_path)?);
    }

    // Direct single-pass target bitrate calculation to fit within MAX_AUDIO_BYTES.
    let duration_secs = crate::common::ffmpeg::probe_duration(&audio_path)
        .await
        .unwrap_or(0);
    let target_kbps = calculate_target_bitrate_kbps(duration_secs, MAX_AUDIO_BYTES);
    log_trace(
        trace_id,
        "compress_target",
        &format!("duration={duration_secs}s target_bitrate={target_kbps}k"),
    );

    let edit_text = tf(
        "separation.compressing_audio",
        &[("attempt", "1"), ("max", "1")],
    );
    let _ = api
        .edit_message_text(
            &EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(edit_text)
                .build(),
        )
        .await;

    let out_path = tmp_dir.join("compressed.mp3");
    let out_path_str = out_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("invalid UTF-8 path"))?;
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            audio_path_str,
            "-acodec",
            "libmp3lame",
            "-b:a",
            &format!("{target_kbps}k"),
            out_path_str,
        ])
        .output()
        .await?;
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg compress failed: {stderr}");
    }

    let size = std::fs::metadata(&out_path)?.len();
    log_trace(trace_id, "compressed", &format!("size={size}"));

    if size <= MAX_AUDIO_BYTES {
        return Ok(std::fs::read(&out_path)?);
    }

    anyhow::bail!("audio still too large after compression (size={size})");
}

/// Calculates the single-pass target MP3 bitrate in kbps needed to fit an audio file
/// within `max_bytes` based on its duration.
pub fn calculate_target_bitrate_kbps(duration_secs: u64, max_bytes: u64) -> u32 {
    let max_kbps = 320u32;
    let min_kbps = 64u32;
    if duration_secs > 0 {
        let cap_kbps = ((max_bytes as f64 * 8.0) / (duration_secs as f64 * 1000.0) * 0.95) as u32;
        cap_kbps.clamp(min_kbps, max_kbps)
    } else {
        192
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_target_bitrate_kbps() {
        const MAX_BYTES: u64 = 50 * 1024 * 1024;
        // Zero duration defaults to 192 kbps
        assert_eq!(calculate_target_bitrate_kbps(0, MAX_BYTES), 192);

        // Short duration clamped to max 320 kbps
        assert_eq!(calculate_target_bitrate_kbps(60, MAX_BYTES), 320);

        // 1 hour audio (~3600s) -> ~110 kbps
        let one_hour = calculate_target_bitrate_kbps(3600, MAX_BYTES);
        assert!((100..=120).contains(&one_hour));

        // Very long audio (10 hours) clamped to min 64 kbps
        assert_eq!(calculate_target_bitrate_kbps(36000, MAX_BYTES), 64);
    }
}
