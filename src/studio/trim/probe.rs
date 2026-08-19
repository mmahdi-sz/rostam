use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VideoMetadata {
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub bitrate: u64,
    pub fps: u32,
    pub codec: String,
    pub duration_secs: u64,
}

pub fn format_bitrate(bps: u64) -> String {
    if bps == 0 {
        "N/A".to_string()
    } else {
        let kbps = bps / 1000;
        format!("{kbps} kbps")
    }
}

/// Runs `ffprobe` to extract video metadata.
pub async fn run_ffprobe(video_path: &Path) -> anyhow::Result<VideoMetadata> {
    let output = tokio::process::Command::new(crate::config::ffprobe_path())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,bit_rate:stream=width,height,r_frame_rate,codec_name",
            "-of",
            "json",
        ])
        .arg(video_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to execute ffprobe: {e}"))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe failed: {err_msg}");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let format = json.get("format");
    let streams = json.get("streams").and_then(|s| s.as_array());

    let duration_secs = format
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|d| d.round() as u64)
        .unwrap_or(0);

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

    let filename = video_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".to_string());

    Ok(VideoMetadata {
        filename,
        width,
        height,
        bitrate,
        fps,
        codec,
        duration_secs,
    })
}
