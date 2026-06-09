use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{
        AnswerCallbackQueryParams, DeleteMessageParams, EditMessageTextParams,
        SendAudioParams,
    },
    types::{InlineKeyboardMarkup, Message},
};
use tokio::sync::mpsc::UnboundedSender;

use crate::bot::{send_text, edit_to_ai_lab};
use crate::emoji::{FlowManager, FlowState};
use crate::emoji::panel::{btn_icon_success, btn_icon, btn_icon_danger};
use crate::i18n::{t, entities_for_text};
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
pub const CB_SEP_QUEUE_CANCEL: &str = "sep:qcancel";

fn queue_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            btn_icon_danger(&t("separation.queue.cancel_btn"), CB_SEP_QUEUE_CANCEL, "cancel"),
        ]])
        .build()
}

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
    let entities = entities_for_text(&text);
    let mut params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(prompt_keyboard(message_id))
        .build();
    if !entities.is_empty() { params.entities = Some(entities); }
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
    let is_video = message.video.is_some();
    let file_id = message.audio.as_ref().map(|a| a.file_id.clone())
        .or_else(|| message.voice.as_ref().map(|v| v.file_id.clone()))
        .or_else(|| message.video.as_ref().map(|v| v.file_id.clone()))
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));

    let Some(file_id) = file_id else {
        eprintln!("[separation trace={trace_id} event=no_file_id]");
        let _ = send_text(api, chat_id, &t("separation.error.invalid_audio")).await;
        return;
    };

    let orig_filename = message.audio.as_ref().and_then(|a| a.file_name.as_deref())
        .or_else(|| message.document.as_ref().and_then(|d| d.file_name.as_deref()))
        .unwrap_or(if is_video { "video.mp4" } else { "audio.mp3" })
        .to_string();

    eprintln!("[separation trace={trace_id} event=file_stored] file_id={file_id} filename={orig_filename} is_video={is_video}");

    // Update flow to store file info, waiting for mode selection.
    flow_manager.set(user_id, FlowState::AwaitingSeparationMode {
        file_id: file_id.clone(),
        filename: orig_filename.clone(),
        prompt_msg_id: None,
        is_video,
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
                is_video,
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
    flow_clear_tx: UnboundedSender<i64>,
) {
    let trace_id = next_trace_id();
    eprintln!("[separation trace={trace_id} event=callback] user_id={user_id} chat_id={chat_id} data={cb_data}");

    // sep:qcancel — user cancelled while in queue
    if cb_data == CB_SEP_QUEUE_CANCEL {
        eprintln!("[separation trace={trace_id} event=queue_cancel] user_id={user_id}");
        if let FlowState::AwaitingSeparationQueued { cancel } = flow_manager.get(user_id) {
            cancel.store(true, Ordering::Relaxed);
        }
        flow_manager.clear(user_id);
        let r = edit_to_ai_lab(api, chat_id, message_id).await;
        eprintln!("[separation trace={trace_id} event=queue_cancel_done] ok={}", r.is_ok());
        return;
    }

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
    let (file_id, filename, is_video) = match flow_manager.get(user_id) {
        FlowState::AwaitingSeparationMode { file_id, filename, is_video, .. } => (file_id, filename, is_video),
        other => {
            eprintln!("[separation trace={trace_id} event=wrong_state] state={other:?}");
            let _ = send_text(api, chat_id, &t("separation.error.service_unavailable")).await;
            return;
        }
    };

    // Clear flow — processing starts.
    flow_manager.clear(user_id);

    // Edit keyboard message to "processing…"
    let processing_text = if is_video {
        t("separation.extracting_audio")
    } else {
        t("separation.processing")
    };
    let edit_params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&processing_text)
        .build();
    match api.edit_message_text(&edit_params).await {
        Ok(_) => eprintln!("[separation trace={trace_id} event=processing_msg_shown] is_video={is_video}"),
        Err(e) => eprintln!("[separation trace={trace_id} event=processing_msg_failed] err={e}"),
    }

    // Download file from Telegram.
    eprintln!("[separation trace={trace_id} event=download_start] file_id={file_id} filename={filename} is_video={is_video}");
    let file_bytes = match download_file(api, &file_id, trace_id).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[separation trace={trace_id} event=download_failed] err={e}");
            let _ = send_text(api, chat_id, &t("separation.error.service_unavailable")).await;
            let _ = delete_message(api, chat_id, message_id).await;
            return;
        }
    };
    eprintln!("[separation trace={trace_id} event=download_done] bytes={}", file_bytes.len());

    // If video: extract audio with ffmpeg, then compress if needed.
    let tmp_dir = std::env::temp_dir().join(format!("sep_{trace_id}"));
    std::fs::create_dir_all(&tmp_dir).ok();

    let audio_bytes = if is_video {
        match extract_and_prepare_audio(&file_bytes, &tmp_dir, message_id, chat_id, api, trace_id).await {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[separation trace={trace_id} event=extract_failed] err={e}");
                let _ = send_text(api, chat_id, &t("separation.error.audio_extraction_failed")).await;
                let _ = delete_message(api, chat_id, message_id).await;
                std::fs::remove_dir_all(&tmp_dir).ok();
                return;
            }
        }
    } else {
        file_bytes
    };
    eprintln!("[separation trace={trace_id} event=audio_ready] bytes={}", audio_bytes.len());

    // Update status to processing.
    if is_video {
        let edit_params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(t("separation.processing"))
            .build();
        let _ = api.edit_message_text(&edit_params).await;
    }

    // Call separation service.
    // Before sending to service, show queue message and manage 5min + 30min timeout.
    eprintln!("[separation trace={trace_id} event=separate_start] mode={mode_label}");
    let audio_filename: String = if is_video { "audio.mp3".to_string() } else { filename.clone() };

    // Cancel token: set to true if user presses cancel while in queue.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    flow_manager.set(user_id, FlowState::AwaitingSeparationQueued { cancel: cancel_flag.clone() });

    // Show initial "در صف" message.
    {
        let text = t("separation.queue.waiting");
        let entities = entities_for_text(&text);
        let kb = queue_cancel_keyboard();
        let mut params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&text)
            .reply_markup(kb)
            .build();
        if !entities.is_empty() { params.entities = Some(entities); }
        let _ = api.edit_message_text(&params).await;
        eprintln!("[separation trace={trace_id} event=queue_msg_shown]");
    }

    // Spawn all heavy work so the event loop stays free — this makes the cancel button work.
    let api_task = api.clone();
    let cancel_task = cancel_flag.clone();
    tokio::spawn(async move {
        // Status-update task: after 5 min (if not done) edit to "still busy".
        // Keep the handle so we can abort it when separation finishes.
        let api_status = api_task.clone();
        let cancel_status = cancel_task.clone();
        let status_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(300)).await;
            if cancel_status.load(Ordering::Relaxed) { return; }
            let text = t("separation.queue.still_busy");
            let entities = entities_for_text(&text);
            let kb = queue_cancel_keyboard();
            let mut params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(&text)
                .reply_markup(kb)
                .build();
            if !entities.is_empty() { params.entities = Some(entities); }
            let _ = api_status.edit_message_text(&params).await;
        });

        // Race separation against cancel signal (cancel aborts the HTTP request via drop).
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled2 = cancelled.clone();
        let sep_result = tokio::select! {
            r = tokio::time::timeout(
                Duration::from_secs(35 * 60),
                separate_audio(audio_bytes, &audio_filename, mode, user_id, false),
            ) => {
                match r {
                    Ok(r) => Some(r),
                    Err(_) => None, // 35-min timeout
                }
            }
            _ = async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    if cancel_task.load(Ordering::Relaxed) { break; }
                }
                cancelled2.store(true, Ordering::Relaxed);
            } => { None }
        };

        // Abort orphan status-update task now that we have a result.
        status_task.abort();

        // Signal main loop to clear FlowState (AwaitingSeparationQueued → Idle).
        let _ = flow_clear_tx.send(user_id);

        if cancelled.load(Ordering::Relaxed) {
            eprintln!("[separation trace={trace_id} event=cancelled_in_queue]");
            std::fs::remove_dir_all(&tmp_dir).ok();
            return;
        }

        let result = match sep_result {
            None => {
                eprintln!("[separation trace={trace_id} event=queue_timeout]");
                let _ = send_text(&api_task, chat_id, &t("separation.error.queue_timeout")).await;
                let _ = delete_message(&api_task, chat_id, message_id).await;
                std::fs::remove_dir_all(&tmp_dir).ok();
                return;
            }
            Some(r) => r,
        };

        match result {
            Ok(result) => {
                eprintln!("[separation trace={trace_id} event=separate_done] duration={:.1}s vocals_wav={} instrumental_wav={} vocals_compressed={} instrumental_compressed={} ext={}",
                    result.duration_seconds, result.vocals_wav.len(), result.instrumental_wav.len(),
                    result.vocals_compressed.len(), result.instrumental_compressed.len(), result.compressed_ext);

                let _ = delete_message(&api_task, chat_id, message_id).await;

                let vocals_wav_path = tmp_dir.join("vocals.wav");
                let instrumental_wav_path = tmp_dir.join("instrumental.wav");
                let vocals_compressed_path = tmp_dir.join(format!("vocals.{}", result.compressed_ext));
                let instrumental_compressed_path = tmp_dir.join(format!("instrumental.{}", result.compressed_ext));

                std::fs::write(&vocals_wav_path, &result.vocals_wav).ok();
                std::fs::write(&instrumental_wav_path, &result.instrumental_wav).ok();
                std::fs::write(&vocals_compressed_path, &result.vocals_compressed).ok();
                std::fs::write(&instrumental_compressed_path, &result.instrumental_compressed).ok();

                eprintln!("[separation trace={trace_id} event=send_vocals_compressed]");
                let p = SendAudioParams::builder()
                    .chat_id(chat_id)
                    .audio(PathBuf::from(&vocals_compressed_path))
                    .caption(t("separation.result.vocals_compressed_caption"))
                    .build();
                match api_task.send_audio(&p).await {
                    Ok(_) => eprintln!("[separation trace={trace_id} event=vocals_compressed_sent]"),
                    Err(e) => eprintln!("[separation trace={trace_id} event=vocals_compressed_failed] err={e}"),
                }

                eprintln!("[separation trace={trace_id} event=send_vocals_wav]");
                let p = frankenstein::methods::SendDocumentParams::builder()
                    .chat_id(chat_id)
                    .document(PathBuf::from(&vocals_wav_path))
                    .caption(t("separation.result.vocals_wav_caption"))
                    .build();
                match api_task.send_document(&p).await {
                    Ok(_) => eprintln!("[separation trace={trace_id} event=vocals_wav_sent]"),
                    Err(e) => eprintln!("[separation trace={trace_id} event=vocals_wav_failed] err={e}"),
                }

                eprintln!("[separation trace={trace_id} event=send_instrumental_compressed]");
                let p = SendAudioParams::builder()
                    .chat_id(chat_id)
                    .audio(PathBuf::from(&instrumental_compressed_path))
                    .caption(t("separation.result.instrumental_compressed_caption"))
                    .build();
                match api_task.send_audio(&p).await {
                    Ok(_) => eprintln!("[separation trace={trace_id} event=instrumental_compressed_sent]"),
                    Err(e) => eprintln!("[separation trace={trace_id} event=instrumental_compressed_failed] err={e}"),
                }

                eprintln!("[separation trace={trace_id} event=send_instrumental_wav]");
                let p = frankenstein::methods::SendDocumentParams::builder()
                    .chat_id(chat_id)
                    .document(PathBuf::from(&instrumental_wav_path))
                    .caption(t("separation.result.instrumental_wav_caption"))
                    .build();
                match api_task.send_document(&p).await {
                    Ok(_) => eprintln!("[separation trace={trace_id} event=instrumental_wav_sent]"),
                    Err(e) => eprintln!("[separation trace={trace_id} event=instrumental_wav_failed] err={e}"),
                }

                std::fs::remove_dir_all(&tmp_dir).ok();
                eprintln!("[separation trace={trace_id} event=cleanup_done]");
            }
            Err(e) => {
                eprintln!("[separation trace={trace_id} event=separate_error] err={e}");
                let _ = delete_message(&api_task, chat_id, message_id).await;
                use super::error::SeparationError;
                let key = match &e {
                    SeparationError::ServiceUnavailable => "separation.error.service_unavailable",
                    SeparationError::InvalidAudio => "separation.error.invalid_audio",
                    SeparationError::Timeout => "separation.error.timeout",
                    SeparationError::ProcessingFailed(_) => "separation.error.processing_failed",
                };
                let _ = send_text(&api_task, chat_id, &t(key)).await;
                std::fs::remove_dir_all(&tmp_dir).ok();
            }
        }
    });
    // handle_separation_callback returns immediately; the spawned task does all heavy work.
}

