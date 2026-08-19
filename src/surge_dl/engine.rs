use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use frankenstein::{
    client_reqwest::Bot,
    methods::SendDocumentParams,
};

use crate::common::cpu_broker::CpuBrokerGuard;
use crate::common::dir::TempDirGuard;
use crate::i18n::{entities_for_text, t, tf};
use crate::surge_dl::client::{fetch_detail, find_job_id_by_url, list_surge_job_ids, run_surge_add};
use crate::surge_dl::types::{
    DOWNLOAD_TIMEOUT_SECS, MAX_PART_BYTES, POLL_INTERVAL_SECS,
};
use crate::surge_dl::ui::{build_bar, edit_status, fmt_bytes, fmt_speed, show_sent_menu};

pub(crate) async fn run_surge_download(
    api: Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    url: String,
    rename_to: Option<String>,
    trace_id: u64,
) {
    let stats_job_id = crate::stats::record_download_start(user_id, "surge_dl").await;

    let _active_dl_guard = crate::metrics::ActiveDownloadGuard::new();
    let _duration_guard = crate::metrics::RequestDurationGuard::new("surge_dl");
    let job_nonce = rand::random::<u32>();
    let dir = format!(
        "{}/{user_id}/job_{trace_id}_{job_nonce}",
        crate::config::surge_downloads_root()
    );
    let dir_path = PathBuf::from(&dir);
    let _cleanup_guard = TempDirGuard::from_path(dir_path);

    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_ev!("surge_dl", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        crate::stats::record_error_global("surge_dl", &format!("mkdir failed: {e}")).await;
        edit_status(&api, chat_id, message_id, &t("surge.error.download_failed")).await;
        return;
    }

    let before_ids = list_surge_job_ids().await;
    log_ev!("surge_dl", trace_id, "add_spawn", "url" => &url, "dir" => &dir);
    let add_ok = run_surge_add(&url, &dir).await;
    if !add_ok {
        log_ev!("surge_dl", trace_id, "add_failed", "=>" => "fail");
        crate::stats::record_error_global("surge_dl", "surge add failed").await;
        edit_status(&api, chat_id, message_id, &t("surge.error.download_failed")).await;
        return;
    }

    let Some(job_id) = find_job_id_by_url(&url, &before_ids, trace_id).await else {
        log_ev!("surge_dl", trace_id, "job_not_found", "=>" => "fail");
        crate::stats::record_error_global("surge_dl", "surge job id not found after add").await;
        edit_status(&api, chat_id, message_id, &t("surge.error.download_failed")).await;
        return;
    };
    log_ev!("surge_dl", trace_id, "job_found", "id" => &job_id);

    let download_start = std::time::Instant::now();
    let mut last_percent: i64 = -1;
    let mut elapsed = 0u64;
    let detail = loop {
        match fetch_detail(&job_id).await {
            Some(d) if d.status == "completed" => break Some(d),
            Some(d) if d.status == "error" => {
                log_ev!("surge_dl", trace_id, "poll", "filename" => &d.filename, "status" => &d.status, "=>" => "fail");
                break None;
            }
            Some(d) => {
                let percent = d.progress.round() as i64;
                log_ev!("surge_dl", trace_id, "poll", "filename" => &d.filename,
                    "downloaded" => fmt_bytes(d.downloaded), "total" => fmt_bytes(d.total_size),
                    "percent" => percent, "speed" => fmt_speed(d.speed));
                if percent != last_percent {
                    last_percent = percent;
                    let body = tf(
                        "surge.progress",
                        &[
                            ("name", &d.filename),
                            ("bar", &build_bar(percent as f32)),
                            ("percent", &percent.to_string()),
                            ("downloaded", &fmt_bytes(d.downloaded)),
                            ("total", &fmt_bytes(d.total_size)),
                            ("speed", &fmt_speed(d.speed)),
                        ],
                    );
                    edit_status(&api, chat_id, message_id, &body).await;
                }
            }
            None => {}
        }
        if elapsed >= DOWNLOAD_TIMEOUT_SECS {
            log_ev!("surge_dl", trace_id, "poll_timeout", "=>" => "fail");
            break None;
        }
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        elapsed += POLL_INTERVAL_SECS;
    };

    let Some(detail) = detail else {
        crate::stats::record_error_global("surge_dl", "download failed or timed out").await;
        edit_status(&api, chat_id, message_id, &t("surge.error.download_failed")).await;
        crate::stats::record_event_user(user_id, "surge_dl", "download", "fail", 0).await;
        return;
    };
    let download_elapsed = download_start.elapsed();

    let file_path = std::path::Path::new(&dir).join(&detail.filename);
    log_ev!("surge_dl", trace_id, "download_done", "filename" => &detail.filename, "path" => file_path.display(),
        "size" => fmt_bytes(detail.downloaded), "avg_speed" => fmt_speed(detail.avg_speed));
    edit_status(&api, chat_id, message_id, &t("surge.done")).await;
    let file_path = match rename_to {
        Some(new_name) => {
            let renamed = file_path.with_file_name(&new_name);
            match tokio::fs::rename(&file_path, &renamed).await {
                Ok(()) => {
                    log_ev!("surge_dl", trace_id, "renamed", "to" => &new_name);
                    renamed
                }
                Err(e) => {
                    log_ev!("surge_dl", trace_id, "rename_failed", "=>" => format!("fail err={e}"));
                    file_path
                }
            }
        }
        None => file_path,
    };
    let upload_start = std::time::Instant::now();
    let result = if detail.downloaded <= MAX_PART_BYTES {
        send_single_file(&api, chat_id, message_id, &file_path).await
    } else {
        send_split_file(&api, chat_id, message_id, &file_path, user_id, trace_id).await
    };
    let upload_elapsed = upload_start.elapsed();

    match result {
        Ok(()) => {
            log_ev!("surge_dl", trace_id, "result_sent", "=>" => "ok");
            tokio::time::sleep(Duration::from_millis(500)).await;
            show_sent_menu(
                &api,
                chat_id,
                message_id,
                detail.downloaded,
                download_elapsed,
                upload_elapsed,
            )
            .await;
            if let Some(jid) = stats_job_id {
                let up_speed = if upload_elapsed.as_secs_f64() > 0.0 {
                    detail.downloaded as f64 / upload_elapsed.as_secs_f64()
                } else {
                    0.0
                };
                crate::stats::record_upload_done(
                    jid,
                    user_id,
                    detail.downloaded as i64,
                    Some(up_speed as i64),
                    Some(1),
                )
                .await;
                log_ev!("surge_dl", trace_id, "traffic_added", "bytes" => detail.downloaded);
            }

            crate::stats::record_event_user(
                user_id,
                "surge_dl",
                "download",
                "ok",
                detail.downloaded as i64,
            )
            .await;
        }
        Err(e) => {
            log_ev!("surge_dl", trace_id, "result_send_failed", "=>" => format!("fail err={e}"));
            crate::stats::record_error_global("surge_dl", &format!("send failed: {e}")).await;
            edit_status(&api, chat_id, message_id, &t("surge.error.send_failed")).await;
            crate::stats::record_event_user(user_id, "surge_dl", "download", "fail", 0).await;
        }
    }
}

