use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message},
};
use crate::bot::edit_to_tools;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::btn_icon;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{t, tf};
use crate::log::next_trace_id;
use crate::rank;

fn surge_cmd(args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("surge");
    cmd.args(args).arg("--host").arg(crate::config::surge_host());
    cmd
}
const MAX_PART_BYTES: u64 = 2000 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_SECS: u64 = 2 * 3600;
const POLL_INTERVAL_SECS: u64 = 3;

pub const CB_TOOLS_SURGE: &str = "tools:surge";
pub const CB_SURGE_CANCEL: &str = "surge:cancel";

fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(&t("start.back"), CB_SURGE_CANCEL, "back")]])
        .build()
}

// ── menu entry ───────────────────────────────────────────────────────────────

pub async fn enter_surge_dl(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("surge_dl", trace_id, user_id, "clicked" => CB_TOOLS_SURGE);
    flow_manager.set(user_id, FlowState::AwaitingSurgeUrlInput);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id)
        .text(t("surge.prompt"))
        .reply_markup(cancel_keyboard())
        .build();
    let r = api.edit_message_text(&params).await;
    log_ev!("surge_dl", trace_id, "prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

pub async fn handle_surge_cancel(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("surge_dl", trace_id, "cancel", "user_id" => user_id);
    flow_manager.clear(user_id);
    let _ = edit_to_tools(api, chat_id, message_id).await;
}

// ── URL intake ─────────────────────────────────────────────────────────────

fn is_direct_link(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("http://") || text.starts_with("https://")
}

pub async fn handle_surge_text(
    api: &Bot, message: &Message, user_id: i64, flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("surge_dl", trace_id, user_id, "clicked" => "send_surge_url");

    let Some(url) = message.text.as_deref().map(str::trim) else { return };
    if !is_direct_link(url) {
        log_ev!("surge_dl", trace_id, "invalid_url", "input" => url, "=>" => "reject");
        let _ = crate::bot::send_text(api, chat_id, &t("surge.invalid_url")).await;
        return;
    }

    // ── چک ترافیک (روزانه + ماهانه) قبل از شروع دانلود ──
    if let Some(db) = database.as_ref() {
        let client = db.client();
        let user_rank = rank::effective_rank(client, user_id).await;
        let daily_limit = user_rank.daily_traffic_bytes();
        let monthly_limit = user_rank.monthly_traffic_bytes();
        let first_upload_at = rank::quota::get_first_upload_at(client, user_id).await.unwrap_or_else(now_epoch);
        let daily_used = rank::quota::get_daily_traffic(client, user_id).await.unwrap_or(0) as u64;
        let monthly_used = rank::quota::get_monthly_traffic(client, user_id, first_upload_at).await.unwrap_or(0) as u64;

        let block = if daily_used >= daily_limit {
            Some((tf("youtube.traffic_daily_limit", &[("limit", &fmt_traffic_fa(daily_limit))]), user_rank.traffic_daily_next_rank()))
        } else if monthly_used >= monthly_limit {
            Some((tf("youtube.traffic_monthly_limit", &[("limit", &fmt_traffic_fa(monthly_limit))]), user_rank.traffic_monthly_next_rank()))
        } else {
            None
        };

        if let Some((label, next_rank)) = block {
            log_ev!("surge_dl", trace_id, "traffic_paywall", "=>" => "blocked");
            if let Some(min_rank) = next_rank {
                rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
            } else {
                let _ = crate::bot::send_text(api, chat_id, &label).await;
            }
            return;
        }
    }

    flow_manager.clear(user_id);
    log_ev!("surge_dl", trace_id, "url_accepted", "url" => url);

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("surge.queued"))
        .build();
    let status_message_id = match api.send_message(&params).await {
        Ok(r) => r.result.message_id,
        Err(e) => {
            log_ev!("surge_dl", trace_id, "queue_message_failed", "=>" => format!("fail err={e}"));
            return;
        }
    };

    let api2 = api.clone();
    let url_owned = url.to_string();
    tokio::spawn(async move {
        run_surge_download(api2, chat_id, status_message_id, user_id, url_owned, trace_id).await;
    });
}

