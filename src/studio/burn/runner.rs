//! FFmpeg command execution, codec-matched encoding, segment splitting, and monitoring.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use crate::i18n::{apply_premium_to_md, md_escape, tf};
use crate::studio::compress::format_eta_hms;

/// Minimum gap between burn ticker edits (Telegram rejects faster edit rates).
pub const TICKER_MIN_INTERVAL: Duration = Duration::from_secs(3);

/// Video encoder args matched to the source codec. Hardsub always re-encodes, and re-encoding an
/// AV1/HEVC source with libx264 inflates the file 2-3x at the same visual quality — a 900 MB AV1
/// input came back over the 2000 MB upload cap. CRF values are per-encoder scales, each roughly
/// equivalent to x264 CRF 22. Unknown/absent codec falls back to x264 (widest Telegram support).
/// ponytail: no 10-bit handling — pix_fmt is only forced on the x264 path, elsewhere ffmpeg keeps
/// the source format.
pub fn video_encoder_args(source_codec: &str) -> Vec<&'static str> {
    match source_codec.trim().to_ascii_lowercase().as_str() {
        "av1" => vec!["-c:v", "libsvtav1", "-preset", "9", "-crf", "32"],
        "hevc" | "h265" => vec!["-c:v", "libx265", "-preset", "medium", "-crf", "26"],
        "vp9" => vec![
            "-c:v",
            "libvpx-vp9",
            "-crf",
            "32",
            "-b:v",
            "0",
            "-row-mt",
            "1",
        ],
        _ => vec![
            "-c:v", "libx264", "-preset", "medium", "-crf", "22", "-pix_fmt", "yuv420p",
        ],
    }
}

/// How many pieces an oversized output must be cut into to fit under the upload cap. Always ≥2 —
/// this is only called once the output is known to be over the cap.
pub fn upload_part_count(output_bytes: u64, cap_bytes: u64) -> u64 {
    if cap_bytes == 0 {
        return 2;
    }
    output_bytes.div_ceil(cap_bytes).max(2)
}

/// Segment length that cuts `total_duration` into `parts` roughly equal pieces.
pub fn split_segment_secs(total_duration: u64, parts: u64) -> u64 {
    (total_duration / parts.max(1)).max(1)
}

/// Splits a finished video into `parts` roughly equal pieces by duration. Stream-copies, so there
/// is no second re-encode. Segment cuts land on keyframes, so part sizes are only approximate —
/// the caller must still check every part against the upload cap.
/// ponytail: remux only (I/O bound, no filters), so it stays off the CPU broker.
pub fn split_video_into_parts(
    ffmpeg_bin: &Path,
    input: &Path,
    work_dir: &Path,
    total_duration: u64,
    parts: u64,
) -> Result<Vec<PathBuf>, String> {
    let segment_secs = split_segment_secs(total_duration, parts);
    let pattern = work_dir.join("part_%02d.mp4");

    let out = std::process::Command::new(ffmpeg_bin)
        .args(["-y", "-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .args([
            "-c",
            "copy",
            "-map",
            "0",
            "-f",
            "segment",
            "-segment_time",
            &segment_secs.to_string(),
            "-reset_timestamps",
            "1",
        ])
        .arg(&pattern)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg segment spawn failed: {e}"))?;

    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr);
        let tail: String = tail
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!("ffmpeg segment failed: {tail}"));
    }

    let mut found: Vec<PathBuf> = std::fs::read_dir(work_dir)
        .map_err(|e| format!("read work dir failed: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("part_") && n.ends_with(".mp4"))
        })
        .collect();
    found.sort();

    if found.is_empty() {
        return Err("segment produced no parts".to_string());
    }
    Ok(found)
}

