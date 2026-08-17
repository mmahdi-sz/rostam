use frankenstein::{
    AsyncTelegramApi, methods::AnswerCallbackQueryParams, types::MaybeInaccessibleMessage,
    updates::UpdateContent,
};

use crate::bot::{
    CB_ADMIN_BROADCAST, CB_ADMIN_FORCE_JOIN, CB_ADMIN_GEN_CODE, CB_ADMIN_PANEL, CB_ADMIN_SECTION,
    CB_ADMIN_STATS, CB_AI_DENOISE, CB_AI_DEOLDIFY, CB_AI_GWM, CB_AI_NOBG, CB_AI_SEP, CB_AI_STT,
    CB_AI_TTS, CB_AI_UPSCALE, CB_BROADCAST_MODE_COPY, CB_BROADCAST_MODE_FORWARD,
    CB_BROADCAST_SEND_ACTIVE, CB_BROADCAST_SEND_ALL, CB_BROADCAST_TOGGLE_PIN, CB_DENOISE_CANCEL,
    CB_DEOLDIFY_CANCEL, CB_LANG_SET, CB_NOBG_CANCEL, CB_START_AI_LAB, CB_START_GUIDE,
    CB_START_GUIDE_PLATFORM, CB_START_LEADERBOARD, CB_START_STUDIO, CB_START_TOOLS, CB_TTS_CANCEL,
    CB_USER_PANEL,
};

use crate::bot::{
    edit_to_ai_lab, edit_to_leaderboard, edit_to_start_menu, edit_to_tools, send_lang_picker,
    send_start_menu,
};
use crate::config;
use crate::denoise;
use crate::emoji::{BroadcastMode, FlowState, handler as emoji_handler, panel::CB_START_PANEL};
use crate::feynobg::{enter_nobg, handle_nobg_cancel};
use crate::gemini_watermark::{CB_GWM_CANCEL, enter_gwm, handle_gwm_cancel, handle_gwm_image};
use crate::i18n::{LANG, reload_i18n, t, tf};
use crate::ip_lookup::{
    CB_IP_LOOKUP_CANCEL, CB_TOOLS_IP_LOOKUP, detect_ip, enter_ip_lookup, handle_ip_command,
    handle_ip_lookup_auto, handle_ip_lookup_cancel, handle_ip_lookup_text,
};
use crate::log::next_trace_id;
use crate::pdfcompress::{
    CB_PDF_CANCEL, CB_PDF_LEVEL_PREFIX, CB_PDF_MODE_ADVANCED, CB_PDF_MODE_SIMPLE,
    CB_TOOLS_PDF_COMPRESS, enter_pdf_compress, handle_pdf_cancel, handle_pdf_file,
    handle_pdf_level, handle_pdf_mode_simple,
};
use crate::separation::{
    CB_SEP_PREFIX, enter_separation, handle_direct_separation, handle_separation_audio,
    handle_separation_callback,
};
use crate::stats;
use crate::stt::handle::{enter_stt_config, handle_stt_audio, handle_stt_callback};
use crate::surge_dl::{
    CB_SURGE_CANCEL, CB_SURGE_CONFIRM_ORIGINAL, CB_SURGE_CONFIRM_RENAME, CB_TOOLS_SURGE,
    enter_surge_dl, handle_surge_cancel, handle_surge_confirm_original,
    handle_surge_confirm_rename, handle_surge_rename_text, handle_surge_text,
};
use crate::upscale::{
    CB_UPSCALE_ANIME_TOGGLE, CB_UPSCALE_CANCEL, CB_UPSCALE_MODEL_PREFIX, enter_upscale,
    handle_upscale_anime_toggle, handle_upscale_cancel, handle_upscale_image,
    handle_upscale_model_pick,
};
use crate::youtube::trace::log_trace;
use crate::youtube::{extract_youtube_urls, handle_youtube_url};
use frankenstein::types::InlineKeyboardMarkup;

