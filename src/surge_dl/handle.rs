use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::bot::edit_to_tools;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::btn_icon;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{entities_for_text, t, tf};
use crate::log::next_trace_id;
use crate::rank;
use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};

fn surge_cmd(args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("surge");
    cmd.args(args)
        .arg("--host")
        .arg(crate::config::surge_host());
    cmd
}
const MAX_PART_BYTES: u64 = 2000 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_SECS: u64 = 2 * 3600;
const POLL_INTERVAL_SECS: u64 = 3;

pub const CB_TOOLS_SURGE: &str = "tools:surge";
pub const CB_SURGE_CANCEL: &str = "surge:cancel";
pub const CB_SURGE_CONFIRM_ORIGINAL: &str = "surge:confirm:orig";
pub const CB_SURGE_CONFIRM_RENAME: &str = "surge:confirm:rename";

fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            CB_SURGE_CANCEL,
            "back",
        )]])
        .build()
}

fn confirm_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon(
                    &t("surge.confirm_original_button"),
                    CB_SURGE_CONFIRM_ORIGINAL,
                    "check",
                ),
                btn_icon(
                    &t("surge.confirm_rename_button"),
                    CB_SURGE_CONFIRM_RENAME,
                    "edit",
                ),
            ],
            vec![btn_icon(&t("start.back"), CB_SURGE_CANCEL, "back")],
        ])
        .build()
}

// ── menu entry ───────────────────────────────────────────────────────────────

pub async fn enter_surge_dl(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("surge_dl", trace_id, user_id, "clicked" => CB_TOOLS_SURGE);
    flow_manager.set(user_id, FlowState::AwaitingSurgeUrlInput);
    let text = t("surge.prompt");
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .reply_markup(cancel_keyboard())
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    let r = api.edit_message_text(&params).await;
    log_ev!("surge_dl", trace_id, "prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

pub async fn handle_surge_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("surge_dl", trace_id, "cancel", "user_id" => user_id);
    flow_manager.clear(user_id);
    let _ = edit_to_tools(api, chat_id, message_id).await;
}

// ── URL intake ─────────────────────────────────────────────────────────────

pub fn available_disk_space(path: &str) -> std::io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path_obj = Path::new(path);
    let c_path = CString::new(path_obj.as_os_str().as_bytes())?;
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
            let free_bytes = (stat.f_bavail as u64) * (stat.f_frsize as u64);
            Ok(free_bytes)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

pub fn detect_social_platform(text: &str) -> Option<&'static str> {
    let text = text.trim();
    if !crate::validation::is_safe_url(text) {
        return None;
    }
    let Ok(parsed) = reqwest::Url::parse(text) else {
        return None;
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_lowercase();
    if host == "youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
        || host.ends_with(".youtu.be")
    {
        return Some("youtube");
    }
    if host == "t.me"
        || host.ends_with(".t.me")
        || host == "telegram.org"
        || host.ends_with(".telegram.org")
        || host == "telegram.me"
        || host.ends_with(".telegram.me")
    {
        return Some("telegram");
    }
    if host == "instagram.com"
        || host.ends_with(".instagram.com")
        || host == "instagr.am"
        || host.ends_with(".instagr.am")
    {
        return Some("instagram");
    }
    if host == "tiktok.com" || host.ends_with(".tiktok.com") || host == "vt.tiktok.com" {
        return Some("tiktok");
    }
    if host == "twitter.com"
        || host.ends_with(".twitter.com")
        || host == "x.com"
        || host.ends_with(".x.com")
    {
        return Some("twitter");
    }
    if host == "pinterest.com"
        || host.ends_with(".pinterest.com")
        || host == "pin.it"
        || host.ends_with(".pin.it")
    {
        return Some("pinterest");
    }
    if host == "facebook.com"
        || host.ends_with(".facebook.com")
        || host == "fb.watch"
        || host == "fb.com"
    {
        return Some("facebook");
    }
    if host == "threads.net" || host.ends_with(".threads.net") {
        return Some("threads");
    }
    if host == "soundcloud.com" || host.ends_with(".soundcloud.com") {
        return Some("soundcloud");
    }
    if host == "spotify.com" || host.ends_with(".spotify.com") {
        return Some("spotify");
    }
    if host == "aparat.com" || host.ends_with(".aparat.com") {
        return Some("aparat");
    }
    if host == "rubika.ir"
        || host.ends_with(".rubika.ir")
        || host == "rubika.com"
        || host.ends_with(".rubika.com")
    {
        return Some("rubika");
    }
    if host == "eitaa.com" || host.ends_with(".eitaa.com") {
        return Some("eitaa");
    }
    None
}

pub fn is_direct_link(text: &str) -> bool {
    let text = text.trim();
    if !crate::validation::is_safe_url(text) {
        return false;
    }

    let Ok(parsed) = reqwest::Url::parse(text) else {
        return false;
    };

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return false;
    }

    if let Some(host) = parsed.host_str() {
        let host = host.to_lowercase();
        if host == "t.me"
            || host.ends_with(".t.me")
            || host == "telegram.org"
            || host.ends_with(".telegram.org")
            || host == "telegram.me"
            || host.ends_with(".telegram.me")
        {
            return false;
        }
    } else {
        return false;
    }

    true
}