/// Runs ffmpeg to completion. The stdout `-progress` reader lives on its own thread so the
/// cancel check never waits on a line that a stalled ffmpeg will not print.
#[allow(clippy::too_many_arguments)]
pub fn run_ffmpeg_burn(
    ffmpeg_bin: &Path,
    input: &Path,
    filter_arg: &str,
    threads_arg: &str,
    source_codec: &str,
    output: &Path,
    log_path: &Path,
    total_duration: u64,
    job_start: Instant,
    cancel: &Arc<AtomicBool>,
    tick_tx: tokio::sync::watch::Sender<String>,
) -> Result<(), String> {
    let log_file =
        std::fs::File::create(log_path).map_err(|e| format!("ffmpeg log create failed: {e}"))?;

    let mut child = std::process::Command::new(ffmpeg_bin)
        .args(["-y", "-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-map", "0:a:0?", "-vf", filter_arg])
        .args(video_encoder_args(source_codec))
        .args([
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-movflags",
            "+faststart",
            "-threads",
            threads_arg,
            "-progress",
            "pipe:1",
        ])
        .arg(output)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(log_file))
        .spawn()
        .map_err(|e| format!("ffmpeg spawn failed: {e}"))?;

    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let Some(out) = stdout else { return };
        let mut current_us = 0u64;
        let mut speed_str = "1.0x".to_string();
        let mut last_pct = u64::MAX;
        let mut last_edit = Instant::now() - TICKER_MIN_INTERVAL;

        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(val) = line.strip_prefix("out_time_us=") {
                current_us = val.trim().parse::<u64>().unwrap_or(0);
            } else if let Some(val) = line.strip_prefix("speed=") {
                speed_str = val.trim().to_string();
            } else if line.starts_with("progress=") {
                let current_secs = current_us / 1_000_000;
                let pct = ((current_secs * 100) / total_duration).min(100);
                if pct == last_pct && last_edit.elapsed() < TICKER_MIN_INTERVAL {
                    continue;
                }
                last_pct = pct;
                last_edit = Instant::now();

                let speed_num = speed_str
                    .trim_end_matches('x')
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let eta_secs = if speed_num > 0.0 && total_duration > current_secs {
                    ((total_duration - current_secs) as f64 / speed_num) as u64
                } else {
                    0
                };

                let text = apply_premium_to_md(&tf(
                    "studio.burn.status_burning",
                    &[
                        (
                            "elapsed",
                            &md_escape(&format_eta_hms(job_start.elapsed().as_secs())),
                        ),
                        ("pct", &pct.to_string()),
                        ("speed", &md_escape(&speed_str)),
                        ("eta", &md_escape(&format_eta_hms(eta_secs))),
                    ],
                ));
                let _ = tick_tx.send(text);
            }
        }
    });

    let mut cancelled = false;
    let mut wait_err: Option<String> = None;
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            let _ = child.kill();
            break None;
        }
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => std::thread::sleep(Duration::from_millis(300)),
            Err(e) => {
                wait_err = Some(format!("try_wait failed: {e}"));
                let _ = child.kill();
                break None;
            }
        }
    };

    // Always reap, or a cancelled burn leaves a zombie behind.
    let _ = child.wait();
    let _ = reader.join();

    if cancelled {
        return Err("cancelled".to_string());
    }
    if let Some(e) = wait_err {
        return Err(e);
    }
    match status {
        Some(s) if s.success() => Ok(()),
        Some(s) => Err(format!("ffmpeg exited with {:?}", s.code())),
        None => Err("ffmpeg produced no exit status".to_string()),
    }
}

pub async fn extract_thumbnail(video: &Path, thumb: &Path) {
    let _ = tokio::process::Command::new(crate::config::ffmpeg_path())
        .args([
            "-y",
            "-hide_banner",
            "-nostdin",
            "-ss",
            "00:00:00.500",
            "-i",
        ])
        .arg(video)
        .args(["-vframes", "1", "-q:v", "3"])
        .arg(thumb)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

/// Last 400 chars of ffmpeg's stderr log — enough to name the real failure in one log line.
pub fn read_log_tail(log_path: &Path) -> String {
    let raw = std::fs::read_to_string(log_path).unwrap_or_default();
    let cleaned = raw.replace('\n', " ").trim().to_string();
    let chars: Vec<char> = cleaned.chars().collect();
    if chars.len() <= 400 {
        cleaned
    } else {
        chars[chars.len() - 400..].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_part_count_covers_oversized_output() {
        const CAP: u64 = crate::studio::burn::MAX_UPLOAD_BYTES;
        // 2402 MB output: halved, both pieces land under the cap.
        assert_eq!(upload_part_count(2402 * 1024 * 1024, CAP), 2);
        // Just over the cap still halves rather than failing.
        assert_eq!(upload_part_count(CAP + 1, CAP), 2);
        // Far over the cap needs more than two pieces, or a "half" would still be unsendable.
        assert_eq!(upload_part_count(5000 * 1024 * 1024, CAP), 3);
        assert_eq!(upload_part_count(20_000 * 1024 * 1024, CAP), 10);
        // Every piece must fit: bytes/parts is never above the cap.
        for mb in [2001u64, 2402, 3999, 4001, 9000, 40_000] {
            let bytes = mb * 1024 * 1024;
            let parts = upload_part_count(bytes, CAP);
            assert!(parts >= 2, "{mb} MB must be split");
            assert!(
                bytes.div_ceil(parts) <= CAP,
                "{mb} MB in {parts} parts still exceeds the cap"
            );
        }
        // Never divides by zero.
        assert_eq!(upload_part_count(1, 0), 2);
    }

    #[test]
    fn test_video_encoder_args_matches_source_codec() {
        // An AV1 source must not come back as x264 — that is what blew the 2000 MB upload cap.
        assert_eq!(video_encoder_args("av1")[1], "libsvtav1");
        assert_eq!(video_encoder_args("AV1")[1], "libsvtav1");
        assert_eq!(video_encoder_args("hevc")[1], "libx265");
        assert_eq!(video_encoder_args("h265")[1], "libx265");
        assert_eq!(video_encoder_args("vp9")[1], "libvpx-vp9");
        // Fallback keeps the widest-compatibility encoder.
        assert_eq!(video_encoder_args("h264")[1], "libx264");
        assert_eq!(video_encoder_args("unknown")[1], "libx264");
        assert_eq!(video_encoder_args("")[1], "libx264");
        // pix_fmt is only forced on the x264 path.
        assert!(video_encoder_args("h264").contains(&"yuv420p"));
        assert!(!video_encoder_args("av1").contains(&"yuv420p"));
    }
}
