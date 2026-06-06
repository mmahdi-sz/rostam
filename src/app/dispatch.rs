use frankenstein::{
    AsyncTelegramApi,
    methods::AnswerCallbackQueryParams,
    types::MaybeInaccessibleMessage,
    updates::UpdateContent,
};

use crate::bot::{send_start_menu, edit_to_start_menu, edit_to_ai_lab};
use crate::bot::{
    CB_START_EMOJI, CB_START_YOUTUBE, CB_START_AI_LAB,
    CB_AI_DENOISE, CB_AI_UPSCALE, CB_AI_STT, CB_AI_SEP, CB_AI_GWM, CB_DENOISE_CANCEL,
};
use crate::config;
use crate::denoise;
use crate::emoji::{FlowState, handler as emoji_handler, panel::CB_START_PANEL};
use crate::gemini_watermark::{enter_gwm, handle_gwm_cancel, handle_gwm_image, CB_GWM_CANCEL};
use crate::i18n::{t, reload_i18n};
use crate::separation::{enter_separation, handle_separation_audio, handle_separation_callback, CB_SEP_PREFIX};
use crate::stt::{config::CB_STT_CANCEL, handle::{enter_stt_config, handle_stt_audio, handle_stt_callback}};
use crate::upscale::{
    enter_upscale, handle_upscale_anime_toggle, handle_upscale_cancel,
    handle_upscale_image, handle_upscale_model_pick,
    CB_UPSCALE_CANCEL, CB_UPSCALE_MODEL_PREFIX, CB_UPSCALE_ANIME_TOGGLE,
};
use crate::youtube::{extract_youtube_urls, handle_youtube_url, log_trace, next_trace_id};
use frankenstein::{
    methods::SendMessageParams,
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, LinkPreviewOptions, ReplyMarkup},
};

use super::state::AppState;

pub async fn handle_update(
    state: &mut AppState,
    content: UpdateContent,
) -> Result<(), Box<dyn std::error::Error>> {
    // DEV_MODE: فقط ادمین می‌تونه استفاده کنه
    if config::dev_mode() {
        let admin = config::admin_user_id();
        let sender = match &content {
            UpdateContent::Message(m) => m.from.as_ref().map(|u| u.id as i64),
            UpdateContent::CallbackQuery(c) => Some(c.from.id as i64),
            _ => None,
        };
        if sender.is_some() && sender != admin {
            eprintln!("[dev_mode] blocked user_id={:?}", sender);
            return Ok(());
        }
    }

    match content {
        UpdateContent::Message(message) => handle_message(state, *message).await?,
        UpdateContent::CallbackQuery(callback_query) => handle_callback(state, *callback_query).await?,
        _ => {}
    }
    Ok(())
}

