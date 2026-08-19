use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::EditMessageTextParams,
};

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

    // Iteratively compress: reduce bitrate by 10% each attempt until < 50MB.
    // Probe current bitrate then step down.
    let probe = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=bit_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            audio_path_str,
        ])
        .output()
        .await?;
    let initial_bitrate: u32 = String::from_utf8_lossy(&probe.stdout)
        .trim()
        .parse()
        .unwrap_or(320_000);

    let mut bitrate_bps = initial_bitrate;
    let mut attempt = 0u32;
    const MAX_ATTEMPTS: u32 = 20;

    loop {
        attempt += 1;
        bitrate_bps = (bitrate_bps as f64 * 0.9) as u32;
        let bitrate_kbps = (bitrate_bps / 1000).max(32);
        log_trace(
            trace_id,
            "compress_attempt",
            &format!("attempt={attempt} bitrate={bitrate_kbps}k"),
        );

        let edit_text = tf(
            "separation.compressing_audio",
            &[
                ("attempt", &attempt.to_string()),
                ("max", &MAX_ATTEMPTS.to_string()),
            ],
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

        let out_path = tmp_dir.join(format!("compressed_{attempt}.mp3"));
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
                &format!("{bitrate_kbps}k"),
                out_path_str,
            ])
            .output()
            .await?;
        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            anyhow::bail!("ffmpeg compress failed: {stderr}");
        }

        let size = std::fs::metadata(&out_path)?.len();
        log_trace(
            trace_id,
            "compressed",
            &format!("attempt={attempt} size={size}"),
        );

        if size <= MAX_AUDIO_BYTES {
            return Ok(std::fs::read(&out_path)?);
        }

        if attempt >= MAX_ATTEMPTS || bitrate_kbps <= 32 {
            anyhow::bail!("audio still too large after max compression attempts");
        }
    }
}
