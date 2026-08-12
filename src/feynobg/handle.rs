use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{InlineKeyboardButton, InlineKeyboardMarkup, Message, ReplyMarkup},
};

use super::engine::run_nobg;
use crate::bot::{CB_NOBG_CANCEL, edit_to_ai_lab, send_text};
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf, to_fa_digits};
use crate::log::next_trace_id;
use crate::rank::{
    self,
    quota::{QuotaKind, refund_usage, reserve_usage},
};
use crate::stats;

pub fn nobg_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            InlineKeyboardButton::builder()
                .text(&t("nobg.cancel_button"))
                .callback_data(CB_NOBG_CANCEL)
                .build(),
        ]])
        .build()
}

pub async fn enter_nobg(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("feynobg", trace_id, user_id, "clicked" => "enter_nobg");

    flow_manager.set(user_id, FlowState::AwaitingNobgImage);

    let text = apply_premium_to_md(&t("nobg.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(nobg_cancel_keyboard())
        .build();

    let res = api.edit_message_text(&params).await;
    if let Err(e) = res {
        log_ev!("feynobg", trace_id, "edit_message_failed", "err" => format!("{e:?}"));
    }
}

pub async fn handle_nobg_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("feynobg", trace_id, user_id, "clicked" => "nobg_cancel");

    flow_manager.set(user_id, FlowState::Idle);

    if let Err(e) = edit_to_ai_lab(api, chat_id, message_id).await {
        log_ev!("feynobg", trace_id, "cancel_edit_failed", "err" => format!("{e:?}"));
    }
}

pub async fn handle_nobg_image(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
    database: Option<PostgresDatabase>,
) {
    if crate::moebius::cpu::is_user_cpu_busy(user_id).await {
        let _ = crate::bot::send_text(api, message.chat.id, &t("active_job_running")).await;
        return;
    }

    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("feynobg", trace_id, user_id, "clicked" => "send_nobg_image");

    let file_id = if let Some(photos) = &message.photo {
        photos.last().map(|p| p.file_id.clone())
    } else if let Some(doc) = &message.document {
        Some(doc.file_id.clone())
    } else {
        None
    };

    let Some(file_id) = file_id else {
        let text = apply_premium_to_md(&t("nobg.unsupported_format"));
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

    // ── Reserve weekly rank quota (atomic: check + debit in one statement) ──
    let mut reserved = false;
    if let Some(db) = database.as_ref() {
        let user_rank = rank::effective_rank(db.client(), user_id).await;
        let limit = user_rank.nobg_weekly_quota();
        match reserve_usage(
            db.client(),
            user_id,
            QuotaKind::NobgWeekly,
            1,
            7 * 86400,
            limit as i64,
        )
        .await
        {
            Ok(Some(used_after)) => {
                reserved = true;
                log_ev!("feynobg", trace_id, "quota_reserved", "used" => used_after, "limit" => limit);
            }
            Ok(None) => {
                log_ev!("feynobg", trace_id, "quota_check", "limit" => limit, "=>" => "blocked");
                let label = tf("nobg.quota_weekly_limit", &[("limit", &limit.to_string())]);
                if let Some(min_rank) = user_rank.nobg_next_rank() {
                    crate::rank::paywall::block_limit(api, chat_id, &label, min_rank).await;
                } else {
                    let text = apply_premium_to_md(&label);
                    let _ = api
                        .send_message(
                            &SendMessageParams::builder()
                                .chat_id(chat_id)
                                .text(&text)
                                .parse_mode(ParseMode::MarkdownV2)
                                .build(),
                        )
                        .await;
                }
                flow_manager.set(user_id, FlowState::Idle);
                return;
            }
            Err(e) => {
                // fail closed — notify user on DB error
                log_ev!("feynobg", trace_id, "quota_reserve", "err" => format!("{e}"), "=>" => "fail");
                crate::rank::paywall::quota_db_error(api, chat_id, "feynobg", &format!("{e}"))
                    .await;
                flow_manager.set(user_id, FlowState::Idle);
                return;
            }
        }
    }

    // Refund reserved quota on failure
    macro_rules! refund {
        ($why:expr) => {
            if reserved {
                if let Some(db) = database.as_ref() {
                    log_ev!("feynobg", trace_id, "quota_refund", "why" => $why);
                    if let Err(e) =
                        refund_usage(db.client(), user_id, QuotaKind::NobgWeekly, 1, 7 * 86400)
                            .await
                    {
                        log_ev!("feynobg", trace_id, "quota_refund", "err" => format!("{e}"), "=>" => "fail");
                        stats::record_error_global("feynobg", "quota_refund_failed").await;
                    }
                }
            }
        };
    }

    // Reset user flow state to Idle as image processing starts
    flow_manager.set(user_id, FlowState::Idle);

    // Send status message
    let status_text = apply_premium_to_md(&t("nobg.preparing"));
    let status_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&status_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(nobg_cancel_keyboard()))
                .build(),
        )
        .await
    {
        Ok(res) => Some(res.result),
        Err(e) => {
            log_ev!("feynobg", trace_id, "send_status_failed", "err" => format!("{e:?}"));
            None
        }
    };

    // Prepare temp work directory
    let temp_dir = PathBuf::from(format!("downloads/{user_id}/nobg_{trace_id}"));
    if let Err(e) = tokio::fs::create_dir_all(&temp_dir).await {
        log_ev!("feynobg", trace_id, "create_dir_failed", "err" => format!("{e:?}"));
        if let Some(msg) = status_msg {
            let _ = api
                .delete_message(
                    &DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(msg.message_id)
                        .build(),
                )
                .await;
        }
        let text = apply_premium_to_md(&t("nobg.process_failed"));
        let _ = api
            .send_message(
                &SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(&text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build(),
            )
            .await;
        refund!("create_dir_failed");
        return;
    }

    let is_doc = message.document.is_some();
    let ext = if is_doc { "png" } else { "jpg" };
    let input_path = temp_dir.join(format!("input.{ext}"));
    let output_path = temp_dir.join("output.png");

    let stats_job_id = stats::record_download_start(user_id, "feynobg").await;

    // Download image from Telegram
    log_ev!("feynobg", trace_id, "downloading", "file_id" => &file_id);
    let dl_result = match crate::bot::download_telegram_file(api, &file_id, &input_path).await {
        Ok(res) => res,
        Err(e) => {
            log_ev!("feynobg", trace_id, "download_failed", "err" => format!("{e:?}"));
            if let Some(msg) = status_msg {
                let _ = api
                    .delete_message(
                        &DeleteMessageParams::builder()
                            .chat_id(chat_id)
                            .message_id(msg.message_id)
                            .build(),
                    )
                    .await;
            }
            let text = apply_premium_to_md(&t("nobg.download_failed"));
            let _ = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .build(),
                )
                .await;
            let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            refund!("download_failed");
            return;
        }
    };

    if let Some(jid) = stats_job_id {
        stats::record_download_done(
            jid,
            dl_result.bytes as i64,
            None,
            None,
            Some(dl_result.speed_bps() as i64),
        )
        .await;
    }

    // Run FeyNobg ONNX model
    log_ev!("feynobg", trace_id, "process_start");
    let result = run_nobg(&input_path, &output_path, user_id, trace_id).await;

    match result {
        Ok(duration) => {
            let sec_str = md_escape(&to_fa_digits(&format!("{:.1}", duration.as_secs_f32())));
            let caption_raw = tf("nobg.result_caption", &[("time", &sec_str)]);
            let caption = apply_premium_to_md(&caption_raw);

            let out_bytes = std::fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
            let up_start = std::time::Instant::now();

            let doc_params = SendDocumentParams::builder()
                .chat_id(chat_id)
                .document(output_path.clone())
                .caption(&caption)
                .parse_mode(ParseMode::MarkdownV2)
                .build();
            use crate::bot::send_file_with_upload_ticker;
            let status_mid = status_msg.as_ref().map(|m| m.message_id).unwrap_or(0);
            let send_res = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
                api,
                "sendDocument",
                &doc_params,
                std::path::Path::new(&output_path),
                chat_id,
                status_mid,
                "transfer.stage.sending_document",
                None,
            ).await;

            if let Err(e) = send_res {
                log_ev!("feynobg", trace_id, "send_document_failed", "err" => format!("{e:?}"));
                stats::record_error_global("feynobg", &format!("send_document_failed: {e}")).await;
                stats::record_event_user(user_id, "nobg", "process", "fail", 1).await;
                let _ = send_text(api, chat_id, &t("nobg.process_failed")).await;
                refund!("send_document_failed");
            } else {
                let up_elapsed = up_start.elapsed();
                let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
                    out_bytes as f64 / up_elapsed.as_secs_f64()
                } else {
                    0.0
                };
                if let Some(jid) = stats_job_id {
                    stats::record_upload_done(
                        jid,
                        user_id,
                        out_bytes as i64,
                        Some(up_speed as i64),
                        Some(1),
                    )
                    .await;
                }

                log_ev!("feynobg", trace_id, "success", "duration" => sec_str);
                stats::record_event_user(user_id, "nobg", "process", "ok", 1).await;

                // Quota was deducted during reservation; no secondary charge here

                // UX Improvement: Show prompt menu again so user can immediately send another photo
                let prompt_text = apply_premium_to_md(&t("nobg.prompt"));
                let _ = api
                    .send_message(
                        &SendMessageParams::builder()
                            .chat_id(chat_id)
                            .text(&prompt_text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(nobg_cancel_keyboard()))
                            .build(),
                    )
                    .await;
                flow_manager.set(user_id, FlowState::AwaitingNobgImage);
            }
        }
        Err(e) => {
            log_ev!("feynobg", trace_id, "nobg_failed", "err" => &e);
            stats::record_error_global("feynobg", &e).await;
            stats::record_event_user(user_id, "nobg", "process", "fail", 1).await;
            refund!("nobg_failed");

            let text = apply_premium_to_md(&t("nobg.process_failed"));
            let _ = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .build(),
                )
                .await;
        }
    }

    // Clean up status message
    if let Some(msg) = status_msg {
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(msg.message_id)
                    .build(),
            )
            .await;
    }

    // Clean up temp dir
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}