// ── download orchestration ───────────────────────────────────────────────────

struct SurgeDetail {
    filename: String,
    dest_path: String,
    total_size: u64,
    downloaded: u64,
    progress: f64,
    speed: f64,
    avg_speed: f64,
    status: String,
}

async fn run_surge_download(
    api: Bot, chat_id: i64, message_id: i32, user_id: i64, url: String, trace_id: u64,
) {
    let stats_job_id = crate::stats::record_download_start(user_id).await;
    let dir = format!("{}/{user_id}", crate::config::surge_downloads_root());
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        log_ev!("surge_dl", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        edit_status(&api, chat_id, message_id, &t("surge.error.download_failed")).await;
        return;
    }

    let before_ids = list_ids(trace_id).await;

    log_ev!("surge_dl", trace_id, "add_spawn", "url" => &url, "dir" => &dir);
    let add_ok = run_surge_add(&url, &dir).await;
    if !add_ok {
        log_ev!("surge_dl", trace_id, "add_failed", "=>" => "fail");
        crate::stats::record_error_global("surge_dl", "surge add failed").await;
        edit_status(&api, chat_id, message_id, &t("surge.error.download_failed")).await;
        return;
    }

    let Some(job_id) = find_new_id(&before_ids, trace_id).await else {
        log_ev!("surge_dl", trace_id, "job_not_found", "=>" => "fail");
        crate::stats::record_error_global("surge_dl", "surge job id not found after add").await;
        edit_status(&api, chat_id, message_id, &t("surge.error.download_failed")).await;
        return;
    };
    log_ev!("surge_dl", trace_id, "job_found", "id" => &job_id);

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
                    edit_status(&api, chat_id, message_id, &tf("surge.progress", &[("percent", &percent.to_string())])).await;
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

    log_ev!("surge_dl", trace_id, "download_done", "filename" => &detail.filename, "path" => &detail.dest_path,
        "size" => fmt_bytes(detail.downloaded), "avg_speed" => fmt_speed(detail.avg_speed));
    edit_status(&api, chat_id, message_id, &t("surge.done")).await;

    let file_path = PathBuf::from(&detail.dest_path);
    let result = if detail.downloaded <= MAX_PART_BYTES {
        send_single_file(&api, chat_id, &file_path).await
    } else {
        send_split_file(&api, chat_id, &file_path, trace_id).await
    };

    match result {
        Ok(()) => {
            log_ev!("surge_dl", trace_id, "result_sent", "=>" => "ok");
            let _ = api.delete_message(
                &frankenstein::methods::DeleteMessageParams::builder()
                    .chat_id(chat_id).message_id(message_id).build(),
            ).await;
            if let Some(jid) = stats_job_id {
                crate::stats::record_upload_done(jid, user_id, detail.downloaded as i64).await;
                log_ev!("surge_dl", trace_id, "traffic_added", "bytes" => detail.downloaded);
            }
            crate::stats::record_event_user(user_id, "surge_dl", "download", "ok", detail.downloaded as i64).await;
        }
        Err(e) => {
            log_ev!("surge_dl", trace_id, "result_send_failed", "=>" => format!("fail err={e}"));
            crate::stats::record_error_global("surge_dl", &format!("send failed: {e}")).await;
            edit_status(&api, chat_id, message_id, &t("surge.error.send_failed")).await;
            crate::stats::record_event_user(user_id, "surge_dl", "download", "fail", 0).await;
        }
    }
}

async fn send_single_file(api: &Bot, chat_id: i64, path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(path.to_path_buf())
        .build();
    api.send_document(&params).await?;
    let _ = tokio::fs::remove_file(path).await;
    Ok(())
}

