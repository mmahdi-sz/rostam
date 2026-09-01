use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{EditMessageTextParams, SendMessageParams, SendPhotoParams},
    types::{ChatId, InlineKeyboardMarkup, Message},
};

use super::engine::run_deoldify_colorize;
use crate::bot::CB_DEOLDIFY_CANCEL;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::log::next_trace_id;
use crate::rank;
use crate::rank::quota::{QuotaKind, refund_usage, reserve_usage};
use crate::stats;

use crate::common::job::JobRegistry;

/// Cancel flag per user so the "Cancel" button on status message works.
static ACTIVE_DEOLDIFY_JOBS: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

pub fn cancel_deoldify_job(user_id: i64) -> bool {
    ACTIVE_DEOLDIFY_JOBS.cancel(&user_id)
}

pub fn deoldify_cancel_keyboard() -> InlineKeyboardMarkup {
    crate::common::job_cancel_keyboard(&t("deoldify.cancel_button"), CB_DEOLDIFY_CANCEL, "cancel")
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
    if crate::common::CpuBrokerGuard::is_user_busy(user_id).await {
        let _ = crate::bot::send_text(api, message.chat.id, &t("active_job_running")).await;
        return;
    }

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

    // Quota is reserved upfront before execution (single statement, zero race window).
    let mut reserved = false;
    if let Some(db) = &database {
        let (user_rank, reserve_res) = {
            let client = match db.get().await {
                Ok(c) => c,
                Err(e) => {
                    log_ev!("deoldify", trace_id, "quota_checkout", "err" => format!("{e}"), "=>" => "fail");
                    crate::rank::paywall::quota_db_error(api, chat_id, "deoldify", &format!("{e}"))
                        .await;
                    flow_manager.set(user_id, FlowState::Idle);
                    return;
                }
            };
            let user_rank = rank::effective_rank(&client, user_id).await;
            let limit = user_rank.deoldify_weekly_quota();
            let res = reserve_usage(
                &client,
                user_id,
                QuotaKind::DeoldifyWeekly,
                1,
                7 * 86400,
                limit as i64,
            )
            .await;
            (user_rank, res)
        };
        let limit = user_rank.deoldify_weekly_quota();
        match reserve_res {
            Ok(Some(used_after)) => {
                reserved = true;
                log_ev!("deoldify", trace_id, "quota_reserved", "used" => used_after, "limit" => limit);
            }
            Ok(None) => {
                log_ev!("deoldify", trace_id, "quota_check", "limit" => limit, "=>" => "blocked");
                let label = tf(
                    "deoldify.quota_weekly_limit",
                    &[("limit", &limit.to_string())],
                );
                crate::rank::paywall::block_limit(
                    api,
                    chat_id,
                    &label,
                    crate::rank::types::Rank::Sohrab,
                )
                .await;
                flow_manager.set(user_id, FlowState::Idle);
                return;
            }
            Err(e) => {
                // fail closed — notify user on DB error
                log_ev!("deoldify", trace_id, "quota_reserve", "err" => format!("{e}"), "=>" => "fail");
                crate::rank::paywall::quota_db_error(api, chat_id, "deoldify", &format!("{e}"))
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
                if let Some(db) = &database {
                    log_ev!("deoldify", trace_id, "quota_refund", "why" => $why);
                    if let Ok(client) = db.get().await {
                        if let Err(e) =
                            refund_usage(&client, user_id, QuotaKind::DeoldifyWeekly, 1, 7 * 86400)
                                .await
                        {
                            log_ev!("deoldify", trace_id, "quota_refund", "err" => format!("{e}"), "=>" => "fail");
                            stats::record_error_global("deoldify", "quota_refund_failed").await;
                        }
                    }
                }
            }
        };
    }

    let prep_text = tf("deoldify.preparing", &[("elapsed", "00:00")]);
    let formatted_prep = apply_premium_to_md(&prep_text);

    let status_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&formatted_prep)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
                    deoldify_cancel_keyboard(),
                ))
                .build(),
        )
        .await
    {
        Ok(res) => res.result,
        Err(e) => {
            log_ev!("deoldify", trace_id, "status_msg_failed", "err" => format!("{e:?}"));
            refund!("status_msg_failed");
            return;
        }
    };

    // Cancel flag + elapsed time ticker on status message
    let cancel_flag = ACTIVE_DEOLDIFY_JOBS.register(user_id);
    let _job_guard = ACTIVE_DEOLDIFY_JOBS.guard(user_id);
    let ticker_handle = crate::common::ProgressTicker::new(api, chat_id, status_msg.message_id)
        .with_cancel_flag(cancel_flag.clone())
        .with_keyboard(deoldify_cancel_keyboard())
        .spawn(|elapsed| {
            let secs = elapsed.as_secs();
            let text = tf(
                "deoldify.preparing",
                &[("elapsed", &format!("{:02}:{:02}", secs / 60, secs % 60))],
            );
            Some(apply_premium_to_md(&text))
        });

    let work_dir = format!("downloads/deoldify_{user_id}_{trace_id}");
    let _ = std::fs::create_dir_all(&work_dir);

    let input_path = PathBuf::from(format!("{work_dir}/input.jpg"));
    let output_path = PathBuf::from(format!("{work_dir}/output.jpg"));

    let stats_job_id = stats::record_download_start(user_id, "deoldify").await;

    // Download photo from Telegram
    let dl_result =
        match crate::bot::files::download_telegram_file(api, &file_id, &input_path).await {
            Ok(res) => res,
            Err(e) => {
                ticker_handle.stop();
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

    // Run DeOldify Colorizer
    let process_res = run_deoldify_colorize(&input_path, &output_path, 24, user_id, trace_id).await;
    ticker_handle.stop();

    // User clicked cancel mid-job: discard result and refund quota.
    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("deoldify", trace_id, "cancelled_mid_job", "user_id" => user_id);
        let _ = std::fs::remove_dir_all(&work_dir);
        refund!("cancelled_mid_job");
        return;
    }

    match process_res {
        Ok(duration) => {
            let proc_str = crate::i18n::md_escape(&format!("{:.1}", duration.as_secs_f32()));
            let report_text = tf("deoldify.report", &[("processing", &proc_str)]);
            let result_caption = format!("{}\n\n{}", t("deoldify.result_caption"), report_text);
            let formatted_caption = apply_premium_to_md(&result_caption);

            let photo_params = SendPhotoParams::builder()
                .chat_id(ChatId::Integer(chat_id))
                .photo(FileUpload::InputFile(InputFile {
                    path: output_path.clone(),
                }))
                .caption(&formatted_caption)
                .parse_mode(ParseMode::MarkdownV2)
                .build();

            let out_bytes = std::fs::metadata(&output_path)
                .map(|m| m.len())
                .unwrap_or(0);
            let up_start = std::time::Instant::now();

            use crate::bot::send_file_with_upload_ticker;
            let send_res = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
                api,
                "sendPhoto",
                &photo_params,
                &output_path,
                chat_id,
                status_msg.message_id,
                "transfer.stage.sending_photo",
                None,
            )
            .await;

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

                    // Quota was deducted during reservation; no secondary charge here
                    stats::record_event_user(user_id, "deoldify", "colorize", "ok", 1).await;
                    log_ev!("deoldify", trace_id, "done", "status" => "ok");
                }
                Err(e) => {
                    let err_str = format!("{e:?}");
                    stats::record_error_global("deoldify", "send_photo_failed").await;
                    log_ev!("deoldify", trace_id, "done", "status" => "fail", "err" => &err_str);
                    refund!("send_photo_failed");
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
            refund!("process_failed");
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

    // Re-arm flow so user can send next image immediately
    flow_manager.set(user_id, FlowState::AwaitingDeoldifyImage);
    let prompt = apply_premium_to_md(&t("deoldify.prompt"));
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&prompt)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
                    deoldify_cancel_keyboard(),
                ))
                .build(),
        )
        .await;
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

    // Cancel active job as well as flow state
    cancel_deoldify_job(user_id);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deoldify_cancel_lifecycle() {
        let user_id = 999_888_003;
        let flag = ACTIVE_DEOLDIFY_JOBS.register(user_id);
        assert!(ACTIVE_DEOLDIFY_JOBS.is_active(&user_id));
        assert!(!flag.load(Ordering::SeqCst));

        // simulate cancel
        let cancelled = cancel_deoldify_job(user_id);
        assert!(cancelled);
        assert!(flag.load(Ordering::SeqCst));
        assert!(!ACTIVE_DEOLDIFY_JOBS.is_active(&user_id));

        // guard drop unregister test
        let user_id_2 = 999_888_004;
        let (flag2, _guard) = ACTIVE_DEOLDIFY_JOBS.register_with_guard(user_id_2);
        assert!(ACTIVE_DEOLDIFY_JOBS.is_active(&user_id_2));
        assert!(!flag2.load(Ordering::SeqCst));
        drop(_guard);
        assert!(!ACTIVE_DEOLDIFY_JOBS.is_active(&user_id_2));
    }
}