use super::state::AppState;

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
        let mut cache = state.user_last_update.lock().await;
        let now = std::time::Instant::now();
        if let Some(&last) = cache.get(&uid) {
            if now.duration_since(last) < std::time::Duration::from_millis(500) {
                eprintln!("[rate_limit] dropped update from user_id={uid}");
                return Ok(());
            }
        }
        cache.insert(uid, now);
        if cache.len() > 50_000 {
            cache.clear();
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
                    crate::referral::record_referral(db.client(), uid, referrer_id).await;
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
                        UpdateContent::Message(message) => handle_message(state, *message).await?,
                        UpdateContent::CallbackQuery(callback_query) => {
                            handle_callback(state, *callback_query).await?
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
        UpdateContent::Message(message) => handle_message(state, *message).await?,
        UpdateContent::CallbackQuery(callback_query) => {
            handle_callback(state, *callback_query).await?
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
        if crate::referral::confirm_on_join(db.client(), uid).await {
            log_trace(
                crate::log::next_trace_id(),
                "referral_confirmed",
                &format!("referred_id={uid}"),
            );
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

async fn handle_message(
    state: &mut AppState,
    message: frankenstein::types::Message,
) -> crate::error::Result<()> {
    if message.from.as_ref().map_or(false, |u| u.is_bot) || message.pinned_message.is_some() {
        return Ok(());
    }
    let AppState {
        api,
        cookie_pool,
        database,
        flow_manager,
        rate_limit_tx,
        ..
    } = state;
    let user_id = message.from.as_ref().map(|u| u.id as i64);
    let msg_text = message.text.as_deref().unwrap_or("");
    if let Some(u) = message.from.as_ref() {
        let trace_id = crate::log::next_trace_id();
        log_actor!("dispatch", trace_id, u, "msg" => msg_text.chars().take(40).collect::<String>());
    }

    // Register user in stats. (Referral attribution already ran before the
    // language gate — a brand-new user never reaches this point.)
    if let Some(uid) = user_id {
        let username = message.from.as_ref().and_then(|u| u.username.as_deref());
        stats::record_user_global(uid, username).await;
    }

    // Step 1: addemoji link detection
    if let Some(uid) = user_id {
        if let Some(text) = message.text.as_deref() {
            if !text.trim_start().starts_with('/') {
                if let Some(pack_name) = emoji_handler::extract_addemoji_pack_name(text) {
                    emoji_handler::handle_addemoji_link(
                        api,
                        &message,
                        uid,
                        &pack_name,
                        flow_manager,
                        database,
                    )
                    .await;
                    return Ok(());
                }
            }
        }
    }

    // Step 2: /start always clears flow (+ deep-link payload: redeem<CODE>)
    if let (Some(uid), Some(text)) = (user_id, message.text.as_deref()) {
        if let Some(rest) = text.strip_prefix("/start") {
            flow_manager.clear(uid);
            let payload = rest.trim();
            if let Some(code) = payload.strip_prefix("redeem") {
                let first_name = message
                    .from
                    .as_ref()
                    .map(|u| u.first_name.as_str())
                    .unwrap_or("");
                let username = message.from.as_ref().and_then(|u| u.username.as_deref());
                crate::redeem::handle::handle_redeem(
                    api,
                    message.chat.id,
                    uid,
                    first_name,
                    username,
                    code,
                    database,
                )
                .await;
            } else {
                // Referral payload recorded earlier, before the language gate.
                send_start_menu(api, message.chat.id).await?;
            }
            return Ok(());
        }
    }

    // Step 3: "Cancel operation" reply keyboard when Idle
    if let (Some(uid), Some(text)) = (user_id, message.text.as_deref()) {
        if text.contains(&crate::i18n::t("emoji.cancel_button"))
            && matches!(flow_manager.get(uid), FlowState::Idle)
        {
            send_start_menu(api, message.chat.id).await?;
            return Ok(());
        }
    }

    // Step 4: active flow dispatch
    if let Some(uid) = user_id {
        if !matches!(flow_manager.get(uid), FlowState::Idle) {
            // Admin sent code generation args (e.g. `30d es 1u`)
            if matches!(flow_manager.get(uid), FlowState::AwaitingRedeemGenArgs) {
                if let Some(text) = message.text.as_deref() {
                    flow_manager.clear(uid);
                    let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
                    if is_admin {
                        crate::redeem::handle::handle_generate(
                            api,
                            message.chat.id,
                            uid,
                            text,
                            database,
                        )
                        .await;
                    }
                    return Ok(());
                }
            }

            // Admin sent new lock link
            if matches!(flow_manager.get(uid), FlowState::AwaitingForceJoinLink) {
                if let Some(text) = message.text.as_deref() {
                    let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
                    if is_admin {
                        crate::force_join::handle_link_message(
                            api,
                            message.chat.id,
                            text,
                            flow_manager,
                            uid,
                        )
                        .await;
                    }
                    return Ok(());
                }
            }

            // Admin sent username/numeric ID/forward for private link
            if let FlowState::AwaitingForceJoinPrivateInfo { link } = flow_manager.get(uid) {
                let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
                if is_admin {
                    crate::force_join::handle_private_info_message(
                        api,
                        message.chat.id,
                        &link,
                        &message,
                        flow_manager,
                        uid,
                    )
                    .await;
                }
                return Ok(());
            }

            // Admin sent input for lock field wizard (name/time/member/reserve)
            if let FlowState::AwaitingForceJoinField { lock_id, field, .. } = flow_manager.get(uid)
            {
                if let Some(text) = message.text.as_deref() {
                    let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
                    if is_admin {
                        crate::force_join::handle_field_message(
                            api,
                            message.chat.id,
                            lock_id,
                            &field,
                            text,
                            flow_manager,
                            uid,
                            database,
                        )
                        .await;
                    }
                    return Ok(());
                }
            }

            // Admin sent broadcast banner (any msg/media/text)
            if let FlowState::AwaitingBroadcastBanner { mode, pin } = flow_manager.get(uid) {
                let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
                if is_admin {
                    let banner_chat_id = message.chat.id;
                    let banner_message_id = message.message_id;

                    let (total_users, active_users) = if let Some(db) = database {
                        match crate::stats::get_broadcast_user_counts(db.client()).await {
                            Ok(counts) => (counts.total, counts.active),
                            Err(_) => (1, 1),
                        }
                    } else {
                        (1, 1)
                    };

                    flow_manager.set(
                        uid,
                        FlowState::AwaitingBroadcastTarget {
                            mode,
                            pin,
                            banner_chat_id,
                            banner_message_id,
                            total_users,
                            active_users,
                        },
                    );

                    let prompt_text = crate::i18n::tf(
                        "admin.broadcast.target_prompt",
                        &[
                            ("total", &total_users.to_string()),
                            ("active", &active_users.to_string()),
                        ],
                    );
                    let kb = crate::admin::broadcast::broadcast_target_keyboard(
                        active_users,
                        total_users,
                    );

                    let _ =
                        crate::bot::send_text_with_kb(api, message.chat.id, &prompt_text, kb).await;
                }
                return Ok(());
            }

            // Admin sent broadcast limit count (e.g. 500)
            if let FlowState::AwaitingBroadcastTarget {
                mode,
                pin,
                banner_chat_id,
                banner_message_id,
                ..
            } = flow_manager.get(uid)
            {
                if let Some(text) = message.text.as_deref() {
                    let is_admin = config::admin_user_id().map(|id| id == uid).unwrap_or(false);
                    if is_admin {
                        if let Ok(limit_num) = text.trim().parse::<i64>() {
                            flow_manager.clear(uid);
                            let db_client = database.as_ref().map(|db| db.client_arc());
                            crate::admin::broadcast::spawn_broadcast_job(
                                api.clone(),
                                db_client,
                                message.chat.id,
                                mode,
                                pin,
                                banner_chat_id,
                                banner_message_id,
                                false,
                                Some(limit_num),
                            );
                        }
                    }
                    return Ok(());
                }
            }

            if emoji_handler::handle_emoji_flow_message(api, &message, uid, flow_manager, database)
                .await
            {
                return Ok(());
            }

            if let FlowState::AwaitingSttAudio { config } = flow_manager.get(uid) {
                if message.voice.is_some() || message.audio.is_some() || message.document.is_some()
                {
                    let file_id = message
                        .voice
                        .as_ref()
                        .map(|v| v.file_id.clone())
                        .or_else(|| message.audio.as_ref().map(|a| a.file_id.clone()))
                        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));
                    if let Some(fid) = file_id {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "stt_route_dispatched",
                            &format!("user_id={uid} chat_id={}", message.chat.id),
                        );
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        let db2 = database.clone();
                        // Flow deliberately kept: user can send next voice with same model after result
                        // Spawn so the event loop stays free during STT (minutes-long operation)
                        super::spawn_user_task(async move {
                            handle_stt_audio(&api2, chat_id2, &fid, uid, &config, db2).await;
                        });
                    }
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingDenoiseAudio) {
                if message.voice.is_some()
                    || message.audio.is_some()
                    || message.video.is_some()
                    || message.document.is_some()
                {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "denoise_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    // Spawn so the event loop stays free during denoise (minutes-long operation).
                    flow_manager.clear(uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let db2 = database.clone();
                    super::spawn_user_task(async move {
                        denoise::handle_denoise_audio(&api2, &msg2, uid, &db2).await;
                    });
                    return Ok(());
                }
            }

            if let FlowState::AwaitingUpscaleImage {
                scale_factor,
                model_name,
                ..
            } = flow_manager.get(uid)
            {
                if message.photo.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "upscale_route_dispatched",
                        &format!("user_id={uid} model={model_name}"),
                    );
                    flow_manager.clear(uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let db2 = database.clone();
                    super::spawn_user_task(async move {
                        handle_upscale_image(api2, msg2, uid, scale_factor, model_name, db2).await;
                    });
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingSeparation) {
                if message.audio.is_some()
                    || message.voice.is_some()
                    || message.document.is_some()
                    || message.video.is_some()
                {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "separation_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    handle_separation_audio(api, &message, uid, flow_manager).await;
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingGeminiWmImage) {
                if message.photo.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "gwm_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    // Spawn so the event loop stays free during watermark removal.
                    flow_manager.clear(uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    super::spawn_user_task(async move {
                        handle_gwm_image(&api2, &msg2, uid).await;
                    });
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingDeoldifyImage) {
                if message.photo.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "deoldify_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    flow_manager.clear(uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let fm_clone = flow_manager.clone();
                    let db_clone = database.clone();
                    super::spawn_user_task(async move {
                        crate::deoldify::handle_deoldify_image(
                            &api2, &msg2, uid, &fm_clone, db_clone,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingNobgImage) {
                if message.photo.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "nobg_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let fm = flow_manager.clone();
                    let db = database.clone();
                    super::spawn_user_task(async move {
                        crate::feynobg::handle_nobg_image(&api2, &msg2, uid, &fm, db).await;
                    });
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingTtsText) {
                if let Some(text) = &message.text {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "tts_text_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    let text_clone = text.clone();
                    let api2 = api.clone();
                    let chat_id = message.chat.id;
                    let flow_manager_clone = flow_manager.clone();
                    let database_clone = database.clone();
                    flow_manager.clear(uid);
                    super::spawn_user_task(async move {
                        crate::moss_tts::handle_tts_text(
                            &api2,
                            chat_id,
                            uid,
                            &text_clone,
                            &flow_manager_clone,
                            database_clone,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingPdfCompressFile) {
                if message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "pdfcompress_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    handle_pdf_file(api, &message, uid, flow_manager).await;
                } else {
                    let _ =
                        crate::bot::send_text(api, message.chat.id, &t("pdfcompress.busy_warning"))
                            .await;
                }
                return Ok(());
            }

            if matches!(
                flow_manager.get(uid),
                FlowState::AwaitingPdfCompressLevel { .. }
            ) {
                let _ = crate::bot::send_text(api, message.chat.id, &t("pdfcompress.busy_warning"))
                    .await;
                return Ok(());
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingPkgFile) {
                if message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_ev!("pkgconvert", trace_id, "file_dispatched", "user_id" => uid);
                    crate::pkgconvert::handle_pkg_file(api, &message, uid, flow_manager, database)
                        .await;
                } else {
                    let _ = crate::bot::send_text_md(api, message.chat.id, &t("pkg.prompt")).await;
                }
                return Ok(());
            }

            if matches!(
                flow_manager.get(uid),
                FlowState::AwaitingPkgConvertChoice { .. }
            ) {
                return Ok(());
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingIpLookupInput) {
                if message.text.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "ip_lookup_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    handle_ip_lookup_text(api, &message, uid, flow_manager).await;
                }
                return Ok(());
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingSurgeUrlInput) {
                if let Some(txt) = message.text.as_deref() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "surge_dl_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    flow_manager.clear(uid);
                    if crate::musicset::try_route_set(api, message.chat.id, uid, database, txt) {
                        return Ok(());
                    }
                    if let Some(track_id) = crate::spotify::extract_spotify_track_id(txt) {
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        let msg_id2 = message.message_id;
                        let db2 = database.clone();
                        super::spawn_user_task(async move {
                            if let Err(e) = crate::spotify::handle_spotify_url(
                                &api2, chat_id2, msg_id2, uid, trace_id, &track_id, &db2,
                            )
                            .await
                            {
                                crate::stats::record_error_global("spotify", e).await;
                            }
                        });
                        return Ok(());
                    }

                    if let Some(sc_url) = crate::soundcloud::extract_soundcloud_url(txt) {
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        let msg_id2 = message.message_id;
                        let db2 = database.clone();
                        super::spawn_user_task(async move {
                            if let Err(e) = crate::soundcloud::handle_soundcloud_url(
                                &api2, chat_id2, msg_id2, uid, trace_id, &sc_url, &db2,
                            )
                            .await
                            {
                                crate::stats::record_error_global("soundcloud", e).await;
                            }
                        });
                        return Ok(());
                    }

                    let platform = crate::surge_dl::detect_social_platform(txt);
                    if platform == Some("youtube") {
                        let urls = extract_youtube_urls(txt);
                        let target_url = if !urls.is_empty() {
                            urls[0].to_string()
                        } else {
                            txt.to_string()
                        };
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        let msg_id2 = message.message_id;
                        let pool2 = cookie_pool.clone();
                        let db2 = database.clone();
                        let rl_tx2 = rate_limit_tx.clone();
                        super::spawn_user_task(async move {
                            if let Err(e) = handle_youtube_url(
                                &api2,
                                chat_id2,
                                msg_id2,
                                user_id,
                                trace_id,
                                &target_url,
                                pool2,
                                &db2,
                                &rl_tx2,
                            )
                            .await
                            {
                                crate::stats::record_error_global("youtube", e).await;
                            }
                        });
                        return Ok(());
                    } else if platform == Some("spotify") {
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        super::spawn_user_task(async move {
                            let _ = crate::bot::send_text(
                                &api2,
                                chat_id2,
                                &t("spotify.only_single_tracks"),
                            )
                            .await;
                        });
                        return Ok(());
                    } else if platform == Some("soundcloud") {
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        super::spawn_user_task(async move {
                            let _ = crate::bot::send_text(
                                &api2,
                                chat_id2,
                                &t("soundcloud.only_single_tracks"),
                            )
                            .await;
                        });
                        return Ok(());
                    } else if let Some(p) = platform {
                        let platform_name = t(&format!("platforms.{p}"));
                        let text = tf(
                            "surge.unsupported_platform",
                            &[("platform", &platform_name)],
                        );
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        super::spawn_user_task(async move {
                            let _ = crate::bot::send_text(&api2, chat_id2, &text).await;
                            let _ = crate::bot::send_tools_menu(&api2, chat_id2).await;
                        });
                        return Ok(());
                    }

                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let fm2 = flow_manager.clone();
                    let db2 = database.clone();
                    super::spawn_user_task(async move {
                        handle_surge_text(&api2, &msg2, uid, &fm2, &db2).await;
                    });
                }
                return Ok(());
            }

            if matches!(
                flow_manager.get(uid),
                FlowState::AwaitingSurgeRenameInput { .. }
            ) {
                if message.text.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "surge_dl_rename_route_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    handle_surge_rename_text(api, &message, uid, flow_manager).await;
                }
                return Ok(());
            }

            if let FlowState::AwaitingCompressPassword { config } = flow_manager.get(uid) {
                if message.text.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "filecompress_password_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    crate::filecompress::handle_fc_password_text(
                        api,
                        &message,
                        uid,
                        flow_manager,
                        config,
                    )
                    .await;
                    return Ok(());
                }
                // Non-text input during password step: prompt text required
                let trace_id = next_trace_id();
                log_trace(
                    trace_id,
                    "filecompress_password_non_text",
                    &format!("user_id={uid} chat_id={}", message.chat.id),
                );
                crate::filecompress::send_password_need_text(api, message.chat.id).await;
                return Ok(());
            }

            if matches!(
                flow_manager.get(uid),
                FlowState::AwaitingCompressFiles { .. } | FlowState::AwaitingCompressOptions { .. }
            ) {
                if let Some(text) = &message.text {
                    let trimmed = text.trim();
                    if trimmed == t("fc.done_upload_button")
                        || trimmed == "اتمام اپلود"
                        || trimmed == "اتمام آپلود"
                        || trimmed.contains("اتمام")
                    {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "filecompress_done_dispatched",
                            &format!("user_id={uid} chat_id={}", message.chat.id),
                        );
                        crate::filecompress::handle_fc_done_text(
                            api,
                            &message,
                            uid,
                            flow_manager,
                            database,
                        )
                        .await;
                        return Ok(());
                    }
                    if trimmed == t("fc.cancel_button")
                        || trimmed.contains(&t("emoji.cancel_button"))
                        || trimmed == "انصراف"
                        || trimmed == "لغو"
                    {
                        let config = match flow_manager.get(uid) {
                            FlowState::AwaitingCompressFiles { config, .. } => *config,
                            FlowState::AwaitingCompressOptions { config } => config,
                            _ => crate::filecompress::CompressConfig::default(),
                        };
                        flow_manager.set(
                            uid,
                            FlowState::AwaitingCompressOptions {
                                config: config.clone(),
                            },
                        );
                        let remove_params = frankenstein::methods::SendMessageParams::builder()
                            .chat_id(message.chat.id)
                            .text("\u{200B}")
                            .reply_markup(frankenstein::types::ReplyMarkup::ReplyKeyboardRemove(
                                frankenstein::types::ReplyKeyboardRemove::builder()
                                    .remove_keyboard(true)
                                    .build(),
                            ))
                            .build();
                        if let Ok(res) = api.send_message(&remove_params).await {
                            let _ = api
                                .delete_message(
                                    &frankenstein::methods::DeleteMessageParams::builder()
                                        .chat_id(message.chat.id)
                                        .message_id(res.result.message_id)
                                        .build(),
                                )
                                .await;
                        }
                        crate::filecompress::send_options_menu(api, message.chat.id, &config).await;
                        return Ok(());
                    }
                }

                if message.document.is_some()
                    || message.video.is_some()
                    || message.audio.is_some()
                    || message.photo.is_some()
                    || message.voice.is_some()
                    || message.video_note.is_some()
                    || message.animation.is_some()
                {
                    let trace_id = next_trace_id();
                    log_trace(
                        trace_id,
                        "filecompress_file_dispatched",
                        &format!("user_id={uid} chat_id={}", message.chat.id),
                    );
                    crate::filecompress::handle_fc_file(api, &message, uid, flow_manager, database)
                        .await;
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingStudioTrimVideo) {
                if message.video.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_ev!("studio_trim", trace_id, "video_dispatched", "user_id" => uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let mut fm = flow_manager.clone();
                    flow_manager.clear(uid);
                    super::spawn_user_task(async move {
                        crate::studio::trim::handle_video_upload(&api2, &msg2, uid, &mut fm).await;
                    });
                    return Ok(());
                }
            }

            if matches!(
                flow_manager.get(uid),
                FlowState::AwaitingStudioCompressVideo
            ) {
                if message.video.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_ev!("studio_compress", trace_id, "video_dispatched", "user_id" => uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let fm = flow_manager.clone();
                    flow_manager.clear(uid);
                    super::spawn_user_task(async move {
                        crate::studio::compress::handle_video_upload(
                            &api2, msg2, uid, trace_id, &fm,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingStudioExtractVideo) {
                if message.video.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_ev!("studio_extract", trace_id, "video_dispatched", "user_id" => uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let fm = flow_manager.clone();
                    flow_manager.clear(uid);
                    super::spawn_user_task(async move {
                        crate::studio::extract::handle_video_upload(
                            &api2, msg2, uid, trace_id, &fm,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }

            if let FlowState::AwaitingStudioBurnInput { session } = flow_manager.get(uid) {
                if message.video.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_ev!("studio_burn", trace_id, "input_dispatched", "user_id" => uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let mut fm = flow_manager.clone();
                    let db2 = database.clone();
                    super::spawn_user_task(async move {
                        crate::studio::burn::handle_input_message(
                            &api2, &msg2, uid, session, &mut fm, &db2,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }

            if let FlowState::AwaitingStudioTrimRanges {
                file_id,
                filename,
                duration_secs,
            } = flow_manager.get(uid)
            {
                if message.text.is_some() {
                    let trace_id = next_trace_id();
                    log_ev!("studio_trim", trace_id, "ranges_dispatched", "user_id" => uid);
                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let fm = flow_manager.clone();
                    let db2 = database.clone();
                    flow_manager.clear(uid);
                    super::spawn_user_task(async move {
                        crate::studio::trim::handle_ranges_input(
                            &api2,
                            &msg2,
                            uid,
                            &file_id,
                            &filename,
                            duration_secs,
                            &fm,
                            db2,
                        )
                        .await;
                    });
                    return Ok(());
                }
            }
        }
    }

    // Step 5: command dispatch
    if let Some(text) = message.text.as_deref() {
        let cmd = text.split('@').next().unwrap_or(text);
        eprintln!("[dispatch event=cmd] user_id={user_id:?} cmd={cmd:?}");
        if cmd == "/ref" || cmd == "/referral" {
            if let Some(uid) = user_id {
                crate::rank::panel::send_referral(api, message.chat.id, uid, database).await;
            }
            return Ok(());
        }
        if cmd == "/rank" {
            eprintln!(
                "[dispatch event=rank_menu] user_id={user_id:?} chat_id={}",
                message.chat.id
            );
            crate::rank::menu::send_rank_menu(api, message.chat.id).await;
            return Ok(());
        }
        if cmd == "/panel" {
            if let Some(uid) = user_id {
                crate::rank::panel::send_user_panel(api, message.chat.id, uid, database).await;
            }
            return Ok(());
        }
        if cmd == "/language" {
            send_lang_picker(api, message.chat.id).await?;
            return Ok(());
        }
        if let Some(rest) = text.strip_prefix("/ip ") {
            if let Some(uid) = user_id {
                handle_ip_command(api, message.chat.id, uid, rest.trim()).await;
            }
            return Ok(());
        }
        // Custom emoji pack panel (admin only, hidden from every user-facing menu).
        if cmd == "/emoji" {
            let trace = next_trace_id();
            let is_admin = config::admin_user_id()
                .map(|id| Some(id) == user_id)
                .unwrap_or(false);
            log_trace(
                trace,
                "cmd_emoji",
                &format!("user_id={user_id:?} is_admin={is_admin}"),
            );
            if is_admin {
                emoji_handler::handle_emoji_command(api, &message, flow_manager, database).await;
            }
            return Ok(());
        }
        // Generate gift code (admin only): /re 30d es 1u or /re
        if cmd == "/re" || text.starts_with("/re ") {
            let is_admin = config::admin_user_id()
                .map(|id| Some(id) == user_id)
                .unwrap_or(false);
            if is_admin {
                if let Some(uid) = user_id {
                    let rest = text.split_once(' ').map(|(_, r)| r.trim()).unwrap_or("");
                    crate::redeem::handle::handle_generate(
                        api,
                        message.chat.id,
                        uid,
                        rest,
                        database,
                    )
                    .await;
                }
            }
            return Ok(());
        }
        match text {
            "/i18n_reload" => {
                let is_admin = config::admin_user_id()
                    .map(|id| Some(id) == user_id)
                    .unwrap_or(false);
                if is_admin {
                    reload_i18n();
                    crate::bot::send_text(api, message.chat.id, "✅ config/i18n.json reloaded.")
                        .await?;
                }
            }
            "/start" => send_start_menu(api, message.chat.id).await?,

            _ => {
                if let (Some(uid), Some((ip, note))) = (user_id, detect_ip(text)) {
                    handle_ip_lookup_auto(api, message.chat.id, uid, ip, note).await;
                    return Ok(());
                }

                if let Some(uid) = user_id {
                    if crate::musicset::try_route_set(api, message.chat.id, uid, database, text) {
                        return Ok(());
                    }
                    if let Some(track_id) = crate::spotify::extract_spotify_track_id(text) {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "route_spotify_url",
                            &format!(
                                "user_id={uid} chat_id={} track_id={track_id}",
                                message.chat.id
                            ),
                        );
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        let msg_id2 = message.message_id;
                        let db2 = database.clone();
                        super::spawn_user_task(async move {
                            if let Err(e) = crate::spotify::handle_spotify_url(
                                &api2, chat_id2, msg_id2, uid, trace_id, &track_id, &db2,
                            )
                            .await
                            {
                                crate::stats::record_error_global("spotify", e).await;
                            }
                        });
                        return Ok(());
                    }

                    if let Some(sc_url) = crate::soundcloud::extract_soundcloud_url(text) {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "route_soundcloud_url",
                            &format!("user_id={uid} chat_id={} url={sc_url}", message.chat.id),
                        );
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        let msg_id2 = message.message_id;
                        let db2 = database.clone();
                        super::spawn_user_task(async move {
                            if let Err(e) = crate::soundcloud::handle_soundcloud_url(
                                &api2, chat_id2, msg_id2, uid, trace_id, &sc_url, &db2,
                            )
                            .await
                            {
                                crate::stats::record_error_global("soundcloud", e).await;
                            }
                        });
                        return Ok(());
                    }

                    if let Some(platform) = crate::surge_dl::detect_social_platform(text) {
                        if platform == "youtube" {
                            let urls = extract_youtube_urls(text);
                            let target_url = if !urls.is_empty() {
                                urls[0].to_string()
                            } else {
                                text.trim().to_string()
                            };
                            let trace_id = next_trace_id();
                            log_trace(
                                trace_id,
                                "route_youtube_url",
                                &format!(
                                    "user_id={uid} chat_id={} url={target_url}",
                                    message.chat.id
                                ),
                            );
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            let msg_id2 = message.message_id;
                            let pool2 = cookie_pool.clone();
                            let db2 = database.clone();
                            let rl_tx2 = rate_limit_tx.clone();
                            super::spawn_user_task(async move {
                                if let Err(e) = handle_youtube_url(
                                    &api2,
                                    chat_id2,
                                    msg_id2,
                                    Some(uid),
                                    trace_id,
                                    &target_url,
                                    pool2,
                                    &db2,
                                    &rl_tx2,
                                )
                                .await
                                {
                                    crate::stats::record_error_global("youtube", e).await;
                                }
                            });
                            return Ok(());
                        } else if platform == "spotify" {
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            super::spawn_user_task(async move {
                                let _ = crate::bot::send_text(
                                    &api2,
                                    chat_id2,
                                    &t("spotify.only_single_tracks"),
                                )
                                .await;
                            });
                            return Ok(());
                        } else if platform == "soundcloud" {
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            super::spawn_user_task(async move {
                                let _ = crate::bot::send_text(
                                    &api2,
                                    chat_id2,
                                    &t("soundcloud.only_single_tracks"),
                                )
                                .await;
                            });
                            return Ok(());
                        } else {
                            let trace_id = next_trace_id();
                            log_trace(
                                trace_id,
                                "route_unsupported_social_platform",
                                &format!(
                                    "user_id={uid} chat_id={} platform={platform} url={text}",
                                    message.chat.id
                                ),
                            );
                            let platform_name = t(&format!("platforms.{platform}"));
                            let msg_text = tf(
                                "surge.unsupported_platform",
                                &[("platform", &platform_name)],
                            );
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            super::spawn_user_task(async move {
                                let _ = crate::bot::send_text(&api2, chat_id2, &msg_text).await;
                                let _ = crate::bot::send_tools_menu(&api2, chat_id2).await;
                            });
                            return Ok(());
                        }
                    }

                    if crate::surge_dl::is_direct_link(text) {
                        let trace_id = next_trace_id();
                        log_trace(
                            trace_id,
                            "route_surge_dl_url",
                            &format!("user_id={uid} chat_id={} url={text}", message.chat.id),
                        );
                        let api2 = api.clone();
                        let msg2 = message.clone();
                        let fm2 = flow_manager.clone();
                        let db2 = database.clone();
                        super::spawn_user_task(async move {
                            handle_surge_text(&api2, &msg2, uid, &fm2, &db2).await;
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

async fn handle_callback(
    state: &mut AppState,
    callback_query: frankenstein::types::CallbackQuery,
) -> crate::error::Result<()> {
    let AppState {
        api,
        flow_manager,
        database,
        ..
    } = state;
    let cb_user_id = callback_query.from.id;
    let cb_data = callback_query.data.as_deref().unwrap_or("");
    let cb_chat_id = callback_query
        .message
        .as_ref()
        .and_then(|m| match m {
            MaybeInaccessibleMessage::Message(msg) => Some(msg.chat.id),
            _ => None,
        })
        .unwrap_or(0);
    {
        let trace_id = crate::log::next_trace_id();
        log_actor!("dispatch", trace_id, &callback_query.from, "clicked" => cb_data, "chat_id" => cb_chat_id);
    }

    // Helper to answer callback and extract Message
    #[allow(unused_macros)]
    macro_rules! answer_and_get_msg {
        () => {{
            let _ = api
                .answer_callback_query(
                    &AnswerCallbackQueryParams::builder()
                        .callback_query_id(callback_query.id.clone())
                        .build(),
                )
                .await;
            match callback_query.message.as_ref() {
                Some(MaybeInaccessibleMessage::Message(msg)) => Some(msg),
                _ => None,
            }
        }};
    }

    if cb_data.starts_with("rank:") || cb_data == crate::rank::paywall::CB_RANK_SHOW_MENU {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        crate::rank::menu::handle_rank_menu_callback(api, &callback_query).await;
        return Ok(());
    }

    if cb_data == CB_USER_PANEL || cb_data.starts_with("user:panel") {
        crate::rank::panel::handle_panel_callback(
            api,
            &callback_query,
            cb_user_id as i64,
            database,
        )
        .await;
        return Ok(());
    }

    if cb_data.starts_with("emoji:") {
        emoji_handler::handle_emoji_callback(api, &callback_query, flow_manager, database).await;
        return Ok(());
    }

    if cb_data.starts_with("studio")
        || cb_data.starts_with("stc:")
        || cb_data.starts_with("strex:")
        || cb_data.starts_with("stb:")
        || cb_data == CB_START_STUDIO
    {
        if let Some(msg) = answer_and_get_msg!() {
            if crate::studio::handle_callback(
                api,
                msg.chat.id,
                msg.message_id,
                cb_user_id as i64,
                cb_data,
                flow_manager,
                database,
            )
            .await
            {
                return Ok(());
            }
        }
    }

    if crate::youtube::handle_quality_callback(api, &callback_query, database).await {
        return Ok(());
    }

    if cb_data == crate::bot::CB_SP_CANCEL {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        crate::spotify::cancel_spotify_job(cb_user_id as i64);
        return Ok(());
    }

    if cb_data == crate::bot::CB_SC_CANCEL {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        crate::soundcloud::cancel_soundcloud_job(cb_user_id as i64);
        return Ok(());
    }

    if cb_data == crate::musicset::CB_MS_MODE_ONE || cb_data == crate::musicset::CB_MS_MODE_ZIP {
        let zip_mode = cb_data == crate::musicset::CB_MS_MODE_ZIP;
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        let msg_id = match callback_query.message.as_ref() {
            Some(MaybeInaccessibleMessage::Message(m)) => m.message_id,
            _ => 0,
        };
        crate::musicset::handle_mode_callback(
            api,
            cb_chat_id,
            cb_user_id as i64,
            msg_id,
            zip_mode,
            database,
        )
        .await;
        return Ok(());
    }

    if cb_data == crate::musicset::CB_MS_CANCEL {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::musicset::take_pending(cb_user_id as i64, message.message_id);
            let _ = edit_to_start_menu(api, message.chat.id, message.message_id).await;
        }
        return Ok(());
    }

    if cb_data == crate::musicset::CB_MS_JOBCANCEL {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        crate::musicset::cancel_job(cb_user_id as i64);
        return Ok(());
    }

    if cb_data == CB_START_GUIDE {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_start_guide",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r = crate::bot::keyboards::edit_to_guide(api, message.chat.id, message.message_id)
                .await;
            log_trace(
                trace_id,
                "cb_start_guide_done",
                &format!("ok={}", r.is_ok()),
            );
        }
        return Ok(());
    }

    if let Some(platform) = cb_data.strip_prefix(CB_START_GUIDE_PLATFORM) {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_start_guide_platform",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id} platform={platform}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if !crate::bot::keyboards::GUIDE_PLATFORMS.contains(&platform) {
            log_trace(trace_id, "cb_start_guide_platform", "=> fail err=unknown");
            return Ok(());
        }
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r = crate::bot::keyboards::edit_to_guide_platform(
                api,
                message.chat.id,
                message.message_id,
                platform,
            )
            .await;
            log_trace(
                trace_id,
                "cb_start_guide_platform_done",
                &format!("ok={}", r.is_ok()),
            );
        }
        return Ok(());
    }

    if cb_data == CB_START_PANEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_start_panel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r = edit_to_start_menu(api, message.chat.id, message.message_id).await;
            log_trace(
                trace_id,
                "cb_start_panel_done",
                &format!("ok={}", r.is_ok()),
            );
        }
        return Ok(());
    }

    if cb_data == CB_START_AI_LAB {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_start_ai_lab",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r = edit_to_ai_lab(api, message.chat.id, message.message_id).await;
            log_trace(
                trace_id,
                "cb_start_ai_lab_done",
                &format!("ok={}", r.is_ok()),
            );
        }
        return Ok(());
    }

    if cb_data == CB_START_TOOLS {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_start_tools",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r = edit_to_tools(api, message.chat.id, message.message_id).await;
            log_trace(
                trace_id,
                "cb_start_tools_done",
                &format!("ok={}", r.is_ok()),
            );
        }
        return Ok(());
    }

    if cb_data == crate::bot::constants::CB_START_DEV_CAFE {
        let trace_id = next_trace_id();
        log_ev!("dev_cafe", trace_id, "cb_start_dev_cafe", "user_id" => cb_user_id);
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let _ = crate::bot::edit_to_dev_cafe(api, message.chat.id, message.message_id).await;
        }
        return Ok(());
    }

    if cb_data == CB_START_LEADERBOARD {
        let trace_id = next_trace_id();
        log_actor!("dispatch", trace_id, &callback_query.from, "clicked" => cb_data);
        log_ev!("referral", trace_id, "leaderboard_enter", "user_id" => cb_user_id);
        stats::record_event_global("referral", "leaderboard_view", "ok", 1).await;
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r =
                edit_to_leaderboard(api, message.chat.id, message.message_id, database.as_ref())
                    .await;
            log_ev!("referral", trace_id, "leaderboard_done", "ok" => r.is_ok());
        }
        return Ok(());
    }

    if cb_data == CB_TOOLS_PDF_COMPRESS {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_pdf_compress_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_pdf_compress(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_PDF_MODE_SIMPLE {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_pdf_mode_simple",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_pdf_mode_simple(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_PDF_MODE_ADVANCED {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_pdf_mode_advanced",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .text(t("pdfcompress.advanced_soon"))
                    .show_alert(true)
                    .build(),
            )
            .await;
        return Ok(());
    }

    if cb_data == "pdf:jobcancel" {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Ok(jobs) = crate::pdfcompress::ACTIVE_PDF_JOBS.lock() {
            if let Some(cancel) = jobs.get(&(cb_user_id as i64)) {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        return Ok(());
    }

    if cb_data == CB_PDF_CANCEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_pdf_cancel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_pdf_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if let Some(level) = cb_data.strip_prefix(CB_PDF_LEVEL_PREFIX) {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_pdf_level",
            &format!("user_id={cb_user_id} level={level}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_pdf_level(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                level,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_TOOLS_IP_LOOKUP {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ip_lookup_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_ip_lookup(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_IP_LOOKUP_CANCEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ip_lookup_cancel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_ip_lookup_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == crate::filecompress::CB_TOOLS_FILECOMPRESS {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_filecompress_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::filecompress::enter_filecompress(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if let Some(action) = cb_data.strip_prefix(crate::filecompress::CB_FC_PREFIX) {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_filecompress_action",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id} action={cb_data}"),
        );
        // Callback response handled inside function
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::filecompress::handle_fc_callback(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                action,
                &callback_query.id,
                database,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_TOOLS_IP_LOOKUP {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ip_lookup_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_ip_lookup(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_IP_LOOKUP_CANCEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ip_lookup_cancel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_ip_lookup_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_TOOLS_SURGE {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_surge_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_surge_dl(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == crate::bot::constants::CB_TOOLS_PKG {
        let trace_id = next_trace_id();
        log_ev!("pkgconvert", trace_id, "cb_pkg_entry", "user_id" => cb_user_id);
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::pkgconvert::enter_pkgconvert(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == crate::bot::constants::CB_PKG_CANCEL {
        let trace_id = next_trace_id();
        log_ev!("pkgconvert", trace_id, "cb_pkg_cancel", "user_id" => cb_user_id);
        flow_manager.clear(cb_user_id as i64);
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let _ = crate::bot::edit_to_dev_cafe(api, message.chat.id, message.message_id).await;
        }
        return Ok(());
    }

    if cb_data == crate::bot::constants::CB_PKG_JOBCANCEL {
        let trace_id = next_trace_id();
        log_ev!("pkgconvert", trace_id, "cb_pkg_jobcancel", "user_id" => cb_user_id);
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::pkgconvert::handle_pkg_jobcancel(
                cb_user_id as i64,
                api,
                message.chat.id,
                message.message_id,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data.starts_with(crate::bot::constants::CB_PKG_CONVERT_PREFIX) {
        let trace_id = next_trace_id();
        log_ev!("pkgconvert", trace_id, "cb_pkg_convert", "user_id" => cb_user_id, "cb" => cb_data);
        let action = &cb_data[crate::bot::constants::CB_PKG_CONVERT_PREFIX.len()..];
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::pkgconvert::handle_pkg_callback(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                action,
                &callback_query.id,
                database,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_SURGE_CANCEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_surge_cancel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_surge_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_SURGE_CONFIRM_ORIGINAL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_surge_confirm_original",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_surge_confirm_original(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_SURGE_CONFIRM_RENAME {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_surge_confirm_rename",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_surge_confirm_rename(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_STT {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_stt_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_stt_config(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                database,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data.starts_with("stt:") {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "stt_callback",
            &format!("user_id={cb_user_id} data={cb_data:?}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_stt_callback(
                api,
                cb_data,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                database,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_DENOISE {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_denoise_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            denoise::enter_denoise(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_DENOISE_CANCEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "denoise_cancel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            denoise::handle_denoise_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_UPSCALE {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_upscale_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_upscale(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_UPSCALE_CANCEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "upscale_cancel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_upscale_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_UPSCALE_ANIME_TOGGLE {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_upscale_anime_toggle(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data.starts_with(CB_UPSCALE_MODEL_PREFIX) {
        let trace_id = next_trace_id();
        let model_name = cb_data.strip_prefix(CB_UPSCALE_MODEL_PREFIX).unwrap_or("");
        log_trace(
            trace_id,
            "upscale_model_pick",
            &format!("user_id={cb_user_id} model={model_name}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_upscale_model_pick(
                api,
                model_name,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_SEP {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_sep_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_separation(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data.starts_with(CB_SEP_PREFIX) {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "sep_callback",
            &format!("user_id={cb_user_id} data={cb_data:?}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;

        if cb_data == crate::bot::CB_SEP_DIRECT {
            if let Some(MaybeInaccessibleMessage::Message(message)) = &callback_query.message {
                let file_id = message
                    .audio
                    .as_ref()
                    .map(|a| a.file_id.as_str())
                    .or_else(|| message.voice.as_ref().map(|v| v.file_id.as_str()))
                    .or_else(|| message.document.as_ref().map(|d| d.file_id.as_str()));

                if let Some(fid) = file_id {
                    handle_direct_separation(
                        api,
                        message.chat.id,
                        cb_user_id as i64,
                        fid,
                        flow_manager,
                    )
                    .await;
                }
            }
            return Ok(());
        }

        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_separation_callback(
                api,
                cb_data,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                database,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_GWM {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_gwm_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_gwm(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_DEOLDIFY {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_deoldify_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::deoldify::enter_deoldify(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_DEOLDIFY_CANCEL {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::deoldify::handle_deoldify_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_NOBG {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_nobg_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_nobg(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_NOBG_CANCEL {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_nobg_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_TTS {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_ai_tts_entry",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::moss_tts::enter_tts(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                database.clone(),
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == crate::moss_tts::CB_TTS_JOB_CANCEL {
        let trace_id = next_trace_id();
        let signalled = crate::moss_tts::signal_tts_cancel(cb_user_id as i64);
        log_trace(
            trace_id,
            "tts_job_cancel",
            &format!("user_id={cb_user_id} signalled={signalled}"),
        );
        // Toast text from i18n; progress message cleared by main handler
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .text(crate::i18n::t("tts.cancel_button"))
                    .build(),
            )
            .await;
        return Ok(());
    }

    if cb_data == CB_TTS_CANCEL {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::moss_tts::handle_tts_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_GWM_CANCEL {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "gwm_cancel",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_gwm_cancel(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
            )
            .await;
        }
        return Ok(());
    }

    let is_admin = config::admin_user_id()
        .map(|id| id == cb_user_id as i64)
        .unwrap_or(false);

    if cb_data == CB_ADMIN_PANEL && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            use crate::emoji::panel::btn_icon;
            use crate::i18n::t;
            let kb = frankenstein::types::InlineKeyboardMarkup::builder()
                .inline_keyboard(vec![
                    vec![btn_icon(&t("admin.stats_button"), CB_ADMIN_STATS, "stats")],
                    vec![btn_icon(
                        &t("admin.force_join_button"),
                        CB_ADMIN_FORCE_JOIN,
                        "",
                    )],
                    vec![btn_icon(
                        &t("admin.gencode_button"),
                        CB_ADMIN_GEN_CODE,
                        "panel",
                    )],
                    vec![btn_icon(
                        &t("admin.broadcast_button"),
                        CB_ADMIN_BROADCAST,
                        "",
                    )],
                    vec![btn_icon(
                        &t("admin.back"),
                        crate::bot::CB_START_PANEL,
                        "back",
                    )],
                ])
                .build();
            let _ = crate::bot::edit_text(
                api,
                message.chat.id,
                message.message_id,
                &t("admin.panel_title"),
                Some(kb),
            )
            .await;
        }
        return Ok(());
    }

    // Stats hub + every section page share one arm: `admin:stats` = overview.
    if (cb_data == CB_ADMIN_STATS || cb_data.starts_with(CB_ADMIN_SECTION)) && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let key = cb_data
                .strip_prefix(CB_ADMIN_SECTION)
                .unwrap_or(crate::admin::SEC_OVERVIEW);
            let view = match database {
                Some(db) => crate::admin::render_section(db.client(), key).await,
                None => crate::admin::SectionView {
                    text: crate::i18n::t("admin.db_missing"),
                    html: false,
                },
            };
            let kb = crate::admin::stats_keyboard(key);
            let _ = if view.html {
                crate::bot::edit_text_html(
                    api,
                    message.chat.id,
                    message.message_id,
                    &view.text,
                    Some(kb),
                )
                .await
            } else {
                crate::bot::edit_text(
                    api,
                    message.chat.id,
                    message.message_id,
                    &view.text,
                    Some(kb),
                )
                .await
            };
        }
        return Ok(());
    }

    if cb_data == CB_ADMIN_FORCE_JOIN && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::force_join::open_menu(api, message.chat.id, message.message_id).await;
        }
        return Ok(());
    }

    if cb_data == crate::force_join::CB_FJ_TOGGLE && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        crate::force_join::toggle_enabled().await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::force_join::open_menu(api, message.chat.id, message.message_id).await;
        }
        return Ok(());
    }

    if cb_data == crate::force_join::CB_FJ_VIEW && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::force_join::open_locks_list(api, message.chat.id, message.message_id).await;
        }
        return Ok(());
    }

    if cb_data == crate::force_join::CB_FJ_ADD_NEW && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::force_join::prompt_add_new(
                api,
                message.chat.id,
                message.message_id,
                flow_manager,
                cb_user_id as i64,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == crate::force_join::CB_FJ_ADD_CANCEL && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        flow_manager.clear(cb_user_id as i64);
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::force_join::open_locks_list(api, message.chat.id, message.message_id).await;
        }
        return Ok(());
    }

    if let Some(id_str) = cb_data.strip_prefix(crate::force_join::FJ_MANAGE_PREFIX) {
        if is_admin {
            let _ = api
                .answer_callback_query(
                    &AnswerCallbackQueryParams::builder()
                        .callback_query_id(callback_query.id.clone())
                        .build(),
                )
                .await;
            if let (Some(MaybeInaccessibleMessage::Message(message)), Ok(lock_id)) =
                (callback_query.message, id_str.parse::<i64>())
            {
                crate::force_join::open_manage(
                    api,
                    message.chat.id,
                    message.message_id,
                    lock_id,
                    database,
                )
                .await;
            }
        }
        return Ok(());
    }

    if let Some(id_str) = cb_data.strip_prefix(crate::force_join::FJ_MODE_PREFIX) {
        if is_admin {
            if let Ok(lock_id) = id_str.parse::<i64>() {
                use crate::force_join::ToggleModeResult;
                let result = crate::force_join::toggle_lock_mode(api, lock_id).await;
                let alert_text = match &result {
                    ToggleModeResult::BotNotAdmin => Some(t("force_join.bot_not_admin")),
                    ToggleModeResult::NoChatId => Some(t("force_join.no_chat_id")),
                    _ => None,
                };
                let ack = match alert_text {
                    Some(text) => AnswerCallbackQueryParams::builder()
                        .callback_query_id(callback_query.id.clone())
                        .text(text)
                        .show_alert(true)
                        .build(),
                    None => AnswerCallbackQueryParams::builder()
                        .callback_query_id(callback_query.id.clone())
                        .build(),
                };
                let _ = api.answer_callback_query(&ack).await;
                if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                    crate::force_join::open_manage(
                        api,
                        message.chat.id,
                        message.message_id,
                        lock_id,
                        database,
                    )
                    .await;
                }
            } else {
                let _ = api
                    .answer_callback_query(
                        &AnswerCallbackQueryParams::builder()
                            .callback_query_id(callback_query.id.clone())
                            .build(),
                    )
                    .await;
            }
        }
        return Ok(());
    }

    // Lock field edit wizards: name/time/member/reserve -> text prompt
    {
        use crate::force_join::{
            FJ_MEMBER_PREFIX, FJ_NAME_PREFIX, FJ_RESERVE_PREFIX, FJ_TIME_PREFIX,
        };
        let field = if let Some(id) = cb_data.strip_prefix(FJ_NAME_PREFIX) {
            Some(("name", id))
        } else if let Some(id) = cb_data.strip_prefix(FJ_TIME_PREFIX) {
            Some(("time", id))
        } else if let Some(id) = cb_data.strip_prefix(FJ_MEMBER_PREFIX) {
            Some(("member", id))
        } else if let Some(id) = cb_data.strip_prefix(FJ_RESERVE_PREFIX) {
            Some(("reserve", id))
        } else {
            None
        };
        if let Some((field, id_str)) = field {
            if is_admin {
                let _ = api
                    .answer_callback_query(
                        &AnswerCallbackQueryParams::builder()
                            .callback_query_id(callback_query.id.clone())
                            .build(),
                    )
                    .await;
                if let (Some(MaybeInaccessibleMessage::Message(message)), Ok(lock_id)) =
                    (callback_query.message, id_str.parse::<i64>())
                {
                    crate::force_join::prompt_field(
                        api,
                        message.chat.id,
                        message.message_id,
                        lock_id,
                        field,
                        flow_manager,
                        cb_user_id as i64,
                    )
                    .await;
                }
            }
            return Ok(());
        }
    }

    if let Some(id_str) = cb_data.strip_prefix(crate::force_join::FJ_DELETE_PREFIX) {
        if is_admin {
            let _ = api
                .answer_callback_query(
                    &AnswerCallbackQueryParams::builder()
                        .callback_query_id(callback_query.id.clone())
                        .build(),
                )
                .await;
            if let (Some(MaybeInaccessibleMessage::Message(message)), Ok(lock_id)) =
                (callback_query.message, id_str.parse::<i64>())
            {
                crate::force_join::open_delete_confirm(
                    api,
                    message.chat.id,
                    message.message_id,
                    lock_id,
                )
                .await;
            }
        }
        return Ok(());
    }

    if let Some(id_str) = cb_data.strip_prefix(crate::force_join::FJ_DELETE_YES_PREFIX) {
        if is_admin {
            let _ = api
                .answer_callback_query(
                    &AnswerCallbackQueryParams::builder()
                        .callback_query_id(callback_query.id.clone())
                        .build(),
                )
                .await;
            if let (Some(MaybeInaccessibleMessage::Message(message)), Ok(lock_id)) =
                (callback_query.message, id_str.parse::<i64>())
            {
                crate::force_join::delete_lock(lock_id).await;
                crate::force_join::open_locks_list(api, message.chat.id, message.message_id).await;
            }
        }
        return Ok(());
    }

    if cb_data == crate::force_join::CB_FJ_NOOP && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        return Ok(());
    }

    if cb_data == CB_ADMIN_GEN_CODE && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            crate::redeem::handle::open_panel_edit(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
            )
            .await;
        }
        return Ok(());
    }

    // Code generation GUI buttons (gc:*) - admin only
    if cb_data.starts_with(crate::redeem::panel::CB_GC_PREFIX) && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if cb_data != crate::redeem::panel::CB_GC_NOP {
            if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
                crate::redeem::handle::handle_panel_callback(
                    api,
                    message.chat.id,
                    message.message_id,
                    cb_user_id as i64,
                    cb_data,
                    database,
                )
                .await;
            }
        }
        return Ok(());
    }

    // ── Admin Broadcast Handlers ─────────────────────────────────────────────
    if cb_data == CB_ADMIN_BROADCAST && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        flow_manager.clear(cb_user_id as i64);
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let kb = crate::admin::broadcast::broadcast_menu_keyboard(false);
            let _ = crate::bot::edit_text_md(
                api,
                message.chat.id,
                message.message_id,
                &t("admin.broadcast.menu_title"),
                Some(kb),
            )
            .await;
        }
        return Ok(());
    }

    if cb_data.starts_with(CB_BROADCAST_TOGGLE_PIN) && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        // New state rides in the callback data: `broadcast:toggle_pin:{0|1}`.
        let new_pin_state = cb_data.ends_with(":1");
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let kb = crate::admin::broadcast::broadcast_menu_keyboard(new_pin_state);
            let _ = crate::bot::edit_text_md(
                api,
                message.chat.id,
                message.message_id,
                &t("admin.broadcast.menu_title"),
                Some(kb),
            )
            .await;
        }
        return Ok(());
    }

    if (cb_data.starts_with(CB_BROADCAST_MODE_COPY)
        || cb_data.starts_with(CB_BROADCAST_MODE_FORWARD))
        && is_admin
    {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        let mode = if cb_data.starts_with(CB_BROADCAST_MODE_FORWARD) {
            BroadcastMode::Forward
        } else {
            BroadcastMode::Copy
        };
        let pin = cb_data.ends_with(":1");
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            flow_manager.set(
                cb_user_id as i64,
                FlowState::AwaitingBroadcastBanner { mode, pin },
            );
            let kb = InlineKeyboardMarkup::builder()
                .inline_keyboard(vec![vec![crate::emoji::panel::btn_icon(
                    &t("admin.back"),
                    CB_ADMIN_BROADCAST,
                    "back",
                )]])
                .build();
            let _ = crate::bot::edit_text_md(
                api,
                message.chat.id,
                message.message_id,
                &t("admin.broadcast.prompt_send_banner"),
                Some(kb),
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_BROADCAST_SEND_ACTIVE && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            if let FlowState::AwaitingBroadcastTarget {
                mode,
                pin,
                banner_chat_id,
                banner_message_id,
                ..
            } = flow_manager.get(cb_user_id as i64)
            {
                flow_manager.clear(cb_user_id as i64);
                let db_client = database.as_ref().map(|db| db.client_arc());
                crate::admin::broadcast::spawn_broadcast_job(
                    api.clone(),
                    db_client,
                    message.chat.id,
                    mode,
                    pin,
                    banner_chat_id,
                    banner_message_id,
                    true,
                    None,
                );
            }
        }
        return Ok(());
    }

    if cb_data == CB_BROADCAST_SEND_ALL && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            if let FlowState::AwaitingBroadcastTarget {
                mode,
                pin,
                banner_chat_id,
                banner_message_id,
                ..
            } = flow_manager.get(cb_user_id as i64)
            {
                flow_manager.clear(cb_user_id as i64);
                let db_client = database.as_ref().map(|db| db.client_arc());
                crate::admin::broadcast::spawn_broadcast_job(
                    api.clone(),
                    db_client,
                    message.chat.id,
                    mode,
                    pin,
                    banner_chat_id,
                    banner_message_id,
                    false,
                    None,
                );
            }
        }
        return Ok(());
    }

    // Unknown callback → start menu
    eprintln!(
        "[main event=callback_unhandled] user_id={cb_user_id} chat_id={cb_chat_id} data={cb_data:?}"
    );
    let _ = api
        .answer_callback_query(
            &AnswerCallbackQueryParams::builder()
                .callback_query_id(callback_query.id)
                .build(),
        )
        .await;
    if cb_chat_id != 0 {
        flow_manager.clear(cb_user_id as i64);
        let _ = send_start_menu(api, cb_chat_id).await;
    }
    Ok(())
}
