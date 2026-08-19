use std::process::Stdio;

use super::super::trace::log_trace;

pub async fn split_video(
    input: &str,
    dir: &std::path::Path,
    num_parts: usize,
    duration_secs: Option<u64>,
    trace_id: u64,
) -> Result<Vec<String>, String> {
    let total_secs = match duration_secs.filter(|&d| d > 0) {
        Some(d) => d,
        None => crate::common::ffmpeg::probe_duration(std::path::Path::new(input))
            .await
            .map_err(|e| format!("ffprobe duration: {e}"))?,
    };

    if total_secs == 0 {
        return Err("video duration is zero".to_string());
    }

    let part_secs = (total_secs + num_parts as u64 - 1) / num_parts as u64;
    log_trace(
        trace_id,
        "split_plan",
        &format!("total_secs={total_secs} parts={num_parts} part_secs={part_secs}"),
    );

    let mut parts = Vec::new();
    for i in 0..num_parts {
        let start = i as u64 * part_secs;
        if start >= total_secs {
            break;
        }

        let out_path = dir.join(format!("part{:02}.mp4", i + 1));
        let out_str = out_path.to_string_lossy().into_owned();

        log_trace(
            trace_id,
            "split_part_start",
            &format!(
                "part={} start={start}s duration={part_secs}s out={out_str}",
                i + 1
            ),
        );

        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-ss",
                &start.to_string(),
                "-i",
                input,
                "-t",
                &part_secs.to_string(),
                "-c",
                "copy",
                "-avoid_negative_ts",
                "make_zero",
                &out_str,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| format!("ffmpeg spawn part {}: {e}", i + 1))?;

        if !status.success() {
            return Err(format!("ffmpeg exit {} on part {}", status, i + 1));
        }

        log_trace(
            trace_id,
            "split_part_done",
            &format!("part={} path={out_str}", i + 1),
        );
        parts.push(out_str);
    }

    Ok(parts)
}
