use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{
        AnswerCallbackQueryParams, DeleteMessageParams, EditMessageTextParams,
        SendAudioParams,
    },
    types::{InlineKeyboardMarkup, Message},
};

use crate::bot::{send_text, edit_to_ai_lab};
use crate::emoji::{FlowManager, FlowState};
use crate::emoji::panel::{btn_icon_success, btn_icon, btn_icon_danger};
use crate::i18n::t;
use crate::youtube::log_trace;

use super::client::separate_audio;
use super::types::SeparationMode;

static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

fn next_trace_id() -> u64 {
    NEXT_TRACE.fetch_add(1, Ordering::Relaxed)
}

pub const CB_AI_SEP: &str = "ai:sep";
pub const CB_SEP_PREFIX: &str = "sep:";

pub const CB_SEP_BACK: &str = "sep:back";

fn prompt_keyboard(msg_id: i32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(&t("start.back"), &format!("{CB_SEP_BACK}:{msg_id}"), "back")],
        ])
        .build()
}

fn mode_keyboard(msg_id: i32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![
                btn_icon_success(&t("separation.btn.quality"), &format!("sep:quality:{msg_id}"), "quality_high"),
                btn_icon(&t("separation.btn.fast"), &format!("sep:fast:{msg_id}"), "speed_fast"),
            ],
            vec![btn_icon_danger(&t("separation.btn.cancel"), &format!("sep:cancel:{msg_id}"), "cancel")],
        ])
        .build()
}

/// Called when user presses AI Lab → جداسازی صدا.
pub async fn enter_separation(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingSeparation);
    eprintln!("[separation trace={trace_id} event=enter] user_id={user_id} chat_id={chat_id}");

    let text = t("separation.send_audio_prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(prompt_keyboard(message_id))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[separation trace={trace_id} event=prompt_shown]"),
        Err(e) => eprintln!("[separation trace={trace_id} event=prompt_failed] err={e}"),
    }
}

/// Called when user sends audio while in AwaitingSeparation state.
pub async fn handle_separation_audio(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let msg_id = message.message_id;

    eprintln!("[separation trace={trace_id} event=audio_received] user_id={user_id} chat_id={chat_id} msg_id={msg_id} has_audio={} has_voice={} has_doc={}",
        message.audio.is_some(), message.voice.is_some(), message.document.is_some());

    // Keep flow alive — mode hasn't been selected yet.
    // We store the file_id so we can download after mode selection.
    let file_id = message.audio.as_ref().map(|a| a.file_id.clone())
        .or_else(|| message.voice.as_ref().map(|v| v.file_id.clone()))
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));

    let Some(file_id) = file_id else {
        eprintln!("[separation trace={trace_id} event=no_file_id]");
        let _ = send_text(api, chat_id, &t("separation.error.invalid_audio")).await;
        return;
    };

    let orig_filename = message.audio.as_ref().and_then(|a| a.file_name.as_deref())
        .or_else(|| message.document.as_ref().and_then(|d| d.file_name.as_deref()))
        .unwrap_or("audio.mp3")
        .to_string();

    eprintln!("[separation trace={trace_id} event=file_stored] file_id={file_id} filename={orig_filename}");

    // Update flow to store file info, waiting for mode selection.
    flow_manager.set(user_id, FlowState::AwaitingSeparationMode {
        file_id: file_id.clone(),
        filename: orig_filename.clone(),
        prompt_msg_id: None,
    });

    // Send mode selection keyboard as a new message.
    let text = t("separation.select_mode");
    let kb = mode_keyboard(msg_id);
    let params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(kb))
        .build();
    match api.send_message(&params).await {
        Ok(resp) => {
            let prompt_id = resp.result.message_id;
            eprintln!("[separation trace={trace_id} event=mode_keyboard_sent] prompt_msg_id={prompt_id}");
            // Store the prompt message id so we can edit/delete it later.
            flow_manager.set(user_id, FlowState::AwaitingSeparationMode {
                file_id,
                filename: orig_filename,
                prompt_msg_id: Some(prompt_id),
            });
        }
        Err(e) => eprintln!("[separation trace={trace_id} event=mode_keyboard_failed] err={e}"),
    }
}

