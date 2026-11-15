#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::probe::probe_audio_metadata;
use super::range::format_timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioContainer {
    Mp3,
    M4a,
    Flac,
    Wav,
    Opus,
}

impl AudioContainer {
    pub fn from_ext_or_codec(ext: &str, codec: &str) -> Self {
        let ext_lower = ext.to_lowercase();
        let codec_lower = codec.to_lowercase();
        match ext_lower.as_str() {
            "flac" => AudioContainer::Flac,
            "wav" => AudioContainer::Wav,
            "m4a" | "aac" => AudioContainer::M4a,
            "opus" | "ogg" => AudioContainer::Opus,
            "mp3" => AudioContainer::Mp3,
            _ => match codec_lower.as_str() {
                "flac" => AudioContainer::Flac,
                c if c.starts_with("pcm_") => AudioContainer::Wav,
                "aac" => AudioContainer::M4a,
                "opus" | "vorbis" => AudioContainer::Opus,
                _ => AudioContainer::Mp3,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrimResult {
    pub duration_secs: u64,
    pub re_encoded: bool,
}

impl std::ops::Deref for AudioTrimResult {
    type Target = u64;
    fn deref(&self) -> &Self::Target {
        &self.duration_secs
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrimResult {
    pub success: bool,
    pub re_encoded: bool,
    pub thumb_path: Option<std::path::PathBuf>,
}

/// Helper function to execute an ffmpeg command with cancellation polling and deadlock-free Stdio.
async fn run_ffmpeg_cancellable(
    args: &[&str],
    output_path: &Path,
    cancel_flag: Option<&Arc<AtomicBool>>,
) -> anyhow::Result<bool> {
    let mut cmd = tokio::process::Command::new(crate::config::ffmpeg_path());
    cmd.args(args);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn ffmpeg: {e}"))?;

    loop {
        if let Some(cf) = cancel_flag {
            if cf.load(Ordering::Relaxed) {
                let _ = child.kill().await;
                anyhow::bail!("trim cancelled by user");
            }
        }

        match tokio::time::timeout(Duration::from_millis(100), child.wait()).await {
            Ok(status_res) => {
                let status = status_res.map_err(|e| anyhow::anyhow!("ffmpeg wait error: {e}"))?;
                let success = status.success()
                    && output_path.exists()
                    && std::fs::metadata(output_path)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false);
                return Ok(success);
            }
            Err(_) => {
                // Timeout, continue loop to check cancel_flag
            }
        }
    }
}

/// Trims an audio file using start and end timestamps in seconds.
pub async fn trim_audio_clip(
    source_path: &Path,
    output_path: &Path,
    start_secs: u64,
    end_secs: u64,
    cancel_flag: Option<Arc<AtomicBool>>,
    threads: Option<&str>,
    trace_id: u64,
) -> anyhow::Result<AudioTrimResult> {
    let start_ts = format_timestamp(start_secs);
    let end_ts = format_timestamp(end_secs);
    trim_audio_clip_timestamps(
        source_path,
        output_path,
        &start_ts,
        &end_ts,
        cancel_flag,
        threads,
        trace_id,
    )
    .await
}

/// Trims an audio file using formatted timestamp strings (e.g. "00:01:15").
pub async fn trim_audio_clip_timestamps(
    source_path: &Path,
    output_path: &Path,
    start_ts: &str,
    end_ts: &str,
    cancel_flag: Option<Arc<AtomicBool>>,
    threads: Option<&str>,
    trace_id: u64,
) -> anyhow::Result<AudioTrimResult> {
    if let Some(ref cf) = cancel_flag {
        if cf.load(Ordering::Relaxed) {
            anyhow::bail!("audio trim cancelled before start");
        }
    }

    // Determine target container format
    let out_ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let src_ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let container = if !out_ext.is_empty() {
        AudioContainer::from_ext_or_codec(&out_ext, "")
    } else if !src_ext.is_empty() {
        AudioContainer::from_ext_or_codec(&src_ext, "")
    } else if let Ok(meta) = probe_audio_metadata(source_path).await {
        AudioContainer::from_ext_or_codec(&meta.ext, &meta.codec)
    } else {
        AudioContainer::Mp3
    };

    let src_str = source_path.to_str().unwrap_or_default();
    let out_str = output_path.to_str().unwrap_or_default();

    let mut re_encoded = false;

    match container {
        AudioContainer::Flac => {
            // FLAC requires lossless re-encode (-c:a flac) to update STREAMINFO total samples header.
            let mut args = vec![
                "-y", "-ss", start_ts, "-to", end_ts, "-i", src_str, "-map", "0", "-c:a", "flac",
                "-c:v", "copy",
            ];
            if let Some(t) = threads {
                args.push("-threads");
                args.push(t);
            }
            args.push(out_str);

            let ok = run_ffmpeg_cancellable(&args, output_path, cancel_flag.as_ref()).await?;
            if !ok {
                anyhow::bail!("flac re-encode trim failed for {out_str}");
            }
            re_encoded = true;
        }
        AudioContainer::Mp3 | AudioContainer::M4a | AudioContainer::Opus | AudioContainer::Wav => {
            // For lossy formats and WAV, attempt fast stream copy first.
            let mut copy_args = vec![
                "-y", "-ss", start_ts, "-to", end_ts, "-i", src_str, "-map", "0", "-c", "copy",
            ];
            if matches!(
                container,
                AudioContainer::Mp3 | AudioContainer::M4a | AudioContainer::Wav
            ) {
                copy_args.push("-avoid_negative_ts");
                copy_args.push("make_zero");
            }
            if matches!(container, AudioContainer::M4a) {
                copy_args.push("-movflags");
                copy_args.push("+faststart");
            }
            if let Some(t) = threads {
                copy_args.push("-threads");
                copy_args.push(t);
            }
            copy_args.push(out_str);

            let copy_ok =
                run_ffmpeg_cancellable(&copy_args, output_path, cancel_flag.as_ref()).await?;

            if copy_ok {
                log_ev!("studio_trim", trace_id, "audio_copy_success", "out" => out_str);
            } else {
                // Fallback to codec-matched re-encode
                log_ev!(
                    "studio_trim",
                    trace_id,
                    "audio_copy_failed_fallback_encode",
                    "container" => format!("{container:?}")
                );
                if output_path.exists() {
                    let _ = tokio::fs::remove_file(output_path).await;
                }

                let mut fb_args = vec![
                    "-y", "-ss", start_ts, "-to", end_ts, "-i", src_str, "-map", "0",
                ];
                let wav_codec_buf;
                match container {
                    AudioContainer::Mp3 => {
                        fb_args.extend_from_slice(&[
                            "-c:a",
                            "libmp3lame",
                            "-q:a",
                            "2",
                            "-c:v",
                            "copy",
                        ]);
                    }
                    AudioContainer::M4a => {
                        fb_args.extend_from_slice(&[
                            "-c:a",
                            "aac",
                            "-b:a",
                            "192k",
                            "-c:v",
                            "copy",
                            "-movflags",
                            "+faststart",
                        ]);
                    }
                    AudioContainer::Opus => {
                        fb_args.extend_from_slice(&[
                            "-c:a", "libopus", "-b:a", "128k", "-c:v", "copy",
                        ]);
                    }
                    AudioContainer::Wav => {
                        wav_codec_buf = if let Ok(meta) = probe_audio_metadata(source_path).await {
                            if meta.codec.starts_with("pcm_") {
                                meta.codec
                            } else {
                                "pcm_s16le".to_string()
                            }
                        } else {
                            "pcm_s16le".to_string()
                        };
                        fb_args.extend_from_slice(&["-c:a", &wav_codec_buf]);
                    }
                    AudioContainer::Flac => unreachable!(),
                }
                if let Some(t) = threads {
                    fb_args.push("-threads");
                    fb_args.push(t);
                }
                fb_args.push(out_str);

                let fb_ok =
                    run_ffmpeg_cancellable(&fb_args, output_path, cancel_flag.as_ref()).await?;
                if !fb_ok {
                    anyhow::bail!("audio fallback re-encode failed for {out_str}");
                }
                re_encoded = true;
            }
        }
    }

    // Measure exact output duration accurately
    let duration_secs = if let Ok(meta) = probe_audio_metadata(output_path).await {
        meta.duration_secs
    } else {
        crate::common::ffmpeg::probe_duration(output_path).await?
    };
    log_ev!(
        "studio_trim",
        trace_id,
        "audio_trim_complete",
        "duration" => duration_secs,
        "re_encoded" => re_encoded
    );

    Ok(AudioTrimResult {
        duration_secs,
        re_encoded,
    })
}

/// Trims a video file with stream copy -> libx264 re-encode fallback and optional thumbnail extraction.
pub async fn trim_video_clip(
    source_path: &Path,
    output_path: &Path,
    start_secs: u64,
    end_secs: u64,
    thumb_path: Option<&Path>,
    cancel_flag: Option<Arc<AtomicBool>>,
    threads: Option<&str>,
    trace_id: u64,
) -> anyhow::Result<VideoTrimResult> {
    let start_ts = format_timestamp(start_secs);
    let end_ts = format_timestamp(end_secs);
    trim_video_clip_timestamps(
        source_path,
        output_path,
        &start_ts,
        &end_ts,
        thumb_path,
        cancel_flag,
        threads,
        trace_id,
    )
    .await
}

/// Trims a video file using formatted timestamp strings.
pub async fn trim_video_clip_timestamps(
    source_path: &Path,
    output_path: &Path,
    start_ts: &str,
    end_ts: &str,
    thumb_path: Option<&Path>,
    cancel_flag: Option<Arc<AtomicBool>>,
    threads: Option<&str>,
    trace_id: u64,
) -> anyhow::Result<VideoTrimResult> {
    if let Some(ref cf) = cancel_flag {
        if cf.load(Ordering::Relaxed) {
            anyhow::bail!("video trim cancelled before start");
        }
    }

    let src_str = source_path.to_str().unwrap_or_default();
    let out_str = output_path.to_str().unwrap_or_default();

    // 1. Fast copy attempt
    let mut copy_args = vec![
        "-y",
        "-ss",
        start_ts,
        "-to",
        end_ts,
        "-i",
        src_str,
        "-c",
        "copy",
        "-avoid_negative_ts",
        "make_zero",
        "-movflags",
        "+faststart",
    ];
    if let Some(t) = threads {
        copy_args.push("-threads");
        copy_args.push(t);
    }
    copy_args.push(out_str);

    let copy_ok = run_ffmpeg_cancellable(&copy_args, output_path, cancel_flag.as_ref()).await?;

    let mut re_encoded = false;
    if copy_ok {
        log_ev!("studio_trim", trace_id, "video_copy_success", "out" => out_str);
    } else {
        log_ev!("studio_trim", trace_id, "video_copy_failed_fallback_encode", "out" => out_str);
        if output_path.exists() {
            let _ = tokio::fs::remove_file(output_path).await;
        }

        let source_codec = crate::common::ffmpeg::probe_metadata(source_path)
            .await
            .map(|m| m.codec)
            .unwrap_or_else(|_| "h264".to_string());
        let v_args = crate::common::ffmpeg::video_encoder_args(&source_codec);

        let mut enc_args = vec![
            "-y",
            "-ss",
            start_ts,
            "-to",
            end_ts,
            "-i",
            src_str,
        ];
        enc_args.extend_from_slice(&v_args);
        enc_args.extend_from_slice(&[
            "-c:a",
            "aac",
            "-avoid_negative_ts",
            "make_zero",
            "-movflags",
            "+faststart",
        ]);
        if let Some(t) = threads {
            enc_args.push("-threads");
            enc_args.push(t);
        }
        enc_args.push(out_str);

        let enc_ok = run_ffmpeg_cancellable(&enc_args, output_path, cancel_flag.as_ref()).await?;
        if !enc_ok {
            anyhow::bail!("video fallback encode failed for {out_str}");
        }
        re_encoded = true;
    }

    // 2. Generate video thumbnail if requested
    let mut generated_thumb = None;
    if let Some(thumb) = thumb_path {
        let thumb_str = thumb.to_str().unwrap_or_default();
        let status = tokio::process::Command::new(crate::config::ffmpeg_path())
            .args([
                "-y",
                "-ss",
                "00:00:00.500",
                "-i",
                out_str,
                "-vframes",
                "1",
                "-q:v",
                "3",
                thumb_str,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;

        if let Ok(s) = status {
            if s.success() && thumb.exists() {
                generated_thumb = Some(thumb.to_path_buf());
            }
        }
    }

    Ok(VideoTrimResult {
        success: true,
        re_encoded,
        thumb_path: generated_thumb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TestCleanupGuard(PathBuf);
    impl Drop for TestCleanupGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn create_test_dir(prefix: &str) -> (PathBuf, TestCleanupGuard) {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).expect("failed to create test dir");
        let guard = TestCleanupGuard(dir.clone());
        (dir, guard)
    }

    #[tokio::test]
    async fn test_trim_mp3_with_cover_and_tags() {
        let (dir, _guard) = create_test_dir("trim_mp3_test");
        let src_mp3 = dir.join("source.mp3");
        let cover_jpg = dir.join("cover.jpg");
        let out_mp3 = dir.join("out_trimmed.mp3");
        let extracted_thumb = dir.join("thumb.jpg");

        // 1. Create cover image
        let s1 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=400x400:d=1",
                "-vframes",
                "1",
                cover_jpg.to_str().unwrap(),
            ])
            .status()
            .expect("create cover image");
        assert!(s1.success());

        // 2. Create audio sine wav
        let raw_wav = dir.join("raw.wav");
        let s2 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=800:duration=6",
                raw_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create raw wav");
        assert!(s2.success());

        // 3. Assemble MP3 with cover and ID3 tags
        let s3 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                raw_wav.to_str().unwrap(),
                "-i",
                cover_jpg.to_str().unwrap(),
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:a",
                "libmp3lame",
                "-c:v",
                "copy",
                "-id3v2_version",
                "3",
                "-metadata",
                "title=Bohemian Rhapsody",
                "-metadata",
                "artist=Queen",
                "-metadata",
                "album=A Night at the Opera",
                src_mp3.to_str().unwrap(),
            ])
            .status()
            .expect("assemble source mp3");
        assert!(s3.success());

        // 4. Probe source metadata
        let src_meta = probe_audio_metadata(&src_mp3).await.expect("probe src mp3");
        assert_eq!(src_meta.duration_secs, 6);
        assert_eq!(src_meta.title.as_deref(), Some("Bohemian Rhapsody"));
        assert_eq!(src_meta.artist.as_deref(), Some("Queen"));
        assert_eq!(src_meta.album.as_deref(), Some("A Night at the Opera"));
        assert!(src_meta.has_cover_art);

        // 5. Trim audio clip (1s to 4s => ~3s nominal duration)
        let res = trim_audio_clip(&src_mp3, &out_mp3, 1, 4, None, None, 9001)
            .await
            .expect("trim mp3 clip");
        assert!(
            (res.duration_secs as i64 - 3).abs() <= 1,
            "probed duration should be ~3s (actual: {})",
            res.duration_secs
        );
        assert!(!res.re_encoded, "stream copy should succeed for valid MP3");

        // 6. Verify tags and cover art on output
        let out_meta = probe_audio_metadata(&out_mp3).await.expect("probe out mp3");
        assert_eq!(out_meta.duration_secs, 3);
        assert!(
            (out_meta.duration_exact - 3.0).abs() < 0.1,
            "exact duration should be within ~26ms MP3 frame boundary (actual: {})",
            out_meta.duration_exact
        );
        assert_eq!(out_meta.title.as_deref(), Some("Bohemian Rhapsody"));
        assert_eq!(out_meta.artist.as_deref(), Some("Queen"));
        assert_eq!(out_meta.album.as_deref(), Some("A Night at the Opera"));
        assert!(out_meta.has_cover_art);

        // 7. Verify cover art extraction
        let cover_ok = super::super::probe::extract_cover_art(&out_mp3, &extracted_thumb)
            .await
            .expect("extract cover art");
        assert!(cover_ok);
        assert!(extracted_thumb.exists());
        let thumb_size = std::fs::metadata(&extracted_thumb).unwrap().len();
        assert!(thumb_size > 0 && thumb_size <= 200 * 1024);
    }

    #[tokio::test]
    async fn test_trim_flac_lossless_header_fix() {
        let (dir, _guard) = create_test_dir("trim_flac_test");
        let src_flac = dir.join("source.flac");
        let out_flac = dir.join("out_trimmed.flac");

        // 1. Create source FLAC (7s duration) with tags
        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=7",
                "-c:a",
                "flac",
                "-metadata",
                "TITLE=Stairway to Heaven",
                "-metadata",
                "ARTIST=Led Zeppelin",
                src_flac.to_str().unwrap(),
            ])
            .status()
            .expect("create source flac");
        assert!(s.success());

        // 2. Trim FLAC (2s to 5s => 3s)
        let res = trim_audio_clip(&src_flac, &out_flac, 2, 5, None, None, 9002)
            .await
            .expect("trim flac");
        assert_eq!(res.duration_secs, 3);
        assert!(
            res.re_encoded,
            "FLAC must use lossless re-encode to update STREAMINFO"
        );

        // 3. Verify probe on output FLAC reports accurate duration and tags
        let out_meta = probe_audio_metadata(&out_flac)
            .await
            .expect("probe out flac");
        assert_eq!(out_meta.duration_secs, 3);
        assert_eq!(out_meta.title.as_deref(), Some("Stairway to Heaven"));
        assert_eq!(out_meta.artist.as_deref(), Some("Led Zeppelin"));
    }

    #[tokio::test]
    async fn test_trim_m4a_copy_and_tags() {
        let (dir, _guard) = create_test_dir("trim_m4a_test");
        let src_m4a = dir.join("source.m4a");
        let out_m4a = dir.join("out_trimmed.m4a");

        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:duration=6",
                "-c:a",
                "aac",
                "-metadata",
                "title=M4A Track",
                "-metadata",
                "artist=AAC Performer",
                src_m4a.to_str().unwrap(),
            ])
            .status()
            .expect("create source m4a");
        assert!(s.success());

        let res = trim_audio_clip(&src_m4a, &out_m4a, 1, 3, None, None, 9003)
            .await
            .expect("trim m4a");
        assert!(
            (res.duration_secs as i64 - 2).abs() <= 1,
            "probed duration should be ~2s (actual: {})",
            res.duration_secs
        );

        let out_meta = probe_audio_metadata(&out_m4a).await.expect("probe out m4a");
        assert_eq!(out_meta.duration_secs, 2);
        assert!(
            (out_meta.duration_exact - 2.0).abs() < 0.1,
            "exact duration should be within ~23ms AAC frame boundary (actual: {})",
            out_meta.duration_exact
        );
        assert_eq!(out_meta.title.as_deref(), Some("M4A Track"));
        assert_eq!(out_meta.artist.as_deref(), Some("AAC Performer"));
    }

