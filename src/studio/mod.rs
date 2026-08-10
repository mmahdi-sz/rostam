//! Photo & Video Magic Studio (`studio`) top-level module.
//!
//! Houses media editing tools starting with video trimming (`studio_trim`).

pub mod compress;
pub mod pipeline;
pub mod trim;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, ReplyMarkup},
};

use crate::bot::constants::{
    CB_START_PANEL, CB_START_STUDIO, CB_STUDIO_COMPRESS, CB_STUDIO_PANEL, CB_STUDIO_TRIM,
    CB_STUDIO_TRIM_CANCEL, CB_STUDIO_TRIM_JOBCANCEL,
};
use crate::emoji::panel::{btn_icon, btn_icon_danger, btn_icon_primary};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t};
use crate::log::next_trace_id;

pub fn studio_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(
                &t("studio.trim_button"),
                CB_STUDIO_TRIM,
                "scissors",
            )],
            vec![btn_icon(
                &t("studio.compress_button"),
                CB_STUDIO_COMPRESS,
                "movie",
            )],
            vec![btn_icon_primary(
                &t("start.back"),
                CB_START_PANEL,
                "back",
            )],
        ])
        .build()
}

/// Enters the Photo & Video Magic Studio submenu.
pub async fn enter_studio(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.clear(user_id);
    log_actor_id!("studio", trace_id, user_id, "clicked" => CB_START_STUDIO);

    let text = apply_premium_to_md(&t("studio.title"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(studio_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}

/// Sends the Photo & Video Magic Studio menu as a new message.
pub async fn send_studio_menu_new_msg(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.clear(user_id);
    log_actor_id!("studio", trace_id, user_id, "rearm" => "studio_menu");

    let text = apply_premium_to_md(&t("studio.title"));
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(studio_keyboard()))
        .build();

    let _ = api.send_message(&params).await;
}

/// Enters the Video Trim & Edit prompt, setting `FlowState::AwaitingStudioTrimVideo`.
pub async fn enter_trim_prompt(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingStudioTrimVideo);
    log_actor_id!("studio_trim", trace_id, user_id, "clicked" => CB_STUDIO_TRIM);

    let text = apply_premium_to_md(&t("studio.trim.send_video_prompt"));
    let kb = InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.trim.cancel_btn"),
            CB_STUDIO_TRIM_CANCEL,
            "cancel",
        )]])
        .build();

    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(kb)
        .build();

    let _ = api.edit_message_text(&params).await;
}

/// Handles Studio callback queries.
pub async fn handle_callback(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    cb_data: &str,
    flow_manager: &FlowManager,
) -> bool {
    let trace_id = next_trace_id();
    log_ev!("studio", trace_id, "callback", "cb" => cb_data, "user_id" => user_id);

    if cb_data == CB_START_STUDIO || cb_data == CB_STUDIO_PANEL {
        enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_COMPRESS {
        compress::enter_compress_prompt(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data.starts_with("stc:") {
        compress::handle_compress_cb(api, chat_id, message_id, user_id, cb_data, flow_manager).await
    } else if cb_data == CB_STUDIO_TRIM {
        enter_trim_prompt(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_TRIM_CANCEL {
        log_ev!("studio_trim", trace_id, "cancel_flow", "user_id" => user_id);
        enter_studio(api, chat_id, message_id, user_id, flow_manager).await;
        true
    } else if cb_data == CB_STUDIO_TRIM_JOBCANCEL {
        log_ev!("studio_trim", trace_id, "job_cancel_clicked", "user_id" => user_id);
        let cancelled = pipeline::cancel_active_job(user_id);
        log_ev!("studio_trim", trace_id, "job_cancel_result", "cancelled" => cancelled);
        true
    } else if cb_data == crate::bot::constants::CB_STUDIO_TRIM_START || cb_data.starts_with("studio_trim:start") {
        log_ev!("studio_trim", trace_id, "start_ranges_prompt", "user_id" => user_id);
        let text = apply_premium_to_md(&t("studio.trim.ranges_prompt"));
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(trim::cancel_keyboard())
            .build();
        if let Err(e) = api.edit_message_text(&params).await {
            log_ev!("studio_trim", trace_id, "start_ranges_prompt_failed", "=>" => format!("fail err={e}"));
        }
        true
    } else {
        false
    }
}