/// Handles all sep:* callbacks.
pub async fn handle_separation_callback(
    api: &Bot,
    cb_data: &str,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    eprintln!("[separation trace={trace_id} event=callback] user_id={user_id} chat_id={chat_id} data={cb_data}");

    // sep:back:{msg_id} — برگشت به AI Lab از صفحه prompt
    if cb_data.starts_with("sep:back:") {
        flow_manager.clear(user_id);
        let r = edit_to_ai_lab(api, chat_id, message_id).await;
        eprintln!("[separation trace={trace_id} event=back_done] ok={}", r.is_ok());
        return;
    }

    // sep:cancel:{msg_id}
    if let Some(rest) = cb_data.strip_prefix("sep:cancel:") {
        eprintln!("[separation trace={trace_id} event=cancel] msg_id_from_cb={rest}");
        flow_manager.clear(user_id);
        let r = edit_to_ai_lab(api, chat_id, message_id).await;
        eprintln!("[separation trace={trace_id} event=cancel_done] ok={}", r.is_ok());
        return;
    }

    // sep:quality:{orig_msg_id} or sep:fast:{orig_msg_id}
    let (mode, _orig_msg_id) = if let Some(rest) = cb_data.strip_prefix("sep:quality:") {
        (SeparationMode::Quality, rest)
    } else if let Some(rest) = cb_data.strip_prefix("sep:fast:") {
        (SeparationMode::Fast, rest)
    } else {
        eprintln!("[separation trace={trace_id} event=unknown_callback] data={cb_data}");
        return;
    };

    let mode_label = match mode {
        SeparationMode::Quality => "quality",
        SeparationMode::Fast => "fast",
    };
    eprintln!("[separation trace={trace_id} event=mode_selected] user_id={user_id} mode={mode_label}");

    // Read stored file info from flow state.
    let (file_id, filename) = match flow_manager.get(user_id) {
        FlowState::AwaitingSeparationMode { file_id, filename, .. } => (file_id, filename),
        other => {
            eprintln!("[separation trace={trace_id} event=wrong_state] state={other:?}");
            let _ = send_text(api, chat_id, &t("separation.error.service_unavailable")).await;
            return;
        }
    };

    // Clear flow — processing starts.
    flow_manager.clear(user_id);

    // Edit keyboard message to "processing…"
    let processing_text = t("separation.processing");
    let edit_params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&processing_text)
        .build();
    match api.edit_message_text(&edit_params).await {
        Ok(_) => eprintln!("[separation trace={trace_id} event=processing_msg_shown]"),
        Err(e) => eprintln!("[separation trace={trace_id} event=processing_msg_failed] err={e}"),
    }

    // Download audio from Telegram.
    eprintln!("[separation trace={trace_id} event=download_start] file_id={file_id} filename={filename}");
    let audio_bytes = match download_file(api, &file_id, trace_id).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[separation trace={trace_id} event=download_failed] err={e}");
            let _ = send_text(api, chat_id, &t("separation.error.service_unavailable")).await;
            let _ = delete_message(api, chat_id, message_id).await;
            return;
        }
    };
    eprintln!("[separation trace={trace_id} event=download_done] bytes={}", audio_bytes.len());

    // Call separation service.
    eprintln!("[separation trace={trace_id} event=separate_start] mode={mode_label}");
    match separate_audio(audio_bytes, &filename, mode, user_id).await {
        Ok(result) => {
            eprintln!("[separation trace={trace_id} event=separate_done] duration={:.1}s vocals_wav={} instrumental_wav={} vocals_compressed={} instrumental_compressed={} ext={}",
                result.duration_seconds, result.vocals_wav.len(), result.instrumental_wav.len(),
                result.vocals_compressed.len(), result.instrumental_compressed.len(), result.compressed_ext);

            // Delete processing message.
            let _ = delete_message(api, chat_id, message_id).await;

            let tmp_dir = std::env::temp_dir().join(format!("sep_{trace_id}"));
            std::fs::create_dir_all(&tmp_dir).ok();

            let vocals_wav_path = tmp_dir.join("vocals.wav");
            let instrumental_wav_path = tmp_dir.join("instrumental.wav");
            let vocals_compressed_path = tmp_dir.join(format!("vocals.{}", result.compressed_ext));
            let instrumental_compressed_path = tmp_dir.join(format!("instrumental.{}", result.compressed_ext));

            std::fs::write(&vocals_wav_path, &result.vocals_wav).ok();
            std::fs::write(&instrumental_wav_path, &result.instrumental_wav).ok();
            std::fs::write(&vocals_compressed_path, &result.vocals_compressed).ok();
            std::fs::write(&instrumental_compressed_path, &result.instrumental_compressed).ok();

            // 1. vocals compressed (audio — inline playback)
            eprintln!("[separation trace={trace_id} event=send_vocals_compressed]");
            let p = SendAudioParams::builder()
                .chat_id(chat_id)
                .audio(PathBuf::from(&vocals_compressed_path))
                .caption(t("separation.result.vocals_compressed_caption"))
                .build();
            match api.send_audio(&p).await {
                Ok(_) => eprintln!("[separation trace={trace_id} event=vocals_compressed_sent]"),
                Err(e) => eprintln!("[separation trace={trace_id} event=vocals_compressed_failed] err={e}"),
            }

            // 2. vocals WAV (document — for editing)
            eprintln!("[separation trace={trace_id} event=send_vocals_wav]");
            let p = frankenstein::methods::SendDocumentParams::builder()
                .chat_id(chat_id)
                .document(PathBuf::from(&vocals_wav_path))
                .caption(t("separation.result.vocals_wav_caption"))
                .build();
            match api.send_document(&p).await {
                Ok(_) => eprintln!("[separation trace={trace_id} event=vocals_wav_sent]"),
                Err(e) => eprintln!("[separation trace={trace_id} event=vocals_wav_failed] err={e}"),
            }

            // 3. instrumental compressed (audio — inline playback)
            eprintln!("[separation trace={trace_id} event=send_instrumental_compressed]");
            let p = SendAudioParams::builder()
                .chat_id(chat_id)
                .audio(PathBuf::from(&instrumental_compressed_path))
                .caption(t("separation.result.instrumental_compressed_caption"))
                .build();
            match api.send_audio(&p).await {
                Ok(_) => eprintln!("[separation trace={trace_id} event=instrumental_compressed_sent]"),
                Err(e) => eprintln!("[separation trace={trace_id} event=instrumental_compressed_failed] err={e}"),
            }

            // 4. instrumental WAV (document — for editing)
            eprintln!("[separation trace={trace_id} event=send_instrumental_wav]");
            let p = frankenstein::methods::SendDocumentParams::builder()
                .chat_id(chat_id)
                .document(PathBuf::from(&instrumental_wav_path))
                .caption(t("separation.result.instrumental_wav_caption"))
                .build();
            match api.send_document(&p).await {
                Ok(_) => eprintln!("[separation trace={trace_id} event=instrumental_wav_sent]"),
                Err(e) => eprintln!("[separation trace={trace_id} event=instrumental_wav_failed] err={e}"),
            }

            std::fs::remove_dir_all(&tmp_dir).ok();
            eprintln!("[separation trace={trace_id} event=cleanup_done]");
        }
        Err(e) => {
            eprintln!("[separation trace={trace_id} event=separate_error] err={e}");
            let _ = delete_message(api, chat_id, message_id).await;
            use super::error::SeparationError;
            let key = match &e {
                SeparationError::ServiceUnavailable => "separation.error.service_unavailable",
                SeparationError::InvalidAudio => "separation.error.invalid_audio",
                SeparationError::Timeout => "separation.error.timeout",
                SeparationError::ProcessingFailed(_) => "separation.error.processing_failed",
            };
            let _ = send_text(api, chat_id, &t(key)).await;
        }
    }
}

