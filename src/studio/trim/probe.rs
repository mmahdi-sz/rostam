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
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=duration,bit_rate:stream=codec_type,width,height,r_frame_rate,avg_frame_rate,codec_name",
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

    let filename = video_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "video.mp4".to_string());

    parse_video_metadata_json(&output.stdout, &filename)
}

pub fn parse_video_metadata_json(json_bytes: &[u8], filename: &str) -> anyhow::Result<VideoMetadata> {
    let json: serde_json::Value = serde_json::from_slice(json_bytes)?;
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

    let video_stream = streams
        .and_then(|arr| {
            arr.iter().find(|st| {
                let is_video = st
                    .get("codec_type")
                    .and_then(|t| t.as_str())
                    .map(|t| t == "video")
                    .unwrap_or(false);
                let has_dims = st.get("width").and_then(|w| w.as_u64()).unwrap_or(0) > 0
                    && st.get("height").and_then(|h| h.as_u64()).unwrap_or(0) > 0;
                is_video || has_dims
            })
        })
        .ok_or_else(|| anyhow::anyhow!("no video stream found in media file"))?;

    let width = video_stream
        .get("width")
        .and_then(|w| w.as_u64())
        .unwrap_or(0) as u32;

    let height = video_stream
        .get("height")
        .and_then(|h| h.as_u64())
        .unwrap_or(0) as u32;

    if width == 0 || height == 0 {
        anyhow::bail!("invalid video dimensions: {width}x{height}");
    }

    let codec = video_stream
        .get("codec_name")
        .and_then(|c| c.as_str())
        .unwrap_or("unknown")
        .to_string();

    let parse_fps = |rate_str: &str| -> u32 {
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
    };

    let fps = video_stream
        .get("r_frame_rate")
        .and_then(|r| r.as_str())
        .map(parse_fps)
        .filter(|&f| f > 0)
        .or_else(|| {
            video_stream
                .get("avg_frame_rate")
                .and_then(|r| r.as_str())
                .map(parse_fps)
        })
        .unwrap_or(0);

    Ok(VideoMetadata {
        filename: filename.to_string(),
        width,
        height,
        bitrate,
        fps,
        codec,
        duration_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_video_metadata_audio_first_stream() {
        // Reproducing the exact bug where audio stream #0 precedes video stream #1
        let json_data = br#"{
            "streams": [
                {
                    "index": 0,
                    "codec_name": "aac",
                    "codec_type": "audio",
                    "r_frame_rate": "0/0"
                },
                {
                    "index": 1,
                    "codec_name": "h264",
                    "codec_type": "video",
                    "width": 1280,
                    "height": 720,
                    "r_frame_rate": "60/1"
                }
            ],
            "format": {
                "duration": "25.770667",
                "bit_rate": "3387754"
            }
        }"#;

        let meta = parse_video_metadata_json(json_data, "file_111.mp4").expect("failed to parse");
        assert_eq!(meta.filename, "file_111.mp4");
        assert_eq!(meta.width, 1280);
        assert_eq!(meta.height, 720);
        assert_eq!(meta.fps, 60);
        assert_eq!(meta.codec, "h264");
        assert_eq!(meta.duration_secs, 26);
        assert_eq!(meta.bitrate, 3387754);
    }

    #[test]
    fn test_parse_video_metadata_no_video_stream() {
        let json_data = br#"{
            "streams": [
                {
                    "index": 0,
                    "codec_name": "aac",
                    "codec_type": "audio",
                    "r_frame_rate": "0/0"
                }
            ],
            "format": {
                "duration": "10.5",
                "bit_rate": "128000"
            }
        }"#;

        let res = parse_video_metadata_json(json_data, "audio.mp3");
        assert!(res.is_err(), "should fail when no video stream is present");
    }
}
