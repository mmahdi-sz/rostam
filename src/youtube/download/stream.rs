use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use frankenstein::client_reqwest::Bot;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::i18n::t;
use crate::youtube::trace::log_trace;

use super::helpers::cleanup_dir;
use super::progress::{format_progress_body, parse_progress_line};
use super::status::{edit_progress_status, edit_status};

pub const EDIT_THROTTLE: Duration = Duration::from_secs(1);

pub enum YtdlpStreamResult {
    Completed {
        filepath: Option<String>,
        stderr_tail: String,
        status: ExitStatus,
    },
    Cancelled,
    Failed,
}

pub async fn run_ytdlp_process(
    mut cmd: tokio::process::Command,
    api: &Bot,
    status_chat_id: i64,
    status_message_id: i32,
    request_id: u64,
    quality_label: &str,
    cancel_fut: &mut (impl std::future::Future<Output = ()> + Unpin),
    trace_id: u64,
    dir: &Path,
) -> YtdlpStreamResult {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log_trace(trace_id, "download_spawn_failed", &e.to_string());
            edit_status(
                api,
                status_chat_id,
                status_message_id,
                t("youtube.download.failed"),
            )
            .await;
            return YtdlpStreamResult::Failed;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        log_trace(trace_id, "download_spawn_failed", "piped stdout missing");
        edit_status(
            api,
            status_chat_id,
            status_message_id,
            t("youtube.download.failed"),
        )
        .await;
        return YtdlpStreamResult::Failed;
    };
    let Some(stderr) = child.stderr.take() else {
        log_trace(trace_id, "download_spawn_failed", "piped stderr missing");
        edit_status(
            api,
            status_chat_id,
            status_message_id,
            t("youtube.download.failed"),
        )
        .await;
        return YtdlpStreamResult::Failed;
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(&'static str, String)>(64);
    let tx_out = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_out.send(("stdout", line)).await;
        }
    });
    let tx_err = tx;
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_err.send(("stderr", line)).await;
        }
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
                        edit_progress_status(api, status_chat_id, status_message_id,
                            format_progress_body(&snap, quality_label), request_id).await;
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
            _ = &mut *cancel_fut => {
                log_trace(trace_id, "download_cancelled", "cancel signal during download");
                let _ = child.kill().await;
                edit_status(api, status_chat_id, status_message_id, t("youtube.download.cancelled")).await;
                cleanup_dir(dir, trace_id).await;
                return YtdlpStreamResult::Cancelled;
            }
        }
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            log_trace(trace_id, "download_wait_failed", &e.to_string());
            edit_status(
                api,
                status_chat_id,
                status_message_id,
                t("youtube.download.failed"),
            )
            .await;
            return YtdlpStreamResult::Failed;
        }
    };

    YtdlpStreamResult::Completed {
        filepath,
        stderr_tail,
        status,
    }
}