async fn download_file(api: &Bot, file_id: &str, trace_id: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use frankenstein::methods::GetFileParams;

    let file_info = api.get_file(&GetFileParams::builder().file_id(file_id).build()).await?;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    eprintln!("[separation trace={trace_id} event=file_path] file_path={file_path}");

    if file_path.starts_with('/') {
        let bytes = std::fs::read(&file_path)?;
        eprintln!("[separation trace={trace_id} event=local_read] size={}", bytes.len());
        return Ok(bytes);
    }

    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{}/{file_path}", crate::config::bot_token()?)
    } else {
        format!("https://api.telegram.org/file/bot{}/{file_path}", crate::config::bot_token()?)
    };

    eprintln!("[separation trace={trace_id} event=http_download] url_prefix={}", &url[..url.len().min(60)]);
    let response = reqwest::get(&url).await?;
    let status = response.status();
    let bytes = response.bytes().await?.to_vec();
    eprintln!("[separation trace={trace_id} event=http_done] status={status} bytes={}", bytes.len());
    Ok(bytes)
}

async fn delete_message(api: &Bot, chat_id: i64, message_id: i32) -> Result<(), Box<dyn std::error::Error>> {
    let params = DeleteMessageParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .build();
    api.delete_message(&params).await?;
    Ok(())
}
