use frankenstein::{
    AsyncTelegramApi, methods::AnswerCallbackQueryParams, types::InlineKeyboardMarkup,
    types::MaybeInaccessibleMessage,
};

use crate::app::state::AppState;
use crate::bot::{
    CB_ADMIN_BROADCAST, CB_ADMIN_FORCE_JOIN, CB_ADMIN_GEN_CODE, CB_ADMIN_PANEL, CB_ADMIN_SECTION,
    CB_ADMIN_STATS, CB_AI_DENOISE, CB_AI_DEOLDIFY, CB_AI_GWM, CB_AI_NOBG, CB_AI_SEP, CB_AI_STT,
    CB_AI_TTS, CB_AI_UPSCALE, CB_BROADCAST_MODE_COPY, CB_BROADCAST_MODE_FORWARD,
    CB_BROADCAST_SEND_ACTIVE, CB_BROADCAST_SEND_ALL, CB_BROADCAST_TOGGLE_PIN, CB_DENOISE_CANCEL,
    CB_DEOLDIFY_CANCEL, CB_NOBG_CANCEL, CB_START_AI_LAB, CB_START_GUIDE, CB_START_GUIDE_PLATFORM,
    CB_START_LEADERBOARD, CB_START_STUDIO, CB_START_TOOLS, CB_TTS_CANCEL, CB_USER_PANEL,
};
use crate::emoji::{BroadcastMode, FlowState, handler as emoji_handler};
use crate::youtube::trace::log_trace;

use crate::bot::{
    edit_to_ai_lab, edit_to_leaderboard, edit_to_start_menu, edit_to_tools, send_start_menu,
};
use crate::config;
use crate::denoise;
use crate::emoji::panel::CB_START_PANEL;
use crate::feynobg::{enter_nobg, handle_nobg_cancel};
use crate::gemini_watermark::{CB_GWM_CANCEL, enter_gwm, handle_gwm_cancel};
use crate::i18n::t;
use crate::ip_lookup::{
    CB_IP_LOOKUP_CANCEL, CB_TOOLS_IP_LOOKUP, enter_ip_lookup, handle_ip_lookup_cancel,
};
use crate::log::next_trace_id;
use crate::pdfcompress::{
    CB_PDF_CANCEL, CB_PDF_LEVEL_PREFIX, CB_PDF_MODE_ADVANCED, CB_PDF_MODE_SIMPLE,
    CB_TOOLS_PDF_COMPRESS, enter_pdf_compress, handle_pdf_cancel, handle_pdf_level,
    handle_pdf_mode_simple,
};
use crate::separation::{
    CB_SEP_PREFIX, enter_separation, handle_direct_separation, handle_separation_callback,
};
use crate::stats;
use crate::stt::handle::{enter_stt_config, handle_stt_callback};
use crate::surge_dl::{
    CB_SURGE_CANCEL, CB_SURGE_CONFIRM_ORIGINAL, CB_SURGE_CONFIRM_RENAME, CB_TOOLS_SURGE,
    enter_surge_dl, handle_surge_cancel, handle_surge_confirm_original,
    handle_surge_confirm_rename,
};
use crate::upscale::{
    CB_UPSCALE_ANIME_TOGGLE, CB_UPSCALE_CANCEL, CB_UPSCALE_MODEL_PREFIX, enter_upscale,
    handle_upscale_anime_toggle, handle_upscale_cancel, handle_upscale_model_pick,
};

pub(super) async fn handle_callback(
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
        crate::pdfcompress::cancel_pdf_job(cb_user_id as i64);
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

    if let Some(action) = cb_data.strip_prefix(crate::bot::constants::CB_PKG_CONVERT_PREFIX) {
        let trace_id = next_trace_id();
        log_ev!("pkgconvert", trace_id, "cb_pkg_convert", "user_id" => cb_user_id, "cb" => cb_data);
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
                Some(db) => match db.get().await {
                    Ok(client) => crate::admin::render_section(&client, key).await,
                    Err(_) => crate::admin::SectionView {
                        text: crate::i18n::t("admin.db_missing"),
                        html: false,
                    },
                },
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
                crate::admin::broadcast::spawn_broadcast_job(
                    api.clone(),
                    database.clone(),
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
                crate::admin::broadcast::spawn_broadcast_job(
                    api.clone(),
                    database.clone(),
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
