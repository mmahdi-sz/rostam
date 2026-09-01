use frankenstein::{
    AsyncTelegramApi, methods::AnswerCallbackQueryParams, types::MaybeInaccessibleMessage,
    updates::UpdateContent,
};

use crate::bot::{CB_LANG_SET, send_lang_picker, send_start_menu};
use crate::config;
use crate::i18n::{LANG, t};
use crate::stats;
use crate::youtube::trace::log_trace;

use super::state::AppState;

mod callback;
mod flow;
mod text;

pub async fn handle_update(
    state: &mut AppState,
    content: UpdateContent,
) -> crate::error::Result<()> {
    // DEV_MODE: Admin-only access
    if config::dev_mode() {
        let admin = config::admin_user_id();
        let sender = match &content {
            UpdateContent::Message(m) => m.from.as_ref().filter(|u| !u.is_bot).map(|u| u.id as i64),
            UpdateContent::CallbackQuery(c) => {
                if c.from.is_bot {
                    None
                } else {
                    Some(c.from.id as i64)
                }
            }
            _ => None,
        };
        if sender.is_some() && sender != admin {
            eprintln!("[dev_mode] blocked user_id={sender:?}");
            return Ok(());
        }
    }

    // chat_member membership locks update — keep Redis cache fresh
    if let UpdateContent::ChatMember(cm) = &content {
        crate::force_join::on_chat_member_update(&cm.chat, &cm.new_chat_member).await;
        return Ok(());
    }

    let sender = match &content {
        UpdateContent::Message(m) => {
            // Skip bot messages
            if m.from.as_ref().map_or(false, |u| u.is_bot) {
                return Ok(());
            }
            // Skip Telegram service messages
            if m.pinned_message.is_some()
                || m.new_chat_members.is_some()
                || m.left_chat_member.is_some()
                || m.new_chat_title.is_some()
                || m.new_chat_photo.is_some()
                || m.delete_chat_photo.is_some()
                || m.group_chat_created.is_some()
                || m.supergroup_chat_created.is_some()
                || m.channel_chat_created.is_some()
                || m.message_auto_delete_timer_changed.is_some()
                || m.migrate_to_chat_id.is_some()
                || m.migrate_from_chat_id.is_some()
            {
                return Ok(());
            }
            if let Some(text) = &m.text {
                if text.len() > 4096 {
                    eprintln!(
                        "[security] dropped oversized message from user_id={:?}",
                        m.from.as_ref().map(|u| u.id)
                    );
                    return Ok(());
                }
            }
            m.from.as_ref().map(|u| u.id as i64)
        }
        UpdateContent::CallbackQuery(c) => {
            if c.from.is_bot {
                return Ok(());
            }
            Some(c.from.id as i64)
        }
        _ => None,
    };

    if let Some(uid) = sender {
        let now = std::time::Instant::now();
        if let Some(last) = state.user_last_update.get(&uid) {
            if now.duration_since(*last) < std::time::Duration::from_millis(500) {
                eprintln!("[rate_limit] dropped update from user_id={uid}");
                return Ok(());
            }
        }
        state.user_last_update.insert(uid, now);
        if state.user_last_update.len() > 50_000 {
            state
                .user_last_update
                .retain(|_, v| now.duration_since(*v) < std::time::Duration::from_secs(3600));
        }
    }

    // Referral attribution must run before the language + force-join gates:
    // a brand-new user gets the language picker and an early return, so the
    // `?start=<referrer_id>` payload would be lost forever.
    if let (Some(uid), Some(db)) = (sender, state.database.as_ref()) {
        if let UpdateContent::Message(m) = &content {
            if let Some(referrer_id) = m
                .text
                .as_deref()
                .and_then(|t| t.strip_prefix("/start"))
                .and_then(|rest| rest.trim().parse::<i64>().ok())
            {
                if referrer_id != uid && !stats::user_seen(uid).await {
                    let trace_id = crate::log::next_trace_id();
                    if let Ok(client) = db.get().await {
                        crate::referral::record_referral(&*client, uid, referrer_id).await;
                    }
                    log_trace(
                        trace_id,
                        "referral_attributed",
                        &format!("referred_id={uid} referrer_id={referrer_id}"),
                    );
                }
            }
        }
    }

    // ── language gate ────────────────────────────────────────────────────────
    // callback "lang:set:xx" -> save language, ack, proceed
    // without language -> send language picker and return
    if let Some(uid) = sender {
        let cb_data = if let UpdateContent::CallbackQuery(cq) = &content {
            cq.data.as_deref()
        } else {
            None
        };
        let is_check_btn = cb_data == Some(crate::force_join::CB_FJ_CHECK);

        if let Some(lang) = cb_data.and_then(|d| d.strip_prefix(CB_LANG_SET)) {
            // ack callback
            if let UpdateContent::CallbackQuery(cq) = &content {
                let _ = state
                    .api
                    .answer_callback_query(
                        &AnswerCallbackQueryParams::builder()
                            .callback_query_id(cq.id.clone())
                            .build(),
                    )
                    .await;
            }
            stats::set_user_language(uid, lang).await;
            eprintln!("[dispatch event=lang_set] user_id={uid} lang={lang}");
            // Show start menu after setting language
            let chat_id = match &content {
                UpdateContent::CallbackQuery(cq) => cq
                    .message
                    .as_ref()
                    .and_then(|m| match m {
                        MaybeInaccessibleMessage::Message(msg) => Some(msg.chat.id),
                        _ => None,
                    })
                    .unwrap_or(uid),
                _ => uid,
            };
            let lang_owned = lang.to_owned();
            LANG.scope(lang_owned, async {
                send_start_menu(&state.api, chat_id).await
            })
            .await?;
            return Ok(());
        }

        // Language check only if DB exists
        if state.database.is_some() {
            // redeem deep-link: bypass gate to activate code first, then select language
            let is_redeem = if let UpdateContent::Message(m) = &content {
                m.text
                    .as_deref()
                    .and_then(|t| t.strip_prefix("/start"))
                    .map(|r| r.trim().starts_with("redeem"))
                    .unwrap_or(false)
            } else {
                false
            };

            let lang_opt = stats::get_user_language(uid).await;
            if lang_opt.is_none() && !is_redeem {
                let chat_id = match &content {
                    UpdateContent::Message(m) => m.chat.id,
                    UpdateContent::CallbackQuery(cq) => cq
                        .message
                        .as_ref()
                        .and_then(|m| match m {
                            MaybeInaccessibleMessage::Message(msg) => Some(msg.chat.id),
                            _ => None,
                        })
                        .unwrap_or(uid),
                    _ => uid,
                };
                send_lang_picker(&state.api, chat_id).await?;
                return Ok(());
            }
            let lang = lang_opt.unwrap_or_else(|| "fa".to_string());
            return LANG
                .scope(lang, async {
                    if !is_redeem && !gate_force_join(state, &content, uid, is_check_btn).await? {
                        return Ok(());
                    }
                    match content {
                        UpdateContent::Message(message) => {
                            text::handle_message(state, *message).await?
                        }
                        UpdateContent::CallbackQuery(callback_query) => {
                            callback::handle_callback(state, *callback_query).await?
                        }
                        _ => {}
                    }
                    Ok(())
                })
                .await;
        }

        // Without DB: no redeem concept (requires DB) -> always gate
        if !gate_force_join(state, &content, uid, is_check_btn).await? {
            return Ok(());
        }
    }

    // Without DB (or update without sender): direct dispatch with default lang
    match content {
        UpdateContent::Message(message) => text::handle_message(state, *message).await?,
        UpdateContent::CallbackQuery(callback_query) => {
            callback::handle_callback(state, *callback_query).await?
        }
        _ => {}
    }
    Ok(())
}

