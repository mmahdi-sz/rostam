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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioMetadata {
    pub filename: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub codec: String,
    pub duration_secs: u64,
    pub duration_exact: f64,
    pub bitrate: u64,
    pub has_cover_art: bool,
    pub ext: String,
}

impl AudioMetadata {
    #[allow(dead_code)]
    pub fn duration(&self) -> f64 {
        self.duration_exact
    }
}

pub fn format_bitrate(bps: u64) -> String {
    if bps == 0 {
        "N/A".to_string()
    } else {
        let kbps = bps / 1000;
        format!("{kbps} kbps")
    }
}

/// Convenience alias for `run_ffprobe` to probe video metadata.
#[allow(dead_code)]
pub async fn probe_video_metadata(video_path: &Path) -> anyhow::Result<VideoMetadata> {
    run_ffprobe(video_path).await
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

pub fn parse_video_metadata_json(
    json_bytes: &[u8],
    filename: &str,
) -> anyhow::Result<VideoMetadata> {
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

/// Runs `ffprobe` to extract audio metadata, format tags, stream properties, and cover art disposition.
#[allow(dead_code)]
pub async fn probe_audio_metadata(path: &Path) -> anyhow::Result<AudioMetadata> {
    let output = tokio::process::Command::new(crate::config::ffprobe_path())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,bit_rate",
            "-show_entries",
            "format_tags",
            "-show_entries",
            "stream=index,codec_name,codec_type,sample_rate,channels,bit_rate",
            "-show_entries",
            "stream_tags",
            "-show_entries",
            "stream_disposition=attached_pic",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to execute ffprobe: {e}"))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe audio failed: {err_msg}");
    }

    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio".to_string());

    parse_audio_metadata_json(&output.stdout, &filename, path)
}