pub(crate) async fn send_single_file(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(path.to_path_buf())
        .build();
    use crate::bot::send_file_with_upload_ticker;
    send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendDocument",
        &params,
        path,
        chat_id,
        message_id,
        "transfer.stage.sending_document",
        None,
    )
    .await?;
    Ok(())
}

pub(crate) async fn send_split_file(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    path: &Path,
    user_id: i64,
    trace_id: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let archive_base = path.with_file_name(format!("{stem}.archive.rar"));
    log_ev!("surge_dl", trace_id, "rar_spawn", "archive" => archive_base.display());

    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "surge_dl").await;

    let mut cmd = tokio::process::Command::new("rar");
    cmd.arg("a")
        .arg(format!("-v{}m", MAX_PART_BYTES / (1024 * 1024)))
        .arg("-m0")
        .arg(&archive_base)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = cmd.spawn()?;
    let status =
        tokio::time::timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS), child.wait()).await??;
    cpu_guard.release().await;

    if !status.success() {
        return Err(format!("rar exit {status}").into());
    }

    let parts = list_rar_parts(&archive_base).await?;
    log_ev!("surge_dl", trace_id, "rar_done", "parts" => parts.len());

    let total = parts.len();
    use crate::bot::send_file_with_upload_ticker;
    for (i, part) in parts.iter().enumerate() {
        let caption = tf(
            "surge.sending_part",
            &[("n", &(i + 1).to_string()), ("total", &total.to_string())],
        );
        let caption_entities = entities_for_text(&caption);
        let mut params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(part.clone())
            .caption(&caption)
            .build();
        if !caption_entities.is_empty() {
            params.caption_entities = Some(caption_entities);
        }
        let status_mid = message_id;
        let _ = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            api,
            "sendDocument",
            &params,
            part,
            chat_id,
            status_mid,
            "transfer.stage.sending_document",
            None,
        )
        .await?;
        log_ev!("surge_dl", trace_id, "part_sent", "n" => i + 1, "total" => total);
    }

    Ok(())
}

async fn list_rar_parts(archive_base: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut parts = Vec::new();
    let parent = archive_base.parent().unwrap_or_else(|| Path::new("."));
    let stem = archive_base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("archive");
    let mut dir = tokio::fs::read_dir(parent).await?;
    while let Some(entry) = dir.next_entry().await? {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(stem)
            && (name_str.ends_with(".rar") || name_str.contains(".part"))
        {
            parts.push(entry.path());
        }
    }
    parts.sort();
    Ok(parts)
}