pub async fn handle_surge_text(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("surge_dl", trace_id, user_id, "clicked" => "send_surge_url");

    let Some(url) = message.text.as_deref().map(str::trim) else {
        return;
    };

    if let Some(platform) = detect_social_platform(url) {
        if platform != "youtube" {
            log_ev!("surge_dl", trace_id, "unsupported_social_platform", "platform" => platform, "input" => url);
            let platform_name = t(&format!("platforms.{platform}"));
            let text = tf("surge.unsupported_platform", &[("platform", &platform_name)]);
            let _ = crate::bot::send_text(api, chat_id, &text).await;
            let _ = crate::bot::send_tools_menu(api, chat_id).await;
            return;
        }
    }

    if !is_direct_link(url) {
        log_ev!("surge_dl", trace_id, "invalid_url", "input" => url, "=>" => "reject");
        let _ = crate::bot::send_text_with_back(api, chat_id, &t("surge.invalid_url")).await;
        return;
    }

    log_ev!("surge_dl", trace_id, "url_accepted", "url" => url);

    let (filename, size_bytes) = probe_url(url).await;
    log_ev!("surge_dl", trace_id, "probed", "name" => &filename, "size" => format!("{size_bytes:?}"));

    // ── چک فضای آزاد دیسک (کسر ۲۰٪ بافر) ──
    let downloads_root = crate::config::surge_downloads_root();
    if let Ok(free_bytes) = available_disk_space(&downloads_root) {
        let max_allowed = (free_bytes as f64 * 0.8) as u64;
        if let Some(sb) = size_bytes {
            if sb > max_allowed {
                log_ev!("surge_dl", trace_id, "disk_space_exceeded", "file_size" => sb, "max_allowed" => max_allowed, "=>" => "reject");
                let _ = crate::bot::send_text_with_back(
                    api,
                    chat_id,
                    &tf("surge.error.too_large", &[("max", &fmt_bytes(max_allowed))]),
                )
                .await;
                return;
            }
        }
    }

    // ── چک ترافیک (روزانه + ماهانه) با احتساب حجم فایل جدید ──
    if let Some(db) = database.as_ref() {
        let client = db.client();
        let user_rank = rank::effective_rank(client, user_id).await;
        let daily_limit = user_rank.daily_traffic_bytes();
        let monthly_limit = user_rank.monthly_traffic_bytes();
        let first_upload_at = rank::quota::get_first_upload_at(client, user_id)
            .await
            .unwrap_or_else(now_epoch);
        let daily_used = rank::quota::get_daily_traffic(client, user_id)
            .await
            .unwrap_or(0) as u64;
        let monthly_used = rank::quota::get_monthly_traffic(client, user_id, first_upload_at)
            .await
            .unwrap_or(0) as u64;

        let file_sz = size_bytes.unwrap_or(0);
        let block = if daily_used + file_sz > daily_limit {
            Some((
                tf(
                    "youtube.traffic_daily_limit",
                    &[("limit", &fmt_traffic_fa(daily_limit))],
                ),
                user_rank.traffic_daily_next_rank(),
            ))
        } else if monthly_used + file_sz > monthly_limit {
            Some((
                tf(
                    "youtube.traffic_monthly_limit",
                    &[("limit", &fmt_traffic_fa(monthly_limit))],
                ),
                user_rank.traffic_monthly_next_rank(),
            ))
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

    flow_manager.set(
        user_id,
        FlowState::AwaitingSurgeConfirm {
            url: url.to_string(),
            filename: filename.clone(),
        },
    );

    let size_label = size_bytes
        .map(fmt_bytes)
        .unwrap_or_else(|| t("surge.size_unknown"));
    let text = tf(
        "surge.confirm_prompt",
        &[("name", &filename), ("size", &size_label)],
    );
    let entities = entities_for_text(&text);
    let mut params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(confirm_keyboard()))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    let r = api.send_message(&params).await;
    log_ev!("surge_dl", trace_id, "confirm_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

// ── confirm / rename ──────────────────────────────────────────────────────────

async fn start_surge_job(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    url: String,
    rename_to: Option<String>,
    trace_id: u64,
) {
    let text = t("surge.queued");
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    if let Err(e) = api.edit_message_text(&params).await {
        log_ev!("surge_dl", trace_id, "queue_edit_failed", "=>" => format!("fail err={e}"));
    }
    let api2 = api.clone();
    tokio::spawn(async move {
        run_surge_download(api2, chat_id, message_id, user_id, url, rename_to, trace_id).await;
    });
}

pub async fn handle_surge_confirm_original(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("surge_dl", trace_id, user_id, "clicked" => CB_SURGE_CONFIRM_ORIGINAL);
    let FlowState::AwaitingSurgeConfirm { url, .. } = flow_manager.get(user_id) else {
        log_ev!("surge_dl", trace_id, "confirm_stale", "=>" => "ignored");
        return;
    };
    flow_manager.clear(user_id);
    start_surge_job(api, chat_id, message_id, user_id, url, None, trace_id).await;
}

pub async fn handle_surge_confirm_rename(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("surge_dl", trace_id, user_id, "clicked" => CB_SURGE_CONFIRM_RENAME);
    let FlowState::AwaitingSurgeConfirm { url, filename, .. } = flow_manager.get(user_id) else {
        log_ev!("surge_dl", trace_id, "confirm_stale", "=>" => "ignored");
        return;
    };
    flow_manager.set(
        user_id,
        FlowState::AwaitingSurgeRenameInput {
            url,
            original_filename: filename,
            prompt_message_id: message_id,
        },
    );
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(t("surge.rename_prompt"))
        .build();
    let r = api.edit_message_text(&params).await;
    log_ev!("surge_dl", trace_id, "rename_prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

pub async fn handle_surge_rename_text(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("surge_dl", trace_id, user_id, "clicked" => "send_surge_rename");

    let FlowState::AwaitingSurgeRenameInput {
        url,
        original_filename,
        prompt_message_id,
    } = flow_manager.get(user_id)
    else {
        return;
    };
    let Some(typed) = message
        .text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return;
    };

    // Reduce to a bare filename — drops any path separators or `..` so the rename
    // can't escape the per-user download dir (with_file_name runs as root).
    let Some(typed) = sanitize_rename(typed) else {
        log_ev!("surge_dl", trace_id, "rename_rejected", "=>" => "invalid_name");
        let _ = crate::bot::send_text_with_back(api, chat_id, &t("surge.error.invalid_name")).await;
        return;
    };
    let typed = typed.as_str();

    let new_name = if typed.contains('.') {
        typed.to_string()
    } else {
        match std::path::Path::new(&original_filename)
            .extension()
            .and_then(|e| e.to_str())
        {
            Some(ext) => format!("{typed}.{ext}"),
            None => typed.to_string(),
        }
    };
    flow_manager.clear(user_id);
    log_ev!("surge_dl", trace_id, "rename_accepted", "name" => &new_name);

    start_surge_job(
        api,
        chat_id,
        prompt_message_id,
        user_id,
        url,
        Some(new_name),
        trace_id,
    )
    .await;
}

// ── download orchestration ───────────────────────────────────────────────────

struct SurgeDetail {
    filename: String,
    url: String,
    total_size: u64,
    downloaded: u64,
    progress: f64,
    speed: f64,
    avg_speed: f64,
    status: String,
}

struct DirCleanupGuard(PathBuf);
impl Drop for DirCleanupGuard {
    fn drop(&mut self) {
        let path = self.0.clone();
        tokio::spawn(async move {
            let _ = tokio::fs::remove_dir_all(path).await;
        });
    }
}

async fn run_surge_download(
    api: Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    url: String,
    rename_to: Option<String>,
    trace_id: u64,
) {
    let stats_job_id = crate::stats::record_download_start(user_id).await;
    let _active_dl_guard = crate::metrics::ActiveDownloadGuard::new();
    let _duration_guard = crate::metrics::RequestDurationGuard::new("surge_dl");
    let job_nonce = rand::random::<u32>();
    let dir = format!("{}/{user_id}/job_{trace_id}_{job_nonce}", crate::config::surge_downloads_root());
    let dir_path = PathBuf::from(&dir);
    let _cleanup_guard = DirCleanupGuard(dir_path);

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
        send_single_file(&api, chat_id, &file_path).await
    } else {
        send_split_file(&api, chat_id, &file_path, user_id, trace_id).await
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
                crate::stats::record_upload_done(jid, user_id, detail.downloaded as i64).await;
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

async fn send_single_file(
    api: &Bot,
    chat_id: i64,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(path.to_path_buf())
        .build();
    api.send_document(&params).await?;
    Ok(())
}

async fn send_split_file(
    api: &Bot,
    chat_id: i64,
    path: &Path,
    user_id: i64,
    trace_id: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let archive_base = path.with_file_name(format!("{stem}.archive.rar"));
    log_ev!("surge_dl", trace_id, "rar_spawn", "archive" => archive_base.display());

    let cores = acquire_cpu(user_id, trace_id).await;

    let mut cmd = tokio::process::Command::new("rar");
    cmd.arg("a")
        .arg(format!("-v{}m", MAX_PART_BYTES / (1024 * 1024)))
        .arg("-m0")
        .arg(&archive_base)
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let status =
        tokio::time::timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS), child.wait()).await??;
    release_cpu(cores, trace_id).await;

    if !status.success() {
        return Err(format!("rar exit {status}").into());
    }

    let parts = list_rar_parts(&archive_base).await?;
    log_ev!("surge_dl", trace_id, "rar_done", "parts" => parts.len());

    let total = parts.len();
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
        api.send_document(&params).await?;
        log_ev!("surge_dl", trace_id, "part_sent", "n" => i + 1, "total" => total);
    }

    Ok(())
}

async fn list_rar_parts(
    archive_base: &Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    let dir = archive_base.parent().ok_or("no parent dir")?;
    let stem = archive_base
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("no stem")?;
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
    parts.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|s| {
                let part_str = s.rsplit(".part").next()?;
                part_str.split('.').next()?.parse::<u32>().ok()
            })
            .unwrap_or(0)
    });
    Ok(parts)
}