/// Force-join gate — runs after language and activation code check.
/// `is_check_btn` means user clicked "Joined"; bypasses cache (live check)
/// and answers via toast/alert on the callback query.
/// Returns `Ok(true)` to proceed, `Ok(false)` to return early.
async fn gate_force_join(
    state: &AppState,
    content: &UpdateContent,
    uid: i64,
    is_check_btn: bool,
) -> crate::error::Result<bool> {
    let joined = if is_check_btn {
        crate::force_join::is_joined_live(&state.api, uid).await
    } else {
        crate::force_join::is_joined(&state.api, uid).await
    };

    let chat_id = match content {
        UpdateContent::Message(m) => m.chat.id,
        UpdateContent::CallbackQuery(cq) => cq
            .message
            .as_ref()
            .and_then(|m| match m {
                MaybeInaccessibleMessage::Message(msg) => Some(msg.chat.id),
                _ => None,
            })
            .unwrap_or(uid),
        _ => uid,
    };

    if !joined {
        if let UpdateContent::CallbackQuery(cq) = content {
            let params = if is_check_btn {
                AnswerCallbackQueryParams::builder()
                    .callback_query_id(cq.id.clone())
                    .text(t("force_join.still_not_joined"))
                    .show_alert(true)
                    .build()
            } else {
                AnswerCallbackQueryParams::builder()
                    .callback_query_id(cq.id.clone())
                    .build()
            };
            let _ = state.api.answer_callback_query(&params).await;
        }
        if !is_check_btn {
            crate::force_join::send_lock_message(&state.api, chat_id).await;
        }
        return Ok(false);
    }

    // Joined → confirm any pending referral right away (no-op when there is
    // none: single PK-indexed statement).
    if let Some(db) = state.database.as_ref() {
        if let Ok(client) = db.get().await {
            if crate::referral::confirm_on_join(&*client, uid).await {
                log_trace(
                    crate::log::next_trace_id(),
                    "referral_confirmed",
                    &format!("referred_id={uid}"),
                );
            }
        }
    }

    if is_check_btn {
        if let UpdateContent::CallbackQuery(cq) = content {
            let _ = state
                .api
                .answer_callback_query(
                    &AnswerCallbackQueryParams::builder()
                        .callback_query_id(cq.id.clone())
                        .text(t("force_join.now_joined"))
                        .build(),
                )
                .await;
        }
        send_start_menu(&state.api, chat_id).await?;
        return Ok(false);
    }

    Ok(true)
}