/// Parses the JSON stdout emitted by ffprobe for audio files.
pub fn parse_audio_metadata_json(
    json_bytes: &[u8],
    filename: &str,
    path: &Path,
) -> anyhow::Result<AudioMetadata> {
    let json: serde_json::Value = serde_json::from_slice(json_bytes)?;
    let format = json.get("format");
    let streams = json.get("streams").and_then(|s| s.as_array());

    let audio_stream = streams
        .and_then(|arr| {
            arr.iter().find(|st| {
                st.get("codec_type")
                    .and_then(|t| t.as_str())
                    .map(|t| t == "audio")
                    .unwrap_or(false)
            })
        })
        .ok_or_else(|| anyhow::anyhow!("no audio stream found in media file"))?;

    let codec = audio_stream
        .get("codec_name")
        .and_then(|c| c.as_str())
        .unwrap_or("unknown")
        .to_string();

    let duration_exact = format
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| {
            audio_stream
                .get("duration")
                .and_then(|d| d.as_str())
                .and_then(|s| s.parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    let duration_secs = duration_exact.round() as u64;

    let bitrate = audio_stream
        .get("bit_rate")
        .and_then(|b| b.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| {
            format
                .and_then(|f| f.get("bit_rate"))
                .and_then(|b| b.as_str())
                .and_then(|s| s.parse::<u64>().ok())
        })
        .unwrap_or(0);

    let has_cover_art = streams
        .map(|arr| {
            arr.iter().any(|st| {
                let is_video = st
                    .get("codec_type")
                    .and_then(|t| t.as_str())
                    .map(|t| t == "video")
                    .unwrap_or(false);
                let is_attached_pic = st
                    .get("disposition")
                    .and_then(|d| d.get("attached_pic"))
                    .and_then(|v| v.as_i64())
                    == Some(1);
                is_attached_pic || is_video
            })
        })
        .unwrap_or(false);

    let format_tags = format.and_then(|f| f.get("tags"));
    let stream_tags = audio_stream.get("tags");

    let title = get_tag_ci(format_tags, &["title"]).or_else(|| get_tag_ci(stream_tags, &["title"]));

    let artist = get_tag_ci(
        format_tags,
        &["artist", "performer", "album_artist", "composer"],
    )
    .or_else(|| {
        get_tag_ci(
            stream_tags,
            &["artist", "performer", "album_artist", "composer"],
        )
    });

    let album = get_tag_ci(format_tags, &["album"]).or_else(|| get_tag_ci(stream_tags, &["album"]));

    let path_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let ext = match path_ext.as_str() {
        "mp3" | "m4a" | "flac" | "wav" | "ogg" | "opus" | "aac" => path_ext,
        _ => match codec.as_str() {
            "mp3" => "mp3".to_string(),
            "aac" => "m4a".to_string(),
            "flac" => "flac".to_string(),
            "opus" => {
                if path_ext == "opus" {
                    "opus".to_string()
                } else {
                    "ogg".to_string()
                }
            }
            "vorbis" => "ogg".to_string(),
            c if c.starts_with("pcm_") => "wav".to_string(),
            _ => {
                if !path_ext.is_empty() {
                    path_ext
                } else {
                    "mp3".to_string()
                }
            }
        },
    };

    Ok(AudioMetadata {
        filename: filename.to_string(),
        title,
        artist,
        album,
        codec,
        duration_secs,
        duration_exact,
        bitrate,
        has_cover_art,
        ext,
    })
}

fn get_tag_ci(tags_val: Option<&serde_json::Value>, keys: &[&str]) -> Option<String> {
    let obj = tags_val?.as_object()?;
    for (k, v) in obj {
        for key in keys {
            if k.eq_ignore_ascii_case(key) {
                if let Some(s) = v.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Extracts embedded cover art from audio into a Telegram-compliant JPEG (<200KB, max 320x320).
/// Returns true if a valid cover art file was extracted, or false otherwise.
#[allow(dead_code)]
pub async fn extract_cover_art(audio_path: &Path, out_jpg: &Path) -> anyhow::Result<bool> {
    let output = tokio::process::Command::new(crate::config::ffmpeg_path())
        .args([
            "-y",
            "-i",
            audio_path.to_str().unwrap_or_default(),
            "-map",
            "0:v:0",
            "-vf",
            "scale='min(320,iw)':-1",
            "-vframes",
            "1",
            "-q:v",
            "2",
            "-update",
            "1",
            out_jpg.to_str().unwrap_or_default(),
        ])
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to execute ffmpeg for cover extraction: {e}"))?;

    if !output.status.success() {
        if out_jpg.exists() {
            let _ = tokio::fs::remove_file(out_jpg).await;
        }
        return Ok(false);
    }

    if out_jpg.exists() {
        let meta = tokio::fs::metadata(out_jpg).await?;
        let size = meta.len();
        if size > 0 && size <= 200 * 1024 {
            Ok(true)
        } else if size > 200 * 1024 {
            // Second pass with higher compression if unexpectedly oversized
            let pass2 = tokio::process::Command::new(crate::config::ffmpeg_path())
                .args([
                    "-y",
                    "-i",
                    audio_path.to_str().unwrap_or_default(),
                    "-map",
                    "0:v:0",
                    "-vf",
                    "scale='min(320,iw)':-1",
                    "-vframes",
                    "1",
                    "-q:v",
                    "6",
                    "-update",
                    "1",
                    out_jpg.to_str().unwrap_or_default(),
                ])
                .output()
                .await;
            if let Ok(p2) = pass2 {
                if p2.status.success() && out_jpg.exists() {
                    let s2 = tokio::fs::metadata(out_jpg).await?.len();
                    if s2 > 0 && s2 <= 200 * 1024 {
                        return Ok(true);
                    }
                }
            }
            let _ = tokio::fs::remove_file(out_jpg).await;
            Ok(false)
        } else {
            let _ = tokio::fs::remove_file(out_jpg).await;
            Ok(false)
        }
    } else {
        Ok(false)
    }
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

    #[test]
    fn test_parse_audio_metadata_with_cover_and_tags() {
        let json_data = br#"{
            "streams": [
                {
                    "index": 0,
                    "codec_name": "mp3",
                    "codec_type": "audio",
                    "sample_rate": "44100",
                    "channels": 2,
                    "bit_rate": "320000",
                    "disposition": { "attached_pic": 0 }
                },
                {
                    "index": 1,
                    "codec_name": "mjpeg",
                    "codec_type": "video",
                    "disposition": { "attached_pic": 1 }
                }
            ],
            "format": {
                "duration": "184.502000",
                "bit_rate": "324500",
                "tags": {
                    "title": "Bohemian Rhapsody",
                    "artist": "Queen",
                    "album": "A Night at the Opera"
                }
            }
        }"#;

        let meta = parse_audio_metadata_json(json_data, "song.mp3", Path::new("song.mp3"))
            .expect("should parse audio metadata");

        assert_eq!(meta.filename, "song.mp3");
        assert_eq!(meta.codec, "mp3");
        assert_eq!(meta.ext, "mp3");
        assert_eq!(meta.duration_secs, 185);
        assert!((meta.duration_exact - 184.502).abs() < 1e-3);
        assert_eq!(meta.bitrate, 320000);
        assert!(meta.has_cover_art);
        assert_eq!(meta.title.as_deref(), Some("Bohemian Rhapsody"));
        assert_eq!(meta.artist.as_deref(), Some("Queen"));
        assert_eq!(meta.album.as_deref(), Some("A Night at the Opera"));
    }

    #[test]
    fn test_parse_audio_metadata_case_insensitive_tags() {
        let json_data = br#"{
            "streams": [
                {
                    "index": 0,
                    "codec_name": "flac",
                    "codec_type": "audio",
                    "bit_rate": "900000",
                    "tags": {
                        "TITLE": "Stairway to Heaven",
                        "PERFORMER": "Led Zeppelin",
                        "ALBUM": "Led Zeppelin IV"
                    }
                }
            ],
            "format": {
                "duration": "482.100000",
                "bit_rate": "910000"
            }
        }"#;

        let meta = parse_audio_metadata_json(json_data, "track.flac", Path::new("track.flac"))
            .expect("should parse flac metadata");

        assert_eq!(meta.codec, "flac");
        assert_eq!(meta.ext, "flac");
        assert_eq!(meta.duration_secs, 482);
        assert!(!meta.has_cover_art);
        assert_eq!(meta.title.as_deref(), Some("Stairway to Heaven"));
        assert_eq!(meta.artist.as_deref(), Some("Led Zeppelin"));
        assert_eq!(meta.album.as_deref(), Some("Led Zeppelin IV"));
    }

    #[test]
    fn test_parse_audio_metadata_no_audio_stream() {
        let json_data = br#"{
            "streams": [
                {
                    "index": 0,
                    "codec_name": "h264",
                    "codec_type": "video"
                }
            ],
            "format": {
                "duration": "12.0"
            }
        }"#;

        let res = parse_audio_metadata_json(json_data, "video.mp4", Path::new("video.mp4"));
        assert!(res.is_err(), "should fail when no audio stream is present");
    }
}