// ── link preview (name + size before committing to a download) ───────────────

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn filename_from_url(url: &str) -> String {
    let no_query = url.split('?').next().unwrap_or(url);
    let name = no_query
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    safe_download_filename(&percent_decode(name))
}

fn safe_download_filename(name: &str) -> String {
    let cleaned = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .take(255)
        .collect::<String>();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned.to_string()
    }
}

fn extract_content_disposition_filename(header: &str) -> Option<String> {
    for part in header.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("filename*=") {
            let val = v.trim_matches('"');
            if let Some(idx) = val.find("''") {
                let encoded = &val[idx + 2..];
                return Some(percent_decode(encoded));
            }
        }
    }
    for part in header.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("filename=") {
            return Some(v.trim_matches('"').to_string());
        }
    }
    None
}

/// حدس اسم + حجم فایل با یه درخواست HEAD قبل از شروع دانلود واقعی — surge CLI
/// حالت dry-run نداره (فقط `add` که واقعاً دانلود رو شروع می‌کنه)، پس این فقط
/// یه پیش‌نمایشه؛ اگه HEAD رد بشه یا هدر نداشته باشه، با بهترین حدس ادامه می‌دیم.
async fn probe_url(url: &str) -> (String, Option<u64>) {
    let fallback = filename_from_url(url);
    // بعضی سرورها (مثل thinkbroadband) بدون User-Agent با 403 و یه صفحه‌ی خطای
    // کوچیک جواب می‌دن — بدون این هدر و بدون چک status، حجمِ همون صفحه‌ی خطا رو
    // به اشتباه به عنوان حجم فایل برمی‌گردونیم.
    let resp = match reqwest::Client::new()
        .head(url)
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return (fallback, None),
    };
    let filename = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_content_disposition_filename)
        .map(|s| safe_download_filename(&percent_decode(&s)))
        .unwrap_or(fallback);
    let size = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    (filename, size)
}

