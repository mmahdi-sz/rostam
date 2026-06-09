use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::DeleteMessageParams,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;

use crate::i18n::{entities_for_text, t, tf};
use crate::stats;

use super::super::trace::log_trace;
use super::cancel::{register_cancel, unregister_cancel, UnregisterGuard};
use super::helpers::{cleanup_dir, fetch_thumbnail, pick_largest_file, quality_label_for, send_subtitle_files};
use super::progress::{format_progress_body, parse_progress_line, ProgressSnapshot};
use super::selection_helpers::find_format;
use super::split::split_video;
use super::status::{edit_progress_status, edit_status};
use super::store::take_request;
use super::types::{Selection, SubtitleMode};
use super::upload::{build_part_params, build_single_params, send_video_with_progress};

pub const EDIT_THROTTLE: Duration = Duration::from_secs(1);
const DOWNLOAD_ROOT: &str = "/mnt/data/mahdidev/ros/dev/downloads/yt";
const MAX_SIZE_MB: u64 = 2000;
const TARGET_PART_MB: u64 = 1700;

pub fn spawn_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
) {
    let cancel = register_cancel(request_id);
    tokio::spawn(async move {
        run_download(api, request_id, selection, status_chat_id, status_message_id, cancel).await
    });
}

async fn run_download(
    api: Bot,
    request_id: u64,
    selection: Selection,
    status_chat_id: i64,
    status_message_id: i32,
    cancel: Arc<Notify>,
) {
    let height = selection.height;
    let codec = selection.codec;
    let Some(req) = take_request(request_id) else {
        edit_status(&api, status_chat_id, status_message_id, t("youtube.download.request_expired")).await;
        unregister_cancel(request_id);
        return;
    };
    let _cancel_guard = UnregisterGuard(request_id);
    let mut cancel_fut = std::pin::pin!(cancel.notified());
    let trace_id = req.trace_id;
    let user_id = req.user_id.unwrap_or(0);

    // ثبت شروع دانلود
    let stats_job_id = stats::record_download_start(user_id).await;

    let quality_label = quality_label_for(height);
    log_trace(trace_id, "download_begin", &format!(
        "request_id={request_id} height={height} codec={} url={}", codec.key(), req.webpage_url
    ));

    let Some(fmt) = find_format(&req, height, codec) else {
        log_trace(trace_id, "download_format_missing", &format!("height={height} codec={}", codec.key()));
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", "format not found")])).await;
        return;
    };
    let format_id = fmt.format_id.clone();

    let dir = PathBuf::from(format!("{DOWNLOAD_ROOT}/{trace_id}"));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_trace(trace_id, "download_mkdir_failed", &e.to_string());
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", &e.to_string())])).await;
        return;
    }

    let output_template = format!("{}/%(id)s.%(ext)s", dir.display());
    let format_spec = format!("{format_id}+bestaudio/best");
    let progress_template = format!(
        "YT_PROGRESS|%(progress._percent_str)s|%(progress._downloaded_bytes_str)s|%(progress._total_bytes_str)s|%(progress._total_bytes_estimate_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(progress._elapsed_str)s"
    );

    let initial = ProgressSnapshot {
        percent: "0.0%".into(), downloaded: "0B".into(), total: "?".into(),
        speed: "?".into(), eta: "?".into(), elapsed: "00:00".into(), percent_int: 0,
    };
    edit_progress_status(&api, status_chat_id, status_message_id,
        format_progress_body(&initial, &quality_label), request_id).await;

    let postprocess_template = progress_template.clone();
    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--js-runtimes").arg("deno:/root/.deno/bin/deno")
        .arg("--cookies-from-browser").arg(&req.cookie_spec)
        .arg("--no-warnings").arg("--no-playlist").arg("--progress")
        .arg("--no-color").arg("-f").arg(&format_spec)
        .arg("--merge-output-format").arg("mp4")
        .arg("--newline")
        .arg("--progress-template").arg(format!("download:{progress_template}"))
        .arg("--progress-template").arg(format!("postprocess:{postprocess_template}"))
        .arg("--print").arg("after_move:filepath")
        .arg("-o").arg(&output_template);

    if !selection.subtitle_langs.is_empty() {
        let sub_langs = selection.subtitle_langs.join(",");
        // Most YouTube subtitle languages (e.g. fa) exist ONLY as auto-generated
        // captions, so both --write-subs and --write-auto-subs are required —
        // otherwise yt-dlp reports "no subtitles for the requested languages"
        // and produces no subtitle output at all.
        cmd.arg("--write-subs").arg("--write-auto-subs")
            .arg("--sub-langs").arg(&sub_langs);
        match selection.subtitle_mode {
            SubtitleMode::Embedded => {
                // Embed into mp4 (yt-dlp converts vtt -> mov_text automatically).
                cmd.arg("--embed-subs");
            }
            SubtitleMode::File => {
                // Deliver as standalone file(s); convert to srt for broad player support.
                cmd.arg("--convert-subs").arg("srt");
            }
        }
        log_trace(trace_id, "download_subtitle_args", &format!(
            "sub_langs={sub_langs} mode={:?} write_auto=true", selection.subtitle_mode
        ));
    }

    cmd.arg(&req.webpage_url).stdout(Stdio::piped()).stderr(Stdio::piped());
    log_trace(trace_id, "download_args", &format!(
        "cookie_spec={} format_spec={format_spec}", req.cookie_spec
    ));

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log_trace(trace_id, "download_spawn_failed", &e.to_string());
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())])).await;
            return;
        }
    };

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(&'static str, String)>(64);
    let tx_out = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await { let _ = tx_out.send(("stdout", line)).await; }
    });
    let tx_err = tx;
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await { let _ = tx_err.send(("stderr", line)).await; }
    });

    let mut filepath: Option<String> = None;
    let mut last_edit = std::time::Instant::now() - EDIT_THROTTLE;
    let mut last_percent_int = -1;
    let mut stderr_tail = String::new();

    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some((source, line)) = msg else { break; };
                if let Some(snap) = parse_progress_line(&line) {
                    let now = std::time::Instant::now();
                    if snap.percent_int != last_percent_int && now.duration_since(last_edit) >= EDIT_THROTTLE {
                        last_percent_int = snap.percent_int;
                        last_edit = now;
                        log_trace(trace_id, "download_progress", &format!(
                            "src={source} percent={} downloaded={} total={} speed={} eta={}",
                            snap.percent, snap.downloaded, snap.total, snap.speed, snap.eta
                        ));
                        edit_progress_status(&api, status_chat_id, status_message_id,
                            format_progress_body(&snap, &quality_label), request_id).await;
                    }
                    continue;
                }
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() { continue; }
                let is_subtitle = trimmed.ends_with(".srt") || trimmed.ends_with(".vtt");
                if source == "stdout" && trimmed.starts_with('/') && !is_subtitle && tokio::fs::metadata(&trimmed).await.is_ok() {
                    filepath = Some(trimmed.clone());
                    log_trace(trace_id, "download_filepath", &trimmed);
                } else if source == "stderr" {
                    stderr_tail = trimmed.clone();
                    log_trace(trace_id, "yt_dlp_stderr", &trimmed);
                } else {
                    log_trace(trace_id, "yt_dlp_stdout", &trimmed);
                }
            }
            _ = &mut cancel_fut => {
                log_trace(trace_id, "download_cancelled", "cancel signal during download");
                let _ = child.kill().await;
                edit_status(&api, status_chat_id, status_message_id, t("youtube.download.cancelled")).await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }
        }
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            log_trace(trace_id, "download_wait_failed", &e.to_string());
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", &e.to_string())])).await;
            return;
        }
    };

    if !status.success() {
        let err = if stderr_tail.is_empty() { format!("exit {status}") } else { stderr_tail };
        log_trace(trace_id, "download_failed", &format!("status={status} err={err}"));
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.failed", &[("error", &err)])).await;
        cleanup_dir(&dir, trace_id).await;
        return;
    }

    let path = match filepath.or_else(|| pick_largest_file(&dir)) {
        Some(p) => p,
        None => {
            log_trace(trace_id, "download_no_filepath", "no output file located");
            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.failed", &[("error", "no output file")])).await;
            cleanup_dir(&dir, trace_id).await;
            return;
        }
    };

    log_trace(trace_id, "download_complete", &format!("path={path}"));

    let file_size_bytes = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
    if let Some(jid) = stats_job_id {
        stats::record_download_done(jid, file_size_bytes as i64).await;
    }

    let codec_name = t(selection.codec.label_key());
    let bitrate_str = find_format(&req, height, codec)
        .and_then(|f| f.bitrate)
        .map(|b| format!("{:.0}", b))
        .unwrap_or_else(|| "?".to_string());
    let thumb_path = fetch_thumbnail(&req.thumbnail_url, &dir, trace_id).await;

    let file_size_mb = file_size_bytes / (1024 * 1024);
    log_trace(trace_id, "upload_size_check", &format!("size_mb={file_size_mb} max_mb={MAX_SIZE_MB}"));

    let upload_ok = if file_size_mb > MAX_SIZE_MB {
        let num_parts = ((file_size_mb + TARGET_PART_MB - 1) / TARGET_PART_MB) as usize;
        log_trace(trace_id, "split_needed", &format!("size_mb={file_size_mb} parts={num_parts}"));
        edit_status(&api, status_chat_id, status_message_id,
            tf("youtube.download.splitting", &[("parts", &num_parts.to_string())])).await;

        let part_paths = match split_video(&path, &dir, num_parts, req.duration, trace_id).await {
            Ok(p) => p,
            Err(e) => {
                log_trace(trace_id, "split_failed", &e);
                edit_status(&api, status_chat_id, status_message_id,
                    tf("youtube.download.split_failed", &[("error", &e)])).await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }
        };

        let total = part_paths.len();
        let mut all_ok = true;
        for (i, part_path) in part_paths.iter().enumerate() {
            let part_num = i + 1;
            let part_size_mb = tokio::fs::metadata(part_path).await
                .map(|m| m.len() / (1024 * 1024)).unwrap_or(0);
            log_trace(trace_id, "split_part_size", &format!("part={part_num}/{total} size_mb={part_size_mb}"));
            if part_size_mb > MAX_SIZE_MB {
                log_trace(trace_id, "split_part_too_large", &format!("part={part_num} size_mb={part_size_mb}"));
                edit_status(&api, status_chat_id, status_message_id,
                    tf("youtube.download.split_failed", &[("error", &format!("part {part_num} still {part_size_mb}MB"))])).await;
                cleanup_dir(&dir, trace_id).await;
                return;
            }

            edit_status(&api, status_chat_id, status_message_id,
                tf("youtube.download.uploading_part", &[
                    ("part", &part_num.to_string()), ("total", &total.to_string()),
                ])).await;

            let bot_username = crate::config::bot_username().to_string();
            let caption = tf("youtube.download.caption_part", &[
                ("title", &req.title), ("quality", &quality_label),
                ("codec", &codec_name), ("bitrate", &bitrate_str),
                ("part", &part_num.to_string()), ("total", &total.to_string()),
                ("username", &bot_username),
            ]);
            let caption_entities = entities_for_text(&caption);
            let params = build_part_params(part_path, req.chat_id, &thumb_path,
                part_num == 1, caption, caption_entities, height);

            log_trace(trace_id, "upload_part_start", &format!("part={part_num}/{total} path={part_path}"));
            let ok = send_video_with_progress(&api, params, req.chat_id, status_chat_id,
                status_message_id, request_id, &quality_label, &mut cancel_fut, trace_id).await;
            if !ok { all_ok = false; break; }
            log_trace(trace_id, "upload_part_ok", &format!("part={part_num}/{total}"));
        }
        all_ok
    } else {
        edit_status(&api, status_chat_id, status_message_id, t("youtube.download.uploading")).await;
        let bot_username = crate::config::bot_username().to_string();
        let caption = tf("youtube.download.caption", &[
            ("title", &req.title), ("quality", &quality_label),
            ("codec", &codec_name), ("bitrate", &bitrate_str),
            ("username", &bot_username),
        ]);
        let caption_entities = entities_for_text(&caption);
        let params = build_single_params(&path, req.chat_id, &thumb_path,
            caption, caption_entities, height, req.duration);
        log_trace(trace_id, "upload_start", &format!("path={path}"));
        send_video_with_progress(&api, params, req.chat_id, status_chat_id,
            status_message_id, request_id, &quality_label, &mut cancel_fut, trace_id).await
    };

    // In File mode, deliver the standalone subtitle file(s) as documents.
    // (Embedded mode bakes them into the mp4 and needs no separate upload.)
    if upload_ok && selection.subtitle_mode == SubtitleMode::File && !selection.subtitle_langs.is_empty() {
        let count = send_subtitle_files(&api, &dir, req.chat_id, &req.title, trace_id).await;
        log_trace(trace_id, "subtitle_upload_done", &format!("files_sent={count}"));
    }

    if upload_ok {
        if let Some(jid) = stats_job_id {
            stats::record_upload_done(jid, file_size_bytes as i64).await;
        }
        let _ = api.delete_message(
            &DeleteMessageParams::builder()
                .chat_id(status_chat_id)
                .message_id(status_message_id)
                .build(),
        ).await;
    }

    cleanup_dir(&dir, trace_id).await;
}
