use std::time::{Duration, Instant};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{
        DeleteMessageParams, EditMessageTextParams, SendAudioParams, SendMessageParams,
        SendVoiceParams,
    },
    types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup},
};

use tokio::sync::mpsc;

use super::engine::{ProgressSnapshot, run_tts_engine};
use crate::bot::CB_TTS_CANCEL;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::log::next_trace_id;
use crate::rank;
use crate::rank::quota::{QuotaKind, get_usage, refund_usage, reserve_usage};
use crate::stats;

pub fn tts_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            InlineKeyboardButton::builder()
                .text(&t("tts.cancel_button"))
                .callback_data(CB_TTS_CANCEL)
                .build(),
        ]])
        .build()
}

pub async fn enter_tts(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    database: Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_ev!("tts", trace_id, "enter_tts", "user_id" => user_id, "chat_id" => chat_id);

    if let Some(db) = &database {
        let user_rank = rank::effective_rank(db.client(), user_id).await;
        let limit = user_rank.tts_weekly_secs();
        let used = get_usage(db.client(), user_id, QuotaKind::TtsWeekly, 7 * 86400)
            .await
            .unwrap_or(0) as u64;

        if used >= limit {
            log_ev!("tts", trace_id, "quota_check", "used" => used, "limit" => limit, "=>" => "blocked");
            let label = tf(
                "tts.quota_weekly_limit",
                &[("limit", &format!("{}m", limit / 60))],
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
    }

    flow_manager.set(user_id, FlowState::AwaitingTtsText);

    let text = apply_premium_to_md(&t("tts.enter_text_default"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(tts_cancel_keyboard())
        .build();

    let res = api.edit_message_text(&params).await;
    if let Err(e) = res {
        log_ev!("tts", trace_id, "edit_message_failed", "err" => format!("{e:?}"));
    }
}

pub async fn handle_tts_text(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    text_input: &str,
    flow_manager: &FlowManager,
    database: Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_ev!("tts", trace_id, "generate", "user_id" => user_id, "chat_id" => chat_id);

    let est_secs = std::cmp::max(1, (text_input.chars().count() as i64) / 12);

    // رزرو پیش از کار بر پایه‌ی برآورد طول گفتار. سقف اینجا روی مقدارِ
    // «بعد از افزودن» اعمال می‌شود، نه `used >= limit` قبلی — پس متنی که از
    // باقی‌ماندهٔ سهمیه بلندتر باشد هم رد می‌شود.
    let mut reserved = false;
    if let Some(db) = &database {
        let user_rank = rank::effective_rank(db.client(), user_id).await;
        let limit = user_rank.tts_weekly_secs();
        match reserve_usage(
            db.client(),
            user_id,
            QuotaKind::TtsWeekly,
            est_secs,
            7 * 86400,
            limit as i64,
        )
        .await
        {
            Ok(Some(used_after)) => {
                reserved = true;
                log_ev!("tts", trace_id, "quota_reserved", "used" => used_after, "limit" => limit, "est" => est_secs);
            }
            Ok(None) => {
                log_ev!("tts", trace_id, "quota_check", "limit" => limit, "est" => est_secs, "=>" => "blocked");
                let label = tf(
                    "tts.quota_weekly_limit",
                    &[("limit", &format!("{}m", limit / 60))],
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
                // fail closed — تصمیم کاربر: در خطای دیتابیس کاربر مطلع شود.
                log_ev!("tts", trace_id, "quota_reserve", "err" => format!("{e}"), "=>" => "fail");
                crate::rank::paywall::quota_db_error(api, chat_id, "tts", &format!("{e}")).await;
                flow_manager.set(user_id, FlowState::Idle);
                return;
            }
        }
    }

    // برگرداندن سهمیه‌ی رزروشده وقتی کار به نتیجه نرسید.
    macro_rules! refund {
        ($why:expr) => {
            if reserved {
                if let Some(db) = &database {
                    log_ev!("tts", trace_id, "quota_refund", "why" => $why);
                    if let Err(e) = refund_usage(
                        db.client(),
                        user_id,
                        QuotaKind::TtsWeekly,
                        est_secs,
                        7 * 86400,
                    )
                    .await
                    {
                        log_ev!("tts", trace_id, "quota_refund", "err" => format!("{e}"), "=>" => "fail");
                        stats::record_error_global("tts", "quota_refund_failed").await;
                    }
                }
            }
        };
    }

    // Initial status message
    let prep_text = apply_premium_to_md(&t("tts.preparing"));
    let status_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&prep_text)
                .parse_mode(ParseMode::MarkdownV2)
                .build(),
        )
        .await
    {
        Ok(res) => res.result,
        Err(e) => {
            log_ev!("tts", trace_id, "send_status_failed", "err" => format!("{e:?}"));
            refund!("send_status_failed");
            return;
        }
    };

    let status_msg_id = status_msg.message_id;
    let (tx, mut rx) = mpsc::channel::<ProgressSnapshot>(32);

    let text_clone = text_input.to_string();

    // Spawn TTS Engine Task
    let engine_handle =
        tokio::spawn(async move { run_tts_engine(&text_clone, user_id, trace_id, tx).await });

    let mut last_edit = Instant::now();

    while let Some(snap) = rx.recv().await {
        if last_edit.elapsed() >= Duration::from_millis(1500) || snap.percent >= 100.0 {
            let progress_text = tf(
                "tts.progress_body",
                &[
                    ("bar", &snap.bar),
                    ("percent", &format!("{:.0}", snap.percent)),
                    ("elapsed", &snap.elapsed_str),
                    ("eta", &snap.eta_str),
                ],
            );
            let formatted = apply_premium_to_md(&progress_text);

            let edit_params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .text(&formatted)
                .parse_mode(ParseMode::MarkdownV2)
                .build();

            let _ = api.edit_message_text(&edit_params).await;
            last_edit = Instant::now();
        }
    }

    let engine_res = match engine_handle.await {
        Ok(res) => res,
        Err(e) => Err(format!("Join error: {e}")),
    };

    // Delete progress message
    let del_params = DeleteMessageParams::builder()
        .chat_id(chat_id)
        .message_id(status_msg_id)
        .build();
    let _ = api.delete_message(&del_params).await;

    match engine_res {
        Ok(voice_path) => {
            let result_caption = apply_premium_to_md(&t("tts.result_caption"));
            let is_ogg = voice_path.extension().map_or(false, |ext| ext == "ogg");

            let mut is_forbidden = false;
            let mut send_ok = false;
            if is_ogg {
                let voice_params = SendVoiceParams::builder()
                    .chat_id(ChatId::Integer(chat_id))
                    .voice(FileUpload::InputFile(InputFile {
                        path: voice_path.clone(),
                    }))
                    .caption(&result_caption)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build();

                let r = api.send_voice(&voice_params).await;
                if let Err(e) = &r {
                    let err_str = format!("{e:?}");
                    log_ev!("tts", trace_id, "send_voice_failed", "err" => &err_str);
                    if err_str.contains("VOICE_MESSAGES_FORBIDDEN") || err_str.contains("FORBIDDEN")
                    {
                        is_forbidden = true;
                    }
                }
                send_ok = r.is_ok();
            }

            // Fallback to send_audio if it's not ogg or if send_voice failed (e.g. VOICE_MESSAGES_FORBIDDEN)
            if !send_ok {
                let audio_params = SendAudioParams::builder()
                    .chat_id(ChatId::Integer(chat_id))
                    .audio(FileUpload::InputFile(InputFile {
                        path: voice_path.clone(),
                    }))
                    .caption(&result_caption)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build();
                let r = api.send_audio(&audio_params).await;
                if let Err(e) = &r {
                    let err_str = format!("{e:?}");
                    log_ev!("tts", trace_id, "send_audio_failed", "err" => &err_str);
                    if err_str.contains("FORBIDDEN") {
                        is_forbidden = true;
                    }
                }
                send_ok = r.is_ok();
            }

            let _ = std::fs::remove_file(&voice_path);

            if send_ok {
                // سهمیه هنگام رزرو کسر شد؛ بدهکاری دومی اینجا نیست.
                stats::record_event_user(user_id, "tts", "generate", "ok", text_input.len() as i64)
                    .await;
                log_ev!("tts", trace_id, "done", "status" => "ok");

                flow_manager.set(user_id, FlowState::AwaitingTtsText);

                let prompt_text = apply_premium_to_md(&t("tts.enter_text_default"));

                let prompt_params = SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(&prompt_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(ReplyMarkup::InlineKeyboardMarkup(tts_cancel_keyboard()))
                    .build();
                let _ = api.send_message(&prompt_params).await;
            } else {
                stats::record_error_global("tts", "send_voice_failed").await;
                log_ev!("tts", trace_id, "done", "status" => "fail");
                refund!("send_failed");

                let err_key = if is_forbidden {
                    "tts.voice_forbidden_error"
                } else {
                    "tts.process_failed"
                };

                let err_text = apply_premium_to_md(&t(err_key));
                let err_params = SendMessageParams::builder()
                    .chat_id(chat_id)
                    .text(&err_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
                        crate::bot::ai_lab_keyboard(),
                    ))
                    .build();
                let _ = api.send_message(&err_params).await;
            }
        }
        Err(err_msg) => {
            stats::record_error_global("tts", &err_msg).await;
            log_ev!("tts", trace_id, "done", "status" => "engine_err", "err" => &err_msg);
            refund!("engine_err");

            let err_text = apply_premium_to_md(&t("tts.process_failed"));
            let err_params = SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&err_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(
                    crate::bot::ai_lab_keyboard(),
                ))
                .build();
            let _ = api.send_message(&err_params).await;
        }
    }
}

pub async fn handle_tts_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("tts", trace_id, "cancelled", "user_id" => user_id);

    flow_manager.set(user_id, FlowState::Idle);

    let text = apply_premium_to_md(&t("tts.cancelled"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(crate::bot::ai_lab_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}