    #[derive(serde::Deserialize)]
    struct ProbeStreamInfo {
        codec_name: Option<String>,
        sample_fmt: Option<String>,
        sample_rate: Option<String>,
        channels: Option<u32>,
        bits_per_raw_sample: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct ProbeFormatInfo {
        tags: Option<std::collections::HashMap<String, String>>,
    }

    #[derive(serde::Deserialize)]
    struct ProbeOutput {
        streams: Option<Vec<ProbeStreamInfo>>,
        format: Option<ProbeFormatInfo>,
    }

    async fn probe_audio_stream_details(
        path: &Path,
    ) -> (ProbeStreamInfo, std::collections::HashMap<String, String>) {
        let output = tokio::process::Command::new(crate::config::ffprobe_path())
            .args([
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=codec_name,sample_fmt,sample_rate,channels,bits_per_raw_sample",
                "-show_entries",
                "format_tags",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .await
            .expect("run ffprobe");
        assert!(output.status.success());
        let parsed: ProbeOutput = serde_json::from_slice(&output.stdout).expect("parse probe json");
        let stream = parsed
            .streams
            .and_then(|mut s| {
                if s.is_empty() {
                    None
                } else {
                    Some(s.remove(0))
                }
            })
            .expect("stream 0");
        let tags = parsed.format.and_then(|f| f.tags).unwrap_or_default();
        (stream, tags)
    }

    #[tokio::test]
    async fn test_trim_wav_pcm() {
        let (dir, _guard) = create_test_dir("trim_wav_test");
        let src_wav = dir.join("source.wav");
        let out_wav = dir.join("out_trimmed.wav");

        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=600:duration=5",
                "-c:a",
                "pcm_s16le",
                src_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create source wav");
        assert!(s.success());

        let res = trim_audio_clip(&src_wav, &out_wav, 1, 4, None, None, 9004)
            .await
            .expect("trim wav");
        assert_eq!(res.duration_secs, 3);

        let out_meta = probe_audio_metadata(&out_wav).await.expect("probe out wav");
        assert_eq!(out_meta.duration_secs, 3);
    }

    #[tokio::test]
    async fn test_trim_wav_24bit_preservation() {
        let (dir, _guard) = create_test_dir("trim_wav24_test");
        let src_wav = dir.join("source_24bit.wav");
        let out_wav = dir.join("out_trimmed_24bit.wav");

        // 1. Create 5-second 24-bit stereo WAV at 48kHz with metadata tags
        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=48000:duration=5",
                "-ac",
                "2",
                "-c:a",
                "pcm_s24le",
                "-metadata",
                "title=Master 24",
                "-metadata",
                "artist=Sound Lab",
                src_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create 24-bit wav");
        assert!(s.success());

        // 2. Trim audio clip (1s to 4s = 3s duration)
        let res = trim_audio_clip(&src_wav, &out_wav, 1, 4, None, None, 9010)
            .await
            .expect("trim 24-bit wav");
        assert_eq!(res.duration_secs, 3);
        assert!(
            !res.re_encoded,
            "WAV stream copy must succeed without re-encoding"
        );

        // 3. Assert stream precision and tags via ffprobe
        let (stream, _) = probe_audio_stream_details(&out_wav).await;
        assert_eq!(stream.codec_name.as_deref(), Some("pcm_s24le"));
        assert_eq!(stream.sample_fmt.as_deref(), Some("s32"));
        assert_eq!(stream.bits_per_raw_sample.as_deref(), Some("24"));
        assert_eq!(stream.sample_rate.as_deref(), Some("48000"));
        assert_eq!(stream.channels, Some(2));

        let meta = probe_audio_metadata(&out_wav)
            .await
            .expect("probe metadata");
        assert_eq!(meta.title.as_deref(), Some("Master 24"));
        assert_eq!(meta.artist.as_deref(), Some("Sound Lab"));
        assert_eq!(meta.duration_secs, 3);
    }

    #[tokio::test]
    async fn test_trim_wav_32bit_float_preservation() {
        let (dir, _guard) = create_test_dir("trim_wav_f32_test");
        let src_wav = dir.join("source_f32.wav");
        let out_wav = dir.join("out_trimmed_f32.wav");

        // 1. Create 5-second 32-bit float stereo WAV at 48kHz with metadata tags
        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=48000:duration=5",
                "-ac",
                "2",
                "-c:a",
                "pcm_f32le",
                "-metadata",
                "title=Float 32",
                "-metadata",
                "artist=Sound Lab",
                src_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create 32-bit float wav");
        assert!(s.success());

        // 2. Trim audio clip (1s to 4s = 3s duration)
        let res = trim_audio_clip(&src_wav, &out_wav, 1, 4, None, None, 9011)
            .await
            .expect("trim 32-bit float wav");
        assert_eq!(res.duration_secs, 3);
        assert!(
            !res.re_encoded,
            "WAV stream copy must succeed without re-encoding"
        );

        // 3. Assert stream precision and tags via ffprobe
        let (stream, _) = probe_audio_stream_details(&out_wav).await;
        assert_eq!(stream.codec_name.as_deref(), Some("pcm_f32le"));
        assert_eq!(stream.sample_fmt.as_deref(), Some("flt"));
        assert_eq!(stream.sample_rate.as_deref(), Some("48000"));
        assert_eq!(stream.channels, Some(2));

        let meta = probe_audio_metadata(&out_wav)
            .await
            .expect("probe metadata");
        assert_eq!(meta.title.as_deref(), Some("Float 32"));
        assert_eq!(meta.artist.as_deref(), Some("Sound Lab"));
        assert_eq!(meta.duration_secs, 3);
    }

    #[tokio::test]
    async fn test_trim_flac_24bit_96khz_preservation() {
        let (dir, _guard) = create_test_dir("trim_flac24_test");
        let raw_wav = dir.join("raw_24bit_96k.wav");
        let src_flac = dir.join("source_24bit_96k.flac");
        let out_flac = dir.join("out_trimmed_24bit_96k.flac");

        // 1. Create 24-bit 96kHz PCM WAV intermediate
        let s1 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=96000:duration=5",
                "-ac",
                "2",
                "-c:a",
                "pcm_s24le",
                raw_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create intermediate 24-bit 96k wav");
        assert!(s1.success());

        // 2. Encode to 24-bit FLAC with tags
        let s2 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                raw_wav.to_str().unwrap(),
                "-c:a",
                "flac",
                "-metadata",
                "title=FLAC 24",
                "-metadata",
                "artist=HiRes Lab",
                src_flac.to_str().unwrap(),
            ])
            .status()
            .expect("create 24-bit flac");
        assert!(s2.success());

        // 3. Trim FLAC (1s to 4s = 3s duration)
        let res = trim_audio_clip(&src_flac, &out_flac, 1, 4, None, None, 9012)
            .await
            .expect("trim 24-bit flac");
        assert_eq!(res.duration_secs, 3);
        assert!(
            res.re_encoded,
            "FLAC uses lossless re-encode to maintain STREAMINFO headers"
        );

        // 4. Assert stream precision and tags via ffprobe
        let (stream, _) = probe_audio_stream_details(&out_flac).await;
        assert_eq!(stream.codec_name.as_deref(), Some("flac"));
        assert_eq!(stream.sample_fmt.as_deref(), Some("s32"));
        assert_eq!(stream.bits_per_raw_sample.as_deref(), Some("24"));
        assert_eq!(stream.sample_rate.as_deref(), Some("96000"));
        assert_eq!(stream.channels, Some(2));

        let meta = probe_audio_metadata(&out_flac)
            .await
            .expect("probe flac metadata");
        assert_eq!(meta.title.as_deref(), Some("FLAC 24"));
        assert_eq!(meta.artist.as_deref(), Some("HiRes Lab"));
        assert_eq!(meta.duration_secs, 3);
    }

