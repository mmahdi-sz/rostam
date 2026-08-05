use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{EditMessageTextParams, SendMessageParams, SendPhotoParams},
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};

use super::engine::run_deoldify_colorize;
use crate::emoji::{FlowManager, FlowState};
use crate::bot::CB_DEOLDIFY_CANCEL;
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::log::next_trace_id;
use crate::stats;
use crate::rank;
use crate::rank::quota::{get_usage, add_usage, QuotaKind};
use crate::database::postgresql::PostgresDatabase;

pub fn deoldify_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![InlineKeyboardButton::builder()
            .text(&t("deoldify.cancel_button"))
            .callback_data(CB_DEOLDIFY_CANCEL)
            .build()]])
        .build()
}

pub async fn enter_deoldify(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("deoldify", trace_id, "enter", "user_id" => user_id, "chat_id" => chat_id);

    flow_manager.set(user_id, FlowState::AwaitingDeoldifyImage);

    let text = apply_premium_to_md(&t("deoldify.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(deoldify_cancel_keyboard())
        .build();

    let res = api.edit_message_text(&params).await;
    if let Err(e) = res {
        log_ev!("deoldify", trace_id, "edit_message_failed", "err" => format!("{e:?}"));
    }
}

pub async fn handle_deoldify_image(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
    database: Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_ev!("deoldify", trace_id, "handle_image", "user_id" => user_id, "chat_id" => chat_id);

    let file_id = if let Some(photos) = &message.photo {
        photos.last().map(|p| p.file_id.clone())
    } else if let Some(doc) = &message.document {
        Some(doc.file_id.clone())
    } else {
        None
    };

    let Some(file_id) = file_id else {
        let text = apply_premium_to_md(&t("deoldify.unsupported_format"));
        let _ = api
            .send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(&text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build(),
            )
            .await;
        return;
    };

    if let Some(db) = &database {
        let user_rank = rank::effective_rank(db.client(), user_id).await;
        let limit = user_rank.deoldify_weekly_quota();
        let used = get_usage(db.client(), user_id, QuotaKind::DeoldifyWeekly, 7 * 86400)
            .await
            .unwrap_or(0) as u32;

        if used >= limit {
            log_ev!("deoldify", trace_id, "quota_check", "used" => used, "limit" => limit, "=>" => "blocked");
            let label = tf(
                "deoldify.quota_weekly_limit",
                &[("limit", &limit.to_string())],
            );
            crate::rank::paywall::block_limit(api, chat_id, &label, crate::rank::types::Rank::Sohrab).await;
            flow_manager.set(user_id, FlowState::Idle);
            return;
        }
    }

    let prep_text = tf("deoldify.preparing", &[("elapsed", "00:00")]);
    let formatted_prep = apply_premium_to_md(&prep_text);

    let status_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&formatted_prep)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await
    {
        Ok(res) => res.result,
        Err(e) => {
            log_ev!("deoldify", trace_id, "status_msg_failed", "err" => format!("{e:?}"));
            return;
        }
    };
    let _ = status_msg;

    let work_dir = format!("downloads/deoldify_{}_{}", user_id, trace_id);
    let _ = std::fs::create_dir_all(&work_dir);

    let input_path = PathBuf::from(format!("{work_dir}/input.jpg"));
    let output_path = PathBuf::from(format!("{work_dir}/output.jpg"));

    // Download photo from Telegram
    if let Err(e) = crate::bot::files::download_telegram_file(api, &file_id, &input_path).await {
        log_ev!("deoldify", trace_id, "download_failed", "err" => format!("{e:?}"));
        let _ = api
            .delete_message(
                &frankenstein::methods::DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg.message_id)
                    .build(),
            )
            .await;
        let text = apply_premium_to_md(&t("deoldify.download_failed"));
        let _ = api
            .send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(&text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build(),
            )
            .await;
        let _ = std::fs::remove_dir_all(&work_dir);
        return;
    }

    // Run DeOldify Colorizer
    let process_res = run_deoldify_colorize(&input_path, &output_path, 24, user_id, trace_id).await;

    match process_res {
        Ok(duration) => {
            let proc_str = crate::i18n::md_escape(&format!("{:.1}", duration.as_secs_f32()));
            let report_text = tf(
                "deoldify.report",
                &[("processing", &proc_str)],
            );
            let result_caption = format!(
                "{}\n\n{}",
                t("deoldify.result_caption"),
                report_text
            );
            let formatted_caption = apply_premium_to_md(&result_caption);

            let photo_params = SendPhotoParams::builder()
                .chat_id(ChatId::Integer(chat_id))
                .photo(FileUpload::InputFile(InputFile {
                    path: output_path.clone(),
                }))
                .caption(&formatted_caption)
                .parse_mode(ParseMode::MarkdownV2)
                .build();

            let send_res = api.send_photo(&photo_params).await;

            let _ = api
                .delete_message(
                    &frankenstein::methods::DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg.message_id)
                        .build(),
                )
                .await;

            match send_res {
                Ok(_) => {
                    if let Some(db) = &database {
                        let _ = add_usage(db.client(), user_id, QuotaKind::DeoldifyWeekly, 1, 7 * 86400).await;
                    }
                    stats::record_event_user(user_id, "deoldify", "colorize", "ok", 1).await;
                    log_ev!("deoldify", trace_id, "done", "status" => "ok");
                }
                Err(e) => {
                    let err_str = format!("{e:?}");
                    stats::record_error_global("deoldify", "send_photo_failed").await;
                    log_ev!("deoldify", trace_id, "done", "status" => "fail", "err" => &err_str);
                    let err_text = apply_premium_to_md(&t("deoldify.process_failed"));
                    let _ = api
                        .send_message(
                            &SendMessageParams::builder()
                                .chat_id(chat_id)
                                .text(&err_text)
                                .parse_mode(ParseMode::MarkdownV2)
                                .build(),
                        )
                        .await;
                }
            }
        }
        Err(_) => {
            let _ = api
                .delete_message(
                    &frankenstein::methods::DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg.message_id)
                        .build(),
                )
                .await;
            stats::record_error_global("deoldify", "process_failed").await;
            log_ev!("deoldify", trace_id, "done", "status" => "fail");
            let err_text = apply_premium_to_md(&t("deoldify.process_failed"));
            let _ = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(&err_text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .build(),
                )
                .await;
        }
    }

    let _ = std::fs::remove_dir_all(&work_dir);
}

pub async fn handle_deoldify_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("deoldify", trace_id, "cancelled", "user_id" => user_id);

    flow_manager.set(user_id, FlowState::Idle);

    let text = apply_premium_to_md(&t("deoldify.cancelled"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(crate::bot::ai_lab_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}