fn build_bar(percent: f32) -> String {
    let total = 10usize;
    let filled = ((percent / 10.0).round() as i32).clamp(0, total as i32) as usize;
    let mut s = String::new();
    for _ in 0..filled {
        s.push('●');
    }
    for _ in 0..(total - filled) {
        s.push('○');
    }
    s
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
            (
                format!("{:.0}", g.round()),
                crate::i18n::t("youtube.unit_gb"),
            )
        } else {
            (format!("{:.1}", g), crate::i18n::t("youtube.unit_gb"))
        }
    } else {
        (
            format!("{:.0}", (b / MB).round()),
            crate::i18n::t("youtube.unit_mb"),
        )
    };
    format!("{} {}", crate::i18n::to_fa_digits(&num), unit)
}

fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.2}GB", mb / 1024.0)
    } else {
        format!("{mb:.1}MB")
    }
}

// surge already reports speed/avg_speed in MB/s (confirmed against a real
// download's total_size/time_taken) — no unit conversion needed here.
fn fmt_speed(mb_per_sec: f64) -> String {
    format!("{mb_per_sec:.2}MB/s")
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// حجم/زمان — چون تلگرام سرعت آپلود رو گزارش نمی‌ده (برخلاف دانلود که خودِ surge گزارش می‌ده).
fn fmt_speed_from(bytes: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64().max(0.001);
    let mb_per_sec = bytes as f64 / (1024.0 * 1024.0) / secs;
    fmt_speed(mb_per_sec)
}

async fn show_sent_menu(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    bytes: u64,
    download_elapsed: Duration,
    upload_elapsed: Duration,
) {
    let _ = api
        .delete_message(
            &frankenstein::methods::DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .build(),
        )
        .await;

    let is_admin = crate::config::admin_user_id()
        .map(|id| id == chat_id)
        .unwrap_or(false);
    // زمان‌ها رو خودمون اندازه می‌گیریم و سرعت رو از تقسیم حجم بر زمان حساب می‌کنیم —
    // فیلد avg_speed خودِ surge واحدش قابل‌اعتماد نبود (برای دانلود ۹ثانیه‌ای عددی
    // مثل ۱۶۱۸۵۳۹۰۶٫۷۳MB/s برمی‌گردوند).
    let text = tf(
        "surge.sent",
        &[
            ("download_time", &fmt_elapsed(download_elapsed)),
            ("download_speed", &fmt_speed_from(bytes, download_elapsed)),
            ("upload_time", &fmt_elapsed(upload_elapsed)),
            ("upload_speed", &fmt_speed_from(bytes, upload_elapsed)),
        ],
    );
    let entities = entities_for_text(&text);
    let mut params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
            crate::bot::start_menu_keyboard(is_admin),
        ))
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    let _ = api.send_message(&params).await;
}