async fn extract_and_prepare_audio(
    video_bytes: &[u8],
    tmp_dir: &std::path::Path,
    message_id: i32,
    chat_id: i64,
    api: &Bot,
    trace_id: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    const MAX_AUDIO_BYTES: u64 = 50 * 1024 * 1024;

    let video_path = tmp_dir.join("input_video");
    std::fs::write(&video_path, video_bytes)?;

    // Extract audio as MP3 at 320kbps.
    let audio_path = tmp_dir.join("extracted.mp3");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-i", video_path.to_str().unwrap(),
               "-vn", "-acodec", "libmp3lame", "-b:a", "320k",
               audio_path.to_str().unwrap()])
        .output().await?;
    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(format!("ffmpeg extract failed: {stderr}").into());
    }

    let audio_size = std::fs::metadata(&audio_path)?.len();
    eprintln!("[separation trace={trace_id} event=audio_extracted] size={audio_size}");

    if audio_size <= MAX_AUDIO_BYTES {
        return Ok(std::fs::read(&audio_path)?);
    }

    // Iteratively compress: reduce bitrate by 10% each attempt until < 50MB.
    // Probe current bitrate then step down.
    let probe = tokio::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0",
               "-show_entries", "stream=bit_rate",
               "-of", "default=noprint_wrappers=1:nokey=1",
               audio_path.to_str().unwrap()])
        .output().await?;
    let initial_bitrate: u32 = String::from_utf8_lossy(&probe.stdout)
        .trim().parse().unwrap_or(320_000);

    let mut bitrate_bps = initial_bitrate;
    let mut attempt = 0u32;
    const MAX_ATTEMPTS: u32 = 20;

    loop {
        attempt += 1;
        bitrate_bps = (bitrate_bps as f64 * 0.9) as u32;
        let bitrate_kbps = (bitrate_bps / 1000).max(32);
        eprintln!("[separation trace={trace_id} event=compress_attempt] attempt={attempt} bitrate={bitrate_kbps}k");

        let edit_text = crate::i18n::tf("separation.compressing_audio",
            &[("attempt", &attempt.to_string()), ("max", &MAX_ATTEMPTS.to_string())]);
        let _ = api.edit_message_text(&EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(edit_text)
            .build()).await;

        let out_path = tmp_dir.join(format!("compressed_{attempt}.mp3"));
        let status = tokio::process::Command::new("ffmpeg")
            .args(["-y", "-i", audio_path.to_str().unwrap(),
                   "-acodec", "libmp3lame", "-b:a", &format!("{bitrate_kbps}k"),
                   out_path.to_str().unwrap()])
            .output().await?;
        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            return Err(format!("ffmpeg compress failed: {stderr}").into());
        }

        let size = std::fs::metadata(&out_path)?.len();
        eprintln!("[separation trace={trace_id} event=compressed] attempt={attempt} size={size}");

        if size <= MAX_AUDIO_BYTES {
            return Ok(std::fs::read(&out_path)?);
        }

        if attempt >= MAX_ATTEMPTS || bitrate_kbps <= 32 {
            return Err("audio still too large after max compression attempts".into());
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