    #[tokio::test]
    async fn test_trim_wav_fallback_reencode() {
        let (dir, _guard) = create_test_dir("trim_wav_fb_test");
        let src_opus = dir.join("source.opus");
        let out_wav = dir.join("fallback.wav");

        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=800:duration=5",
                "-c:a",
                "libopus",
                src_opus.to_str().unwrap(),
            ])
            .status()
            .expect("create opus");
        assert!(s.success());

        // Trimming opus with .wav target forces copy to fail (opus unsupported in wav) and fallback to re-encode
        let res = trim_audio_clip(&src_opus, &out_wav, 1, 4, None, None, 9013)
            .await
            .expect("trim with fallback to wav");
        assert_eq!(res.duration_secs, 3);
        assert!(
            res.re_encoded,
            "Must re-encode when stream copy is impossible"
        );

        let (stream, _) = probe_audio_stream_details(&out_wav).await;
        assert_eq!(stream.codec_name.as_deref(), Some("pcm_s16le"));
        assert_eq!(stream.channels, Some(1));
    }

    #[tokio::test]
    async fn test_trim_audio_cancelled() {
        let (dir, _guard) = create_test_dir("trim_cancel_test");
        let src_wav = dir.join("source.wav");
        let out_wav = dir.join("out_trimmed.wav");

        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=400:duration=2",
                src_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create wav");
        assert!(s.success());

        let cancel_flag = Arc::new(AtomicBool::new(true)); // Pre-cancelled
        let res = trim_audio_clip(&src_wav, &out_wav, 0, 1, Some(cancel_flag), None, 9005).await;
        assert!(res.is_err(), "pre-cancelled job must return error");
    }

    #[tokio::test]
    async fn test_extract_cover_art_no_cover() {
        let (dir, _guard) = create_test_dir("no_cover_test");
        let src_wav = dir.join("source.wav");
        let no_thumb = dir.join("thumb.jpg");

        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=400:duration=2",
                "-c:a",
                "pcm_s16le",
                src_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create wav");
        assert!(s.success());

        let ok = super::super::probe::extract_cover_art(&src_wav, &no_thumb)
            .await
            .expect("extract cover art should return Ok(false) on audio with no cover");
        assert!(!ok, "should return false when no cover exists");
        assert!(!no_thumb.exists(), "no thumbnail file should remain");
    }

    #[tokio::test]
    async fn test_trim_video_clip_basic() {
        let (dir, _guard) = create_test_dir("trim_vid_test");
        let src_mp4 = dir.join("source.mp4");
        let out_mp4 = dir.join("out_trimmed.mp4");
        let thumb_jpg = dir.join("thumb.jpg");

        // Create 3-second test video
        let s = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=3:size=320x240:rate=25",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:duration=3",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
                "-pix_fmt",
                "yuv420p",
                src_mp4.to_str().unwrap(),
            ])
            .status()
            .expect("create test video");
        assert!(s.success());

        let res = trim_video_clip(&src_mp4, &out_mp4, 1, 2, Some(&thumb_jpg), None, None, 9006)
            .await
            .expect("trim video clip");

        assert!(res.success);
        assert!(out_mp4.exists());
        assert!(thumb_jpg.exists());
    }
}