async fn handle_message(
    state: &mut AppState,
    message: frankenstein::types::Message,
) -> Result<(), Box<dyn std::error::Error>> {
    let AppState { api, cookie_pool, database, flow_manager, rate_limit_tx } = state;
    let user_id = message.from.as_ref().map(|u| u.id as i64);

    // Step 1: addemoji link detection
    if let Some(uid) = user_id {
        if let Some(text) = message.text.as_deref() {
            if !text.trim_start().starts_with('/') {
                if let Some(pack_name) = emoji_handler::extract_addemoji_pack_name(text) {
                    emoji_handler::handle_addemoji_link(
                        api, &message, uid, &pack_name, flow_manager, database,
                    ).await;
                    return Ok(());
                }
            }
        }
    }

    // Step 2: /start always clears flow
    if let (Some(uid), Some("/start")) = (user_id, message.text.as_deref()) {
        flow_manager.clear(uid);
        send_start_menu(api, message.chat.id).await?;
        return Ok(());
    }

    // Step 3: «لغو عملیات» reply keyboard when Idle
    if let (Some(uid), Some(text)) = (user_id, message.text.as_deref()) {
        if text.contains("لغو عملیات") && matches!(flow_manager.get(uid), FlowState::Idle) {
            send_start_menu(api, message.chat.id).await?;
            return Ok(());
        }
    }

    // Step 4: active flow dispatch
    if let Some(uid) = user_id {
        if !matches!(flow_manager.get(uid), FlowState::Idle) {
            if emoji_handler::handle_emoji_flow_message(api, &message, uid, flow_manager, database).await {
                return Ok(());
            }

            if let FlowState::AwaitingSttAudio { config } = flow_manager.get(uid) {
                if message.voice.is_some() || message.audio.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(trace_id, "stt_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                    handle_stt_audio(api, &message, uid, &config).await;
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingDenoiseAudio) {
                if message.voice.is_some() || message.audio.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(trace_id, "denoise_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                    denoise::handle_denoise_audio(api, &message, uid, flow_manager).await;
                    return Ok(());
                }
            }

            if let FlowState::AwaitingUpscaleImage { scale_factor, model_name, .. } = flow_manager.get(uid) {
                if message.photo.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(trace_id, "upscale_route_dispatched", &format!("user_id={uid} model={model_name}"));
                    handle_upscale_image(api, &message, uid, scale_factor, &model_name, flow_manager).await;
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingSeparation) {
                if message.audio.is_some() || message.voice.is_some() || message.document.is_some() || message.video.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(trace_id, "separation_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                    handle_separation_audio(api, &message, uid, flow_manager).await;
                    return Ok(());
                }
            }

            if matches!(flow_manager.get(uid), FlowState::AwaitingGeminiWmImage) {
                if message.photo.is_some() || message.document.is_some() {
                    let trace_id = next_trace_id();
                    log_trace(trace_id, "gwm_route_dispatched", &format!("user_id={uid} chat_id={}", message.chat.id));
                    handle_gwm_image(api, &message, uid, flow_manager).await;
                    return Ok(());
                }
            }
        }
    }

    // Step 5: command dispatch
    if let Some(text) = message.text.as_deref() {
        if text == "/emoji" {
            emoji_handler::handle_emoji_command(api, &message, flow_manager, database).await;
            return Ok(());
        }
        if let Some(rest) = text.strip_prefix("/se") {
            emoji_handler::handle_se_command(api, &message, rest, database).await;
            return Ok(());
        }
        match text {
            "/i18n_reload" => {
                let is_admin = config::admin_user_id().map(|id| Some(id) == user_id).unwrap_or(false);
                if is_admin {
                    reload_i18n();
                    crate::bot::send_text(api, message.chat.id, "✅ i18n.json reloaded.").await?;
                }
            }
            "/start" => send_start_menu(api, message.chat.id).await?,
            _ => {
                let urls = extract_youtube_urls(text);
                for url in urls {
                    let trace_id = next_trace_id();
                    log_trace(trace_id, "route_youtube_url", &format!(
                        "user_id={user_id:?} chat_id={} url={url}", message.chat.id
                    ));
                    handle_youtube_url(
                        api, message.chat.id, message.message_id,
                        user_id, trace_id, &url, cookie_pool, database, rate_limit_tx,
                    ).await;
                }
            }
        }
    }
    Ok(())
}

async fn handle_callback(
    state: &mut AppState,
    callback_query: frankenstein::types::CallbackQuery,
) -> Result<(), Box<dyn std::error::Error>> {
    let AppState { api, flow_manager, database, .. } = state;
    let cb_user_id = callback_query.from.id;
    let cb_data = callback_query.data.as_deref().unwrap_or("");
    let cb_chat_id = callback_query.message.as_ref().and_then(|m| match m {
        MaybeInaccessibleMessage::Message(msg) => Some(msg.chat.id),
        _ => None,
    }).unwrap_or(0);

    eprintln!("[main event=callback_received] user_id={cb_user_id} chat_id={cb_chat_id} data={cb_data:?}");

    // Helper to answer callback and extract Message
    macro_rules! answer_and_get_msg {
        () => {{
            let _ = api.answer_callback_query(
                &AnswerCallbackQueryParams::builder()
                    .callback_query_id(callback_query.id.clone())
                    .build(),
            ).await;
            match callback_query.message.as_ref() {
                Some(MaybeInaccessibleMessage::Message(msg)) => Some(msg),
                _ => None,
            }
        }};
    }

    if cb_data.starts_with("emoji:") {
        emoji_handler::handle_emoji_callback(api, &callback_query, flow_manager, database).await;
        return Ok(());
    }

    if crate::youtube::handle_quality_callback(api, &callback_query).await {
        return Ok(());
    }

    if cb_data == CB_START_EMOJI {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_start_emoji", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            emoji_handler::open_emoji_panel(
                api, message.chat.id, callback_query.from.id as i64, flow_manager, database,
            ).await;
        }
        return Ok(());
    }

    if cb_data == CB_START_YOUTUBE {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_start_youtube", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let icon_id = t("emoji.panel.icons.back");
            let back_btn = InlineKeyboardButton {
                text: t("start.back"),
                icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') { None } else { Some(icon_id) },
                callback_data: Some(CB_START_PANEL.to_string()),
                style: Some(ButtonStyle::Primary),
                url: None, login_url: None, web_app: None,
                switch_inline_query: None, switch_inline_query_current_chat: None,
                switch_inline_query_chosen_chat: None, copy_text: None,
                callback_game: None, pay: None,
            };
            let keyboard = InlineKeyboardMarkup::builder().inline_keyboard(vec![vec![back_btn]]).build();
            let no_preview = LinkPreviewOptions::builder().is_disabled(true).build();
            let params = SendMessageParams::builder()
                .chat_id(message.chat.id)
                .text(t("start.youtube_info"))
                .link_preview_options(no_preview)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
                .build();
            let r = api.send_message(&params).await;
            log_trace(trace_id, "cb_start_youtube_sent", &format!("ok={}", r.is_ok()));
        }
        return Ok(());
    }

    if cb_data == CB_START_PANEL {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_start_panel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r = edit_to_start_menu(api, message.chat.id, message.message_id).await;
            log_trace(trace_id, "cb_start_panel_done", &format!("ok={}", r.is_ok()));
        }
        return Ok(());
    }

    if cb_data == CB_START_AI_LAB {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_start_ai_lab", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            let r = edit_to_ai_lab(api, message.chat.id, message.message_id).await;
            log_trace(trace_id, "cb_start_ai_lab_done", &format!("ok={}", r.is_ok()));
        }
        return Ok(());
    }

    if cb_data == CB_AI_STT {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_ai_stt_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_stt_config(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data.starts_with("stt:") {
        let trace_id = next_trace_id();
        log_trace(trace_id, "stt_callback", &format!("user_id={cb_user_id} data={cb_data:?}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_stt_callback(api, cb_data, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_DENOISE {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_ai_denoise_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            denoise::enter_denoise(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_DENOISE_CANCEL {
        let trace_id = next_trace_id();
        log_trace(trace_id, "denoise_cancel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            denoise::handle_denoise_cancel(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_UPSCALE {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_ai_upscale_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_upscale(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_UPSCALE_CANCEL {
        let trace_id = next_trace_id();
        log_trace(trace_id, "upscale_cancel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_upscale_cancel(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_UPSCALE_ANIME_TOGGLE {
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_upscale_anime_toggle(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data.starts_with(CB_UPSCALE_MODEL_PREFIX) {
        let trace_id = next_trace_id();
        let model_name = cb_data.strip_prefix(CB_UPSCALE_MODEL_PREFIX).unwrap_or("");
        log_trace(trace_id, "upscale_model_pick", &format!("user_id={cb_user_id} model={model_name}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_upscale_model_pick(api, model_name, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_SEP {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_ai_sep_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_separation(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data.starts_with(CB_SEP_PREFIX) {
        let trace_id = next_trace_id();
        log_trace(trace_id, "sep_callback", &format!("user_id={cb_user_id} data={cb_data:?}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_separation_callback(api, cb_data, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_AI_GWM {
        let trace_id = next_trace_id();
        log_trace(trace_id, "cb_ai_gwm_entry", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            enter_gwm(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    if cb_data == CB_GWM_CANCEL {
        let trace_id = next_trace_id();
        log_trace(trace_id, "gwm_cancel", &format!("user_id={cb_user_id} chat_id={cb_chat_id}"));
        let _ = api.answer_callback_query(
            &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id.clone()).build(),
        ).await;
        if let Some(MaybeInaccessibleMessage::Message(message)) = callback_query.message {
            handle_gwm_cancel(api, message.chat.id, message.message_id, cb_user_id as i64, flow_manager).await;
        }
        return Ok(());
    }

    // Unknown callback → start menu
    eprintln!("[main event=callback_unhandled] user_id={cb_user_id} chat_id={cb_chat_id} data={cb_data:?}");
    let _ = api.answer_callback_query(
        &AnswerCallbackQueryParams::builder().callback_query_id(callback_query.id).build(),
    ).await;
    if cb_chat_id != 0 {
        flow_manager.clear(cb_user_id as i64);
        let _ = send_start_menu(api, cb_chat_id).await;
    }
    Ok(())
}
