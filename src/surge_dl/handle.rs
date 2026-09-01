use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{Message, ReplyMarkup},
};

use crate::bot::edit_to_tools;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{entities_for_text, t, tf};
use crate::log::next_trace_id;
use crate::rank;
use crate::surge_dl::engine::run_surge_download;
use crate::surge_dl::probe::{
    available_disk_space, detect_social_platform, is_direct_link, probe_url, sanitize_rename,
};
use crate::surge_dl::types::{CB_SURGE_CONFIRM_ORIGINAL, CB_SURGE_CONFIRM_RENAME, CB_TOOLS_SURGE};
use crate::surge_dl::ui::{cancel_keyboard, confirm_keyboard, fmt_bytes, fmt_traffic_fa};

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

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
            let text = tf(
                "surge.unsupported_platform",
                &[("platform", &platform_name)],
            );
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

    // ── Check free disk space (with 20% buffer) ──
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

    // ── Check traffic quotas (daily + monthly) including new file size ──
    if let Some(db) = database.as_ref() {
        let block_res = {
            if let Ok(client) = db.get().await {
                let user_rank = rank::effective_rank(&client, user_id).await;
                let daily_limit = user_rank.daily_traffic_bytes();
                let monthly_limit = user_rank.monthly_traffic_bytes();
                let first_upload_at = rank::quota::get_first_upload_at(&client, user_id)
                    .await
                    .unwrap_or_else(now_epoch);
                let daily_used = rank::quota::get_daily_traffic(&client, user_id)
                    .await
                    .unwrap_or(0) as u64;
                let monthly_used =
                    rank::quota::get_monthly_traffic(&client, user_id, first_upload_at)
                        .await
                        .unwrap_or(0) as u64;

                let file_sz = size_bytes.unwrap_or(0);
                if daily_used + file_sz > daily_limit {
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
                }
            } else {
                None
            }
        };

        if let Some((label, next_rank)) = block_res {
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
    crate::app::spawn_user_task(async move {
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
