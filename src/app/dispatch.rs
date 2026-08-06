use frankenstein::{
    AsyncTelegramApi, methods::AnswerCallbackQueryParams, types::MaybeInaccessibleMessage,
    updates::UpdateContent,
};

use crate::bot::{
    CB_ADMIN_BROADCAST, CB_ADMIN_ERRORS, CB_ADMIN_FORCE_JOIN, CB_ADMIN_GEN_CODE, CB_ADMIN_PANEL, CB_ADMIN_STATS,
    CB_ADMIN_STATS_MORE, CB_AI_DENOISE, CB_AI_DEOLDIFY, CB_AI_GWM, CB_AI_NOBG, CB_AI_SEP, CB_AI_STT, CB_AI_TTS, CB_AI_UPSCALE,
    CB_BROADCAST_MODE_COPY, CB_BROADCAST_MODE_FORWARD, CB_BROADCAST_SEND_ACTIVE, CB_BROADCAST_SEND_ALL, CB_BROADCAST_TOGGLE_PIN,
    CB_DENOISE_CANCEL, CB_DEOLDIFY_CANCEL, CB_LANG_SET, CB_NOBG_CANCEL, CB_START_AI_LAB, CB_START_EMOJI, CB_START_LEADERBOARD, CB_START_TOOLS,
    CB_START_YOUTUBE, CB_TTS_CANCEL, CB_USER_PANEL,
};

