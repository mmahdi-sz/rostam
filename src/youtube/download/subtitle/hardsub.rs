use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};

use frankenstein::client_reqwest::Bot;

use crate::youtube::download::cpu::{acquire_cpu, release_cpu};
use crate::youtube::download::status::edit_status;
use crate::youtube::format::format_duration;
use crate::youtube::trace::log_trace;

use super::files::subtitle_matches_selection;

pub fn parse_ffmpeg_time(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let sec_part = parts[2].split('.').next()?;
    let s: u64 = sec_part.parse().ok()?;
    Some(h * 3600 + m * 60 + s)
}

pub fn make_hardsub_bar(percent: u32) -> String {
    let filled = ((percent.min(100) as usize) * 10) / 100;
    let empty = 10 - filled;
    format!("{}{}", "●".repeat(filled), "○".repeat(empty))
}

pub async fn hardsub_subtitles(
    api: &Bot,
    chat_id: i64,
    msg_id: i32,
    dir: &std::path::Path,
    video_path: &str,
    target_langs: &[String],
    duration_secs: Option<u64>,
    trace_id: u64,
    user_id: i64,
) -> Result<String, String> {
    log_trace(
        trace_id,
        "hardsub_subtitles_started",
        &format!("video={video_path}"),
    );
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(video_path.to_string());
    };
    let mut srts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_srt = path.extension().and_then(|e| e.to_str()) == Some("srt");
        let matches_selection = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| subtitle_matches_selection(n, target_langs))
            .unwrap_or(false);
        if is_srt && matches_selection {
            srts.push(path);
        }
    }

    if srts.is_empty() {
        log_trace(trace_id, "hardsub_skipped", "no srt found");
        return Ok(video_path.to_string());
    }

    srts.sort_by(|a, b| {
        let a_name = a
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let b_name = b
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let a_is_fa = a_name.contains("translated_fa")
            || a_name.contains(".fa.")
            || a_name.contains("_fa.srt");
        let b_is_fa = b_name.contains("translated_fa")
            || b_name.contains(".fa.")
            || b_name.contains("_fa.srt");
        match (a_is_fa, b_is_fa) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a_name.cmp(&b_name),
        }
    });

    let primary_srt = &srts[0];
    let hardsub_srt = dir.join("hardsub_input.srt");
    if let Err(e) = tokio::fs::copy(primary_srt, &hardsub_srt).await {
        log_trace(trace_id, "hardsub_copy_srt_failed", &e.to_string());
        return Err(e.to_string());
    }

    if msg_id > 0 {
        edit_status(
            api,
            chat_id,
            msg_id,
            crate::i18n::t("youtube.download.hardsubbing"),
        )
        .await;
    }

    let cores = acquire_cpu(user_id, trace_id).await;
    let num_threads = if cores.is_empty() { 4 } else { cores.len() };

    let out_path = format!("{}_hardsub.mp4", video_path.trim_end_matches(".mp4"));
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.current_dir(dir)
       .arg("-y")
       .arg("-i").arg(video_path)
       .arg("-vf").arg("subtitles=hardsub_input.srt:force_style='FontSize=20,PrimaryColour=&H00FFFFFF,OutlineColour=&H00000000,BorderStyle=1,Outline=1'")
       .arg("-c:v").arg("libx264")
       .arg("-preset").arg("fast")
       .arg("-crf").arg("23")
       .arg("-threads").arg(num_threads.to_string())
       .arg("-c:a").arg("copy")
       .arg("-progress").arg("pipe:1")
       .arg("-nostats")
       .arg(&out_path)
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());

    log_trace(
        trace_id,
        "hardsub_exec_start",
        &format!("srt={} threads={num_threads}", primary_srt.display()),
    );

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            release_cpu(cores, trace_id).await;
            log_trace(trace_id, "hardsub_spawn_failed", &e.to_string());
            return Err(e.to_string());
        }
    };

    let Some(stdout) = child.stdout.take() else {
        release_cpu(cores, trace_id).await;
        return Err("piped stdout missing".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        release_cpu(cores, trace_id).await;
        return Err("piped stderr missing".to_string());
    };

    let mut reader = BufReader::new(stdout).lines();
    let stderr_task = tokio::spawn(async move {
        let mut err_reader = BufReader::new(stderr).lines();
        let mut stderr_tail = String::new();
        while let Ok(Some(line)) = err_reader.next_line().await {
            stderr_tail = line;
        }
        stderr_tail
    });

    let mut last_edit = std::time::Instant::now() - std::time::Duration::from_secs(2);
    let mut last_percent_int: i32 = -1;
    let total_secs = duration_secs.unwrap_or(0);

    while let Ok(Some(line)) = reader.next_line().await {
        if line.starts_with("out_time=") {
            let time_str = line.trim_start_matches("out_time=").trim();
            if let Some(current_secs) = parse_ffmpeg_time(time_str) {
                if total_secs > 0 {
                    let percent =
                        ((current_secs as f64 / total_secs as f64) * 100.0).clamp(0.0, 100.0);
                    let percent_int = percent as i32;
                    let now = std::time::Instant::now();
                    if percent_int != last_percent_int
                        && now.duration_since(last_edit) >= std::time::Duration::from_secs(1)
                    {
                        last_percent_int = percent_int;
                        last_edit = now;
                        let eta_secs = total_secs.saturating_sub(current_secs);
                        let bar = make_hardsub_bar(percent_int as u32);
                        let elapsed_str = format_duration(current_secs);
                        let total_str = format_duration(total_secs);
                        let eta_str = format_duration(eta_secs);
                        let text = crate::i18n::tf(
                            "youtube.download.hardsub_progress",
                            &[
                                ("bar", &bar),
                                ("percent", &format!("{percent_int}%")),
                                ("elapsed", &elapsed_str),
                                ("total", &total_str),
                                ("eta", &eta_str),
                            ],
                        );
                        if msg_id > 0 {
                            edit_status(api, chat_id, msg_id, text).await;
                        }
                    }
                }
            }
        }
    }

    let stderr_tail = stderr_task.await.unwrap_or_default();
    let status = child.wait().await;
    release_cpu(cores, trace_id).await;

    match status {
        Ok(s) if s.success() => {
            log_trace(trace_id, "hardsub_ok", &format!("out={out_path}"));
            let _ = tokio::fs::remove_file(video_path).await;
            let _ = tokio::fs::remove_file(&hardsub_srt).await;
            Ok(out_path)
        }
        Ok(s) => {
            log_trace(
                trace_id,
                "hardsub_failed",
                &format!("status={s} err={stderr_tail}"),
            );
            Err(if stderr_tail.is_empty() {
                format!("exit {s}")
            } else {
                stderr_tail
            })
        }
        Err(e) => {
            log_trace(trace_id, "hardsub_wait_failed", &e.to_string());
            Err(e.to_string())
        }
    }
}