async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: &str) {
    let entities = entities_for_text(text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .build();
    if !entities.is_empty() {
        params.entities = Some(entities);
    }
    let _ = api.edit_message_text(&params).await;
}

// ── surge CLI wrapper ────────────────────────────────────────────────────────

async fn run_surge_add(url: &str, dir: &str) -> bool {
    let mut cmd = surge_cmd(&["add", url, "-o", dir]);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) => status.success(),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            false
        }
    }
}

async fn list_surge_job_ids() -> Vec<String> {
    let output = surge_cmd(&["ls", "--json"]).output().await;
    let Ok(output) = output else {
        return vec![];
    };
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    entries
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("id")?.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

async fn find_job_id_by_url(url: &str, before_ids: &[String], trace_id: u64) -> Option<String> {
    for _ in 0..10 {
        let output = surge_cmd(&["ls", "--json"]).output().await;
        let Ok(output) = output else {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        };
        let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
        if let Some(arr) = entries.as_array() {
            let mut candidate_ids: Vec<String> = arr
                .iter()
                .rev()
                .filter_map(|e| e.get("id")?.as_str().map(str::to_string))
                .filter(|id| !before_ids.contains(id))
                .collect();

            if candidate_ids.is_empty() {
                candidate_ids = arr
                    .iter()
                    .rev()
                    .take(10)
                    .filter_map(|e| e.get("id")?.as_str().map(str::to_string))
                    .collect();
            }

            for id in candidate_ids {
                if let Some(detail) = fetch_detail(&id).await {
                    if detail.url == url {
                        return Some(id);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    log_ev!("surge_dl", trace_id, "find_job_id_timeout", "url" => url);
    None
}

async fn fetch_detail(id: &str) -> Option<SurgeDetail> {
    let output = surge_cmd(&["ls", id, "--json"]).output().await.ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    Some(SurgeDetail {
        filename: json.get("filename")?.as_str()?.to_string(),
        url: json.get("url")?.as_str()?.to_string(),
        total_size: json.get("total_size")?.as_u64()?,
        downloaded: json.get("downloaded")?.as_u64()?,
        progress: json.get("progress")?.as_f64()?,
        speed: json.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.0),
        avg_speed: json
            .get("avg_speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        status: json.get("status")?.as_str()?.to_string(),
    })
}

/// Reduces user rename input to a bare filename, rejecting anything that could
/// escape the download dir (path separators, `.`/`..`). Returns None if nothing
/// safe remains — the caller then aborts the rename.
fn sanitize_rename(typed: &str) -> Option<String> {
    std::path::Path::new(typed)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .map(|n| {
            let s: String = n.chars().take(200).collect();
            s
        })
}

use std::sync::OnceLock;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
    let res = client
        .post("http://127.0.0.1:6589/cpu/acquire")
        .form(&[
            ("user_id", user_id.to_string()),
            ("is_vip", "false".to_string()),
        ])
        .timeout(Duration::from_secs(120))
        .send()
        .await;
    match res {
        Ok(r) => {
            let json: serde_json::Value = r.json().await.unwrap_or_default();
            let cores: Vec<i32> = json
                .get("cores")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            log_ev!("surge_dl", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));
            cores
        }
        Err(e) => {
            log_ev!("surge_dl", trace_id, "cpu_acquire_failed", "=>" => format!("fail err={e}"));
            vec![]
        }
    }
}

async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    if cores.is_empty() {
        return;
    }
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
    let body = serde_json::json!({ "cores": cores });
    let r = client
        .post("http://127.0.0.1:6589/cpu/release")
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    log_ev!("surge_dl", trace_id, "cpu_released", "cores" => format!("{cores:?}"), "=>" => if r.is_ok() { "ok" } else { "fail" });
}

#[cfg(test)]
mod tests {
    use super::{
        available_disk_space, extract_content_disposition_filename, is_direct_link,
        safe_download_filename, sanitize_rename,
    };

    #[test]
    fn test_is_direct_link_ignores_telegram_urls() {
        assert!(!is_direct_link("https://t.me/c/3310766784/162"));
        assert!(!is_direct_link("https://user@t.me/c/3310766784/162"));
        assert!(!is_direct_link("https://telegram.org/blog"));
        assert!(!is_direct_link("http://telegram.me/user"));
        assert!(is_direct_link("https://example.com/file.zip"));
        assert!(is_direct_link("http://direct.download.com/video.mp4"));
    }

    #[test]
    fn direct_link_rejects_shell_like_and_private_targets() {
        assert!(!is_direct_link("https://test123 ; sleep 10"));
        assert!(!is_direct_link("http://$(sleep 10)"));
        assert!(!is_direct_link("http://127.0.0.1/private"));
        assert!(!is_direct_link("http://169.254.169.254/latest/meta-data"));
    }

    #[test]
    fn download_filename_drops_shell_metacharacters_and_paths() {
        assert_eq!(safe_download_filename("sleep 15; .pdf"), "sleep 15 .pdf");
        assert_eq!(safe_download_filename("../../etc/passwd"), "etcpasswd");
        assert_eq!(safe_download_filename("$()`|&"), "file");
    }

    #[test]
    fn keeps_plain_names() {
        assert_eq!(sanitize_rename("movie.mp4").as_deref(), Some("movie.mp4"));
        assert_eq!(sanitize_rename("my file").as_deref(), Some("my file"));
    }

    #[test]
    fn test_sanitize_rename_multibyte_utf8() {
        let long_farsi = "نام_فایل_بسیار_طولانی_برای_تست_سیستم_دانلود_که_نباید_در_زمان_برش_بایتی_باعث_پنیک_در_رست_شود_چون_کاراکترهای_فارسی_چندبایتی_هستند_و_برش_روی_مرز_بایت_نامعتبر_موجب_کرش_پروسه_میگردد.mp4";
        let sanitized = sanitize_rename(long_farsi);
        assert!(sanitized.is_some());
        let res = sanitized.unwrap();
        assert!(res.chars().count() <= 200);
    }

    #[test]
    fn test_extract_content_disposition_filename() {
        assert_eq!(
            extract_content_disposition_filename("attachment; filename=\"test.mp4\""),
            Some("test.mp4".to_string())
        );
        assert_eq!(
            extract_content_disposition_filename("attachment; filename*=UTF-8''%D9%81%D8%A7%DB%8C%D9%84.mp4"),
            Some("فایل.mp4".to_string())
        );
    }

    #[test]
    fn test_available_disk_space() {
        let space = available_disk_space("/tmp");
        assert!(space.is_ok());
        assert!(space.unwrap() > 0);
    }

    #[test]
    fn strips_traversal() {
        // Path separators and parent refs are stripped to the trailing component…
        assert_eq!(
            sanitize_rename("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(sanitize_rename("/etc/cron.d/x").as_deref(), Some("x"));
        // …and inputs that reduce to nothing safe are rejected outright.
        assert_eq!(sanitize_rename(".."), None);
        assert_eq!(sanitize_rename("../.."), None);
        assert_eq!(sanitize_rename("/"), None);
    }
}