async fn send_split_file(
    api: &Bot, chat_id: i64, path: &Path, trace_id: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let archive_base = path.with_extension("rar");
    log_ev!("surge_dl", trace_id, "rar_spawn", "archive" => archive_base.display());

    let mut cmd = tokio::process::Command::new("rar");
    cmd.arg("a")
        .arg(format!("-v{}m", MAX_PART_BYTES / (1024 * 1024)))
        .arg("-m0")
        .arg(&archive_base)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let status = tokio::time::timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS), child.wait()).await??;
    if !status.success() {
        return Err(format!("rar exit {status}").into());
    }

    let parts = list_rar_parts(&archive_base).await?;
    log_ev!("surge_dl", trace_id, "rar_done", "parts" => parts.len());

    let total = parts.len();
    for (i, part) in parts.iter().enumerate() {
        let caption = tf("surge.sending_part", &[("n", &(i + 1).to_string()), ("total", &total.to_string())]);
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(part.clone())
            .caption(&caption)
            .build();
        api.send_document(&params).await?;
        log_ev!("surge_dl", trace_id, "part_sent", "n" => i + 1, "total" => total);
    }

    let _ = tokio::fs::remove_file(path).await;
    for part in parts {
        let _ = tokio::fs::remove_file(part).await;
    }
    Ok(())
}

async fn list_rar_parts(archive_base: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    let dir = archive_base.parent().ok_or("no parent dir")?;
    let stem = archive_base.file_stem().and_then(|s| s.to_str()).ok_or("no stem")?;
    let prefix = format!("{stem}.part");
    let mut parts = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".rar") {
            parts.push(entry.path());
        }
    }
    parts.sort();
    Ok(parts)
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// قالب‌بندی حجم به فارسی: «۵ گیگابایت» / «۷۵۰ مگابایت».
fn fmt_traffic_fa(bytes: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    let b = bytes as f64;
    let (num, unit) = if b >= GB {
        let g = b / GB;
        if (g.round() - g).abs() < 0.05 {
            (format!("{:.0}", g.round()), crate::i18n::t("youtube.unit_gb"))
        } else {
            (format!("{:.1}", g), crate::i18n::t("youtube.unit_gb"))
        }
    } else {
        (format!("{:.0}", (b / MB).round()), crate::i18n::t("youtube.unit_mb"))
    };
    format!("{} {}", crate::i18n::to_fa_digits(&num), unit)
}

fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 { format!("{:.2}GB", mb / 1024.0) } else { format!("{mb:.1}MB") }
}

// surge already reports speed/avg_speed in MB/s (confirmed against a real
// download's total_size/time_taken) — no unit conversion needed here.
fn fmt_speed(mb_per_sec: f64) -> String {
    format!("{mb_per_sec:.2}MB/s")
}

async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: &str) {
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id).text(text).build();
    let _ = api.edit_message_text(&params).await;
}

// ── surge CLI wrapper ────────────────────────────────────────────────────────

async fn run_surge_add(url: &str, dir: &str) -> bool {
    let mut cmd = surge_cmd(&["add", url, "-o", dir]);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let Ok(mut child) = cmd.spawn() else { return false };
    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) => status.success(),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            false
        }
    }
}

async fn list_ids(trace_id: u64) -> Vec<String> {
    let output = surge_cmd(&["ls", "--json"]).output().await;
    let Ok(output) = output else {
        log_ev!("surge_dl", trace_id, "list_ids_spawn_failed");
        return vec![];
    };
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    entries.as_array()
        .map(|arr| arr.iter().filter_map(|e| e.get("id")?.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

async fn find_new_id(before: &[String], trace_id: u64) -> Option<String> {
    // `surge add` doesn't print the new job's id, so we diff `ls` before/after —
    // retry a few times in case the daemon hasn't registered the job yet.
    for _ in 0..5 {
        let after = list_ids(trace_id).await;
        if let Some(id) = after.into_iter().find(|id| !before.contains(id)) {
            return Some(id);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}

async fn fetch_detail(id: &str) -> Option<SurgeDetail> {
    let output = surge_cmd(&["ls", id, "--json"]).output().await.ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    Some(SurgeDetail {
        filename: json.get("filename")?.as_str()?.to_string(),
        dest_path: json.get("dest_path")?.as_str()?.to_string(),
        total_size: json.get("total_size")?.as_u64()?,
        downloaded: json.get("downloaded")?.as_u64()?,
        progress: json.get("progress")?.as_f64()?,
        speed: json.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.0),
        avg_speed: json.get("avg_speed").and_then(|v| v.as_f64()).unwrap_or(0.0),
        status: json.get("status")?.as_str()?.to_string(),
    })
}