use crate::bot::{
    edit_to_ai_lab, edit_to_leaderboard, edit_to_start_menu, edit_to_tools, send_lang_picker, send_start_menu,
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
    CB_SEP_PREFIX, enter_separation, handle_separation_audio, handle_separation_callback,
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
use frankenstein::{
    methods::SendMessageParams,
    types::{
        ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions, ReplyMarkup,
    },
};

use super::state::AppState;

pub async fn handle_update(
    state: &mut AppState,
    content: UpdateContent,
) -> crate::error::Result<()> {
    // DEV_MODE: فقط ادمین می‌تونه استفاده کنه
    if config::dev_mode() {
        let admin = config::admin_user_id();
        let sender = match &content {
            UpdateContent::Message(m) => m.from.as_ref().filter(|u| !u.is_bot).map(|u| u.id as i64),
            UpdateContent::CallbackQuery(c) => if c.from.is_bot { None } else { Some(c.from.id as i64) },
            _ => None,
        };
        if sender.is_some() && sender != admin {
            eprintln!("[dev_mode] blocked user_id={:?}", sender);
            return Ok(());
        }
    }

    // chat_member آپدیت قفل‌های عضویت — کش Redis قفل منطبق را تازه نگه می‌دارد.
    if let UpdateContent::ChatMember(cm) = &content {
        crate::force_join::on_chat_member_update(&cm.chat, &cm.new_chat_member).await;
        return Ok(());
    }

    let sender = match &content {
        UpdateContent::Message(m) => {
            // بی‌خیال پیام‌های ارسال شده توسط ربات‌ها (از جمله خود ربات یا ربات‌های دیگر)
            if m.from.as_ref().map_or(false, |u| u.is_bot) {
                return Ok(());
            }
            // بی‌خیال پیام‌های سرویسی تلگرام (مانند پین شدن پیام، عضویت/خروج کاربر و ...)
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

    // ── language gate ────────────────────────────────────────────────────────
    // callback "lang:set:xx" → ذخیره زبان، ack، ادامه عادی
    // بقیه بدون زبان → منوی انتخاب زبان بفرست و برگرد
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
            // بعد از ذخیره زبان، منوی شروع رو نشون بده
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

        // چک زبان فقط اگه DB وجود داشته باشه
        if state.database.is_some() {
            // redeem deep-link: gate رو bypass کن تا کد اول فعال بشه، بعد زبان انتخاب میشه
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

        // بدون DB: مفهومی برای redeem وجود نداره (نیازمند DB است) → همیشه گیت بزن
        if !gate_force_join(state, &content, uid, is_check_btn).await? {
            return Ok(());
        }
    }

    // بدون DB (یا update بدون sender): مستقیم dispatch با fa
    match content {
        UpdateContent::Message(message) => handle_message(state, *message).await?,
        UpdateContent::CallbackQuery(callback_query) => {
            handle_callback(state, *callback_query).await?
        }
        _ => {}
    }
    Ok(())
}

/// قفل عضویت اجباری — بعد از زبان و کد فعال‌سازی اجرا می‌شه. `is_check_btn` یعنی
/// کاربر روی دکمه‌ی سبز «عضو شدم» زده؛ در این حالت کش دور زده می‌شه (چک زنده) و
/// پاسخ به‌صورت toast/alert روی همون callback داده می‌شه، نه پیام جدید.
/// خروجی `Ok(true)` یعنی ادامه بده، `Ok(false)` یعنی همینجا برگرد.
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
        flow_clear_tx,
        ..
    } = state;
    let user_id = message.from.as_ref().map(|u| u.id as i64);
    let msg_text = message.text.as_deref().unwrap_or("");
    if let Some(u) = message.from.as_ref() {
        let trace_id = crate::log::next_trace_id();
        log_actor!("dispatch", trace_id, u, "msg" => msg_text.chars().take(40).collect::<String>());
    }

    // ثبت کاربر در stats. is_new_user برای گیت زدن attribution زیرمجموعه‌گیری لازم است
    // (فقط کاربری که همین الان برای اولین‌بار دیده شده باید به یک referrer نسبت داده شود).
    let mut is_new_user = false;
    if let Some(uid) = user_id {
        let username = message.from.as_ref().and_then(|u| u.username.as_deref());
        is_new_user = stats::record_user_global(uid, username).await;
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
            } else if let Ok(referrer_id) = payload.parse::<i64>() {
                // زیرمجموعه‌گیری: فقط کاربر واقعاً تازه‌وارد و غیر از خودِ referrer نسبت داده می‌شود.
                if is_new_user && referrer_id != uid {
                    if let Some(db) = database {
                        let trace_id = next_trace_id();
                        crate::referral::record_referral(db.client(), uid, referrer_id).await;
                        log_trace(
                            trace_id,
                            "referral_attributed",
                            &format!("referred_id={uid} referrer_id={referrer_id}"),
                        );
                    }
                }
                send_start_menu(api, message.chat.id).await?;
            } else {
                send_start_menu(api, message.chat.id).await?;
            }
            return Ok(());
        }
    }

    // Step 3: «لغو عملیات» reply keyboard when Idle
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
            // ادمین آرگومان‌های ساخت کد را فرستاده (مثل `30d es 1u`)
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

            // ادمین لینک قفل جدید را فرستاده
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

            // ادمین یوزرنیم/آیدی عددی/فوروارد برای لینک خصوصی فرستاده
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

            // ادمین ورودی ویزارد ویرایش فیلد قفل (نام/زمان/عضو/رزرو) را فرستاده
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

            // ادمین بنر همگانی را فرستاده است (هر نوع پیام/مدیا/متن)
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
                    let kb = crate::admin::broadcast::broadcast_target_keyboard(active_users, total_users);

                    let _ = crate::bot::send_text_with_kb(
                        api,
                        message.chat.id,
                        &prompt_text,
                        kb,
                    )
                    .await;
                }
                return Ok(());
            }

            // ادمین عدد سقف ارسال همگانی را فرستاده است (مثلاً 500)
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
                        let tx2 = flow_clear_tx.clone();
                        // Spawn so the event loop stays free during STT (minutes-long operation)
                        tokio::spawn(async move {
                            struct SttFlowGuard(tokio::sync::mpsc::UnboundedSender<i64>, i64);
                            impl Drop for SttFlowGuard {
                                fn drop(&mut self) {
                                    let _ = self.0.send(self.1);
                                }
                            }
                            let _guard = SttFlowGuard(tx2, uid);
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
                    tokio::spawn(async move {
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
                    tokio::spawn(async move {
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
                    tokio::spawn(async move {
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
                    tokio::spawn(async move {
                        crate::deoldify::handle_deoldify_image(
                            &api2,
                            &msg2,
                            uid,
                            &fm_clone,
                            db_clone,
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
                    tokio::spawn(async move {
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
                    tokio::spawn(async move {
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
                        tokio::spawn(async move {
                            if let Err(e) = handle_youtube_url(
                                &api2, chat_id2, msg_id2, user_id, trace_id, &target_url, pool2,
                                &db2, &rl_tx2,
                            )
                            .await
                            {
                                crate::stats::record_error_global("youtube", e).await;
                            }
                        });
                        return Ok(());
                    } else if let Some(p) = platform {
                        let platform_name = t(&format!("platforms.{p}"));
                        let text = tf("surge.unsupported_platform", &[("platform", &platform_name)]);
                        let api2 = api.clone();
                        let chat_id2 = message.chat.id;
                        tokio::spawn(async move {
                            let _ = crate::bot::send_text(&api2, chat_id2, &text).await;
                            let _ = crate::bot::send_tools_menu(&api2, chat_id2).await;
                        });
                        return Ok(());
                    }

                    let api2 = api.clone();
                    let msg2 = message.clone();
                    let fm2 = flow_manager.clone();
                    let db2 = database.clone();
                    tokio::spawn(async move {
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
                    crate::filecompress::handle_fc_file(
                        api,
                        &message,
                        uid,
                        flow_manager,
                        database,
                    )
                    .await;
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
        // ساخت کد هدیه (فقط ادمین): /re 30d es 1u یا /re
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
                                &format!("user_id={uid} chat_id={} url={target_url}", message.chat.id),
                            );
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            let msg_id2 = message.message_id;
                            let pool2 = cookie_pool.clone();
                            let db2 = database.clone();
                            let rl_tx2 = rate_limit_tx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_youtube_url(
                                    &api2, chat_id2, msg_id2, Some(uid), trace_id, &target_url, pool2,
                                    &db2, &rl_tx2,
                                )
                                .await
                                {
                                    crate::stats::record_error_global("youtube", e).await;
                                }
                            });
                            return Ok(());
                        } else {
                            let trace_id = next_trace_id();
                            log_trace(
                                trace_id,
                                "route_unsupported_social_platform",
                                &format!("user_id={uid} chat_id={} platform={platform} url={text}", message.chat.id),
                            );
                            let platform_name = t(&format!("platforms.{platform}"));
                            let msg_text = tf("surge.unsupported_platform", &[("platform", &platform_name)]);
                            let api2 = api.clone();
                            let chat_id2 = message.chat.id;
                            tokio::spawn(async move {
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
                        tokio::spawn(async move {
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
        flow_clear_tx,
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

    if crate::youtube::handle_quality_callback(api, &callback_query, database).await {
        return Ok(());
    }

    if cb_data == CB_START_EMOJI {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_start_emoji",
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
            emoji_handler::open_emoji_panel(
                api,
                message.chat.id,
                callback_query.from.id as i64,
                flow_manager,
                database,
            )
            .await;
        }
        return Ok(());
    }

    if cb_data == CB_START_YOUTUBE {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_start_youtube",
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
            let icon_id = t("emoji.panel.icons.back");
            let back_btn = InlineKeyboardButton {
                text: t("start.back"),
                icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') {
                    None
                } else {
                    Some(icon_id)
                },
                callback_data: Some(CB_START_PANEL.to_string()),
                style: Some(ButtonStyle::Primary),
                url: None,
                login_url: None,
                web_app: None,
                switch_inline_query: None,
                switch_inline_query_current_chat: None,
                switch_inline_query_chosen_chat: None,
                copy_text: None,
                callback_game: None,
                pay: None,
            };
            let keyboard = InlineKeyboardMarkup::builder()
                .inline_keyboard(vec![vec![back_btn]])
                .build();
            let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
            let params = SendMessageParams::builder()
                .chat_id(message.chat.id)
                .text(t("start.youtube_info"))
                .link_preview_options(no_preview)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
                .build();
            let r = api.send_message(&params).await;
            log_trace(
                trace_id,
                "cb_start_youtube_sent",
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
            let r = edit_to_leaderboard(api, message.chat.id, message.message_id, database.as_ref()).await;
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

    if cb_data.starts_with(crate::filecompress::CB_FC_PREFIX) {
        let trace_id = next_trace_id();
        log_trace(
            trace_id,
            "cb_filecompress_action",
            &format!("user_id={cb_user_id} chat_id={cb_chat_id} action={cb_data}"),
        );
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let action = &cb_data[crate::filecompress::CB_FC_PREFIX.len()..];
            crate::filecompress::handle_fc_callback(
                api,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                action,
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
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_separation_callback(
                api,
                cb_data,
                message.chat.id,
                message.message_id,
                cb_user_id as i64,
                flow_manager,
                flow_clear_tx.clone(),
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

    if cb_data == CB_ADMIN_STATS && is_admin {
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
            let text = if let Some(db) = database {
                crate::admin::render_stats(db.client()).await
            } else {
                crate::i18n::t("admin.db_missing")
            };
            let kb = frankenstein::types::InlineKeyboardMarkup::builder()
                .inline_keyboard(vec![
                    vec![btn_icon(
                        &t("admin.stats_more_button"),
                        CB_ADMIN_STATS_MORE,
                        "stats",
                    )],
                    vec![btn_icon(
                        &t("admin.errors_button"),
                        CB_ADMIN_ERRORS,
                        "warning",
                    )],
                    vec![btn_icon(&t("admin.back"), CB_ADMIN_PANEL, "back")],
                ])
                .build();
            let _ =
                crate::bot::edit_text(api, message.chat.id, message.message_id, &text, Some(kb))
                    .await;
        }
        return Ok(());
    }

    if cb_data == CB_ADMIN_STATS_MORE && is_admin {
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
            let text = if let Some(db) = database {
                crate::admin::render_stats_more(db.client()).await
            } else {
                crate::i18n::t("admin.db_missing")
            };
            let kb = frankenstein::types::InlineKeyboardMarkup::builder()
                .inline_keyboard(vec![vec![btn_icon(
                    &t("admin.back"),
                    CB_ADMIN_STATS,
                    "back",
                )]])
                .build();
            let _ =
                crate::bot::edit_text(api, message.chat.id, message.message_id, &text, Some(kb))
                    .await;
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

    // ویزاردهای ویرایش فیلد قفل: نام/زمان/عضو/رزرو → پرامپت ورودی متنی
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
            crate::redeem::handle::open_panel(api, message.chat.id, cb_user_id as i64).await;
        }
        return Ok(());
    }

    // دکمه‌های پنل گرافیکی ساخت کد (gc:*) — فقط ادمین
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

    if cb_data == CB_ADMIN_ERRORS && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let text = if let Some(db) = database {
                crate::admin::render_errors_1d(db.client()).await
            } else {
                crate::i18n::t("admin.db_missing")
            };
            // HTML چون blockquote جمع‌شو داره — پیام جدید می‌فرستیم تا پنل دست‌نخورده بمونه.
            use crate::emoji::panel::btn_icon;
            use crate::i18n::t;
            use frankenstein::{ParseMode, methods::SendMessageParams};
            let kb = frankenstein::types::InlineKeyboardMarkup::builder()
                .inline_keyboard(vec![vec![btn_icon(
                    &t("admin.back"),
                    CB_ADMIN_PANEL,
                    "back",
                )]])
                .build();
            let params = SendMessageParams::builder()
                .chat_id(message.chat.id)
                .text(&text)
                .parse_mode(ParseMode::Html)
                .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(kb))
                .build();
            if let Err(e) = api.send_message(&params).await {
                eprintln!("[admin event=errors_send_failed] err={e}");
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

    if cb_data == CB_BROADCAST_TOGGLE_PIN && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        let (current_mode, current_pin) = match flow_manager.get(cb_user_id as i64) {
            FlowState::AwaitingBroadcastBanner { mode, pin } => (mode, pin),
            _ => (BroadcastMode::Copy, false),
        };
        let new_pin_state = !current_pin;
        flow_manager.set(
            cb_user_id as i64,
            FlowState::AwaitingBroadcastBanner {
                mode: current_mode,
                pin: new_pin_state,
            },
        );
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

    if cb_data == CB_BROADCAST_MODE_COPY && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let current_pin = match flow_manager.get(cb_user_id as i64) {
                FlowState::AwaitingBroadcastBanner { pin, .. } => pin,
                _ => false,
            };
            flow_manager.set(
                cb_user_id as i64,
                FlowState::AwaitingBroadcastBanner {
                    mode: BroadcastMode::Copy,
                    pin: current_pin,
                },
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

    if cb_data == CB_BROADCAST_MODE_FORWARD && is_admin {
        let _ = api
            .answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            )
            .await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let current_pin = match flow_manager.get(cb_user_id as i64) {
                FlowState::AwaitingBroadcastBanner { pin, .. } => pin,
                _ => false,
            };
            flow_manager.set(
                cb_user_id as i64,
                FlowState::AwaitingBroadcastBanner {
                    mode: BroadcastMode::Forward,
                    pin: current_pin,
                },
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
