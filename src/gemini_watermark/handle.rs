use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams},
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};

use crate::bot::{edit_to_ai_lab, send_text_with_back};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t};
use crate::log::next_trace_id;

pub const CB_GWM_CANCEL: &str = "gwm:cancel";

fn cancel_keyboard() -> InlineKeyboardMarkup {
    let icon_id = t("emoji.panel.icons.cancel");
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![InlineKeyboardButton {
            text: t("gemini_wm.cancel_button"),
            callback_data: Some(CB_GWM_CANCEL.to_string()),
            style: Some(ButtonStyle::Danger),
            icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') {
                None
            } else {
                Some(icon_id)
            },
            url: None,
            login_url: None,
            web_app: None,
            switch_inline_query: None,
            switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None,
            copy_text: None,
            callback_game: None,
            pay: None,
        }]])
        .build()
}

pub async fn enter_gwm(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingGeminiWmImage);
    log_ev!("gwm", trace_id, "enter", "raw" => format!("user_id={user_id} chat_id={chat_id}"));

    let text = t("gemini_wm.prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(cancel_keyboard())
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => log_ev!("gwm", trace_id, "prompt_shown"),
        Err(e) => log_ev!("gwm", trace_id, "prompt_failed", "raw" => format!("err={e}")),
    }
}

pub async fn handle_gwm_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("gwm", trace_id, "cancel", "raw" => format!("user_id={user_id} chat_id={chat_id}"));
    flow_manager.clear(user_id);
    let r = edit_to_ai_lab(api, chat_id, message_id).await;
    log_ev!("gwm", trace_id, "cancel_done", "raw" => format!("ok={}", r.is_ok()));
}

