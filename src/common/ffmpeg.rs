//! Robust, deadlock-safe FFmpeg and FFprobe wrappers.
//!
//! Subprocesses are executed either via `.output().await` (which fully drains
//! stdout/stderr) or with `Stdio::null()` to prevent 64KB pipe buffer deadlocks.

use std::path::Path;
use std::process::Stdio;
use anyhow::{bail, Result};

/// Extracted video and audio container metadata.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MediaMetadata {
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub bitrate: u64,
    pub fps: u32,
    pub codec: String,
    pub duration_secs: u64,
    pub duration_exact: f64,
}

/// Probes media file metadata using `ffprobe` in JSON output mode.
///
/// Uses `.output().await` to completely drain stdout/stderr and prevent pipe deadlocks.
pub async fn probe_metadata(path: &Path) -> Result<MediaMetadata> {
    let output = tokio::process::Command::new(crate::config::ffprobe_path())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,bit_rate:stream=width,height,r_frame_rate,codec_name",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to execute ffprobe: {e}"))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        bail!("ffprobe failed: {err_msg}");
    }

    parse_ffprobe_json(&output.stdout, path)
}

/// Parses the JSON stdout emitted by ffprobe.
pub fn parse_ffprobe_json(json_bytes: &[u8], path: &Path) -> Result<MediaMetadata> {
    let json: serde_json::Value = serde_json::from_slice(json_bytes)?;
    let format = json.get("format");
    let streams = json.get("streams").and_then(|s| s.as_array());

    let duration_exact = format
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let duration_secs = duration_exact.round() as u64;

    let bitrate = format
        .and_then(|f| f.get("bit_rate"))
        .and_then(|b| b.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let video_stream = streams.and_then(|arr| {
        arr.iter()
            .find(|st| st.get("width").is_some() || st.get("codec_name").is_some())
    });

    let width = video_stream
        .and_then(|s| s.get("width"))
        .and_then(|w| w.as_u64())
        .unwrap_or(0) as u32;

    let height = video_stream
        .and_then(|s| s.get("height"))
        .and_then(|h| h.as_u64())
        .unwrap_or(0) as u32;

    let codec = video_stream
        .and_then(|s| s.get("codec_name"))
        .and_then(|c| c.as_str())
        .unwrap_or("unknown")
        .to_string();

    let fps = video_stream
        .and_then(|s| s.get("r_frame_rate"))
        .and_then(|r| r.as_str())
        .map(|rate_str| {
            let parts: Vec<&str> = rate_str.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = parts[0].parse().unwrap_or(0.0);
                let den: f64 = parts[1].parse().unwrap_or(1.0);
                if den > 0.0 {
                    (num / den).round() as u32
                } else {
                    0
                }
            } else {
                rate_str.parse::<u32>().unwrap_or(0)
            }
        })
        .unwrap_or(0);

    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".to_string());

    Ok(MediaMetadata {
        filename,
        width,
        height,
        bitrate,
        fps,
        codec,
        duration_secs,
        duration_exact,
    })
}

/// Quickly extracts duration in seconds (rounded up) from audio/video files.
pub async fn probe_duration(path: &Path) -> Result<u64> {
    let meta = probe_metadata(path).await?;
    Ok(meta.duration_exact.ceil() as u64)
}

/// Converts any input audio file to standard mono/stereo PCM WAV (optimal for AI/ASR pipelines).
///
/// Standardizes sample rate (e.g. 16000 Hz for Vosk, 48000 Hz for DeepFilterNet).
/// Uses `Stdio::null()` for stderr to prevent 64KB pipe buffer deadlocks.
pub async fn convert_to_wav(
    input_path: &Path,
    output_wav_path: &Path,
    sample_rate_hz: u32,
    channels: u8,
) -> Result<()> {
    let status = tokio::process::Command::new(crate::config::ffmpeg_path())
        .arg("-y")
        .arg("-i")
        .arg(input_path)
        .args(["-ar", &sample_rate_hz.to_string()])
        .args(["-ac", &channels.to_string()])
        .args(["-c:a", "pcm_s16le"])
        .arg(output_wav_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn ffmpeg convert: {e}"))?;

    if !status.success() {
        bail!("ffmpeg convert exited with status {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ffprobe_json() {
        let json_data = br#"{
            "streams": [
                {
                    "codec_name": "h264",
                    "width": 1920,
                    "height": 1080,
                    "r_frame_rate": "30/1"
                }
            ],
            "format": {
                "duration": "123.456",
                "bit_rate": "2500000"
            }
        }"#;

        let path = Path::new("/tmp/test_video.mp4");
        let meta = parse_ffprobe_json(json_data, path).expect("parse json failed");

        assert_eq!(meta.filename, "test_video.mp4");
        assert_eq!(meta.width, 1920);
        assert_eq!(meta.height, 1080);
        assert_eq!(meta.bitrate, 2500000);
        assert_eq!(meta.fps, 30);
        assert_eq!(meta.codec, "h264");
        assert_eq!(meta.duration_secs, 123);
        assert!((meta.duration_exact - 123.456).abs() < 1e-6);
    }
}