pub async fn handle_gwm_image(api: &Bot, message: &Message, user_id: i64) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    log_actor_id!("gwm", trace_id, user_id, "clicked" => "photo/doc");
    log_ev!("gwm", trace_id, "image_received", "raw" => format!("user_id={user_id} chat_id={chat_id} has_photo={} has_doc={}",
        message.photo.is_some(), message.document.is_some()));

    // Get file_id and extension from photo (largest) or document.
    let file_id = message
        .photo
        .as_ref()
        .and_then(|photos| photos.last())
        .map(|p| p.file_id.clone())
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));

    let Some(file_id) = file_id else {
        log_ev!("gwm", trace_id, "no_file_id");
        let _ = send_text_with_back(api, chat_id, &t("gemini_wm.error.invalid_image")).await;
        return;
    };

    let ext = detect_ext(message);
    log_ev!("gwm", trace_id, "file_info", "raw" => format!("file_id={file_id} ext={ext}"));

    // Flow state is cleared by the dispatcher before spawning this task.
    let status_msg_id = match api
        .send_message(
            &frankenstein::methods::SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&apply_premium_to_md(&t("gemini_wm.processing")))
                .parse_mode(frankenstein::ParseMode::MarkdownV2)
                .build(),
        )
        .await
    {
        Ok(m) => m.result.message_id,
        Err(_) => 0,
    };

    // Download image.
    log_ev!("gwm", trace_id, "download_start", "raw" => format!("file_id={file_id}"));
    let work_dir = std::env::temp_dir().join(format!("gwm_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();
    let input_path = work_dir.join(format!("input.{ext}"));

    let path_str = match input_path.to_str() {
        Some(s) => s,
        None => {
            log_ev!("gwm", trace_id, "download_failed", "raw" => "invalid_path_encoding");
            crate::stats::record_event_user(user_id, "gwm", "", "fail", 0).await;
            crate::stats::record_error_global("gwm", "invalid_path_encoding").await;
            let _ = send_text_with_back(api, chat_id, &t("gemini_wm.error.download_failed")).await;
            std::fs::remove_dir_all(&work_dir).ok();
            return;
        }
    };
    let stats_job_id = crate::stats::record_download_start(user_id, "gwm").await;

    let dl_result = match download_file(api, &file_id, path_str, trace_id).await {
        Ok(res) => res,
        Err(e) => {
            log_ev!("gwm", trace_id, "download_failed", "raw" => format!("err={e}"));
            crate::stats::record_event_user(user_id, "gwm", "", "fail", 0).await;
            crate::stats::record_error_global("gwm", &format!("download failed: {e}")).await;
            let _ = send_text_with_back(api, chat_id, &t("gemini_wm.error.download_failed")).await;
            std::fs::remove_dir_all(&work_dir).ok();
            return;
        }
    };

    if let Some(jid) = stats_job_id {
        crate::stats::record_download_done(
            jid,
            dl_result.bytes as i64,
            None,
            None,
            Some(dl_result.speed_bps() as i64),
        )
        .await;
    }

    let file_size = std::fs::metadata(&input_path).map(|m| m.len()).unwrap_or(0);
    log_ev!("gwm", trace_id, "download_done", "raw" => format!("size={file_size} speed={}", dl_result.speed_human()));

    let image_bytes = match std::fs::read(&input_path) {
        Ok(b) => b,
        Err(e) => {
            log_ev!("gwm", trace_id, "read_failed", "raw" => format!("err={e}"));
            crate::stats::record_event_user(user_id, "gwm", "", "fail", 0).await;
            crate::stats::record_error_global("gwm", &format!("read failed: {e}")).await;
            let _ =
                send_text_with_back(api, chat_id, &t("gemini_wm.error.processing_failed")).await;
            std::fs::remove_dir_all(&work_dir).ok();
            return;
        }
    };
    std::fs::remove_dir_all(&work_dir).ok();

    // Run watermark removal (Moebius ONNX inpainting pipeline). The sparkle is
    // located dynamically; if it can't be found (and the image isn't in the
    // ≥1024 class where a fixed-corner fallback is trusted), the pipeline
    // returns NoWatermark and we tell the user rather than inpainting a guess.
    log_ev!("gwm", trace_id, "remove_start", "raw" => format!("user_id={user_id} bytes={}", image_bytes.len()));
    let t_start = std::time::Instant::now();
    let result_bytes = match crate::moebius::remove_watermark(image_bytes, user_id, trace_id).await
    {
        Ok(v) => v,
        Err(crate::moebius::MoebiusError::NoWatermark) => {
            let elapsed = t_start.elapsed().as_secs_f64();
            log_ev!("gwm", trace_id, "no_watermark", "raw" => format!("elapsed={elapsed:.2}s"));
            crate::stats::record_event_user(user_id, "gwm", "", "no_watermark", 0).await;
            let _ = send_text_with_back(api, chat_id, &t("gemini_wm.error.no_watermark")).await;
            return;
        }
        Err(e) => {
            let elapsed = t_start.elapsed().as_secs_f64();
            log_ev!("gwm", trace_id, "remove_failed", "raw" => format!("elapsed={elapsed:.2}s err={e}"));
            crate::stats::record_event_user(user_id, "gwm", "", "fail", 0).await;
            crate::stats::record_error_global("gwm", &format!("remove failed: {e}")).await;
            let _ =
                send_text_with_back(api, chat_id, &t("gemini_wm.error.processing_failed")).await;
            return;
        }
    };
    let elapsed = t_start.elapsed().as_secs_f64();
    log_ev!("gwm", trace_id, "remove_done", "elapsed" => format!("{elapsed:.2}s"), "bytes" => result_bytes.len());

    // Moebius always emits a single PNG (fresh synthesis of the masked
    // region), regardless of the input format.
    let out_path = std::env::temp_dir().join(format!("gwm_out_{trace_id}.png"));
    if let Err(e) = std::fs::write(&out_path, &result_bytes) {
        log_ev!("gwm", trace_id, "write_failed", "raw" => format!("err={e}"));
        crate::stats::record_event_user(user_id, "gwm", "", "fail", 0).await;
        crate::stats::record_error_global("gwm", &format!("write failed: {e}")).await;
        let _ = send_text_with_back(api, chat_id, &t("gemini_wm.error.processing_failed")).await;
        return;
    }

    let caption = t("gemini_wm.result.single_caption");
    log_ev!("gwm", trace_id, "sending_result", "bytes" => result_bytes.len(), "caption_len" => caption.chars().count());

    let p = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(PathBuf::from(&out_path))
        .caption(&caption)
        .build();

    let up_start = std::time::Instant::now();
    let out_bytes = result_bytes.len() as u64;

    use crate::bot::send_file_with_upload_ticker;
    match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        api,
        "sendDocument",
        &p,
        std::path::Path::new(&out_path),
        chat_id,
        status_msg_id,
        "transfer.stage.sending_document",
        None,
    ).await {
        Ok(_) => {
            let up_elapsed = up_start.elapsed();
            let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
                out_bytes as f64 / up_elapsed.as_secs_f64()
            } else {
                0.0
            };
            if let Some(jid) = stats_job_id {
                crate::stats::record_upload_done(
                    jid,
                    user_id,
                    out_bytes as i64,
                    Some(up_speed as i64),
                    Some(1),
                )
                .await;
            }

            log_ev!("gwm", trace_id, "result_sent");
            crate::stats::record_event_user(user_id, "gwm", "", "ok", 1).await;
            crate::metrics::get()
                .gwm_requests_total
                .with_label_values(&["success"])
                .inc();
        }

        Err(e) => {
            log_ev!("gwm", trace_id, "result_send_failed", "raw" => format!("err={e}"));
            crate::stats::record_event_user(user_id, "gwm", "", "fail", 0).await;
            crate::metrics::get()
                .gwm_requests_total
                .with_label_values(&["fail"])
                .inc();
            crate::stats::record_error_global("gwm", &format!("send failed: {e}")).await;
        }
    }
    std::fs::remove_file(&out_path).ok();
}

fn detect_ext(message: &Message) -> String {
    if let Some(doc) = &message.document {
        if let Some(name) = &doc.file_name {
            if let Some(ext) = name.rsplit('.').next() {
                return ext.to_lowercase();
            }
        }
        if let Some(mime) = &doc.mime_type {
            return match mime.as_str() {
                "image/jpeg" | "image/jpg" => "jpg",
                "image/png" => "png",
                "image/webp" => "webp",
                "image/bmp" => "bmp",
                _ => "jpg",
            }
            .to_string();
        }
    }
    "jpg".to_string()
}

async fn download_file(
    api: &Bot,
    file_id: &str,
    dest: &str,
    trace_id: u64,
) -> Result<crate::bot::TransferResult, Box<dyn std::error::Error + Send + Sync>> {
    log_ev!("gwm", trace_id, "download_start", "raw" => format!("file_id={file_id}"));
    crate::bot::download_telegram_file(api, file_id, dest).await
}

