use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::Message,
};

use crate::bot::edit_to_ai_lab;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{t, tf};

use super::client::{transcribe_voice, get_cpu_status, AsrError};

static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

fn next_trace_id() -> u64 {
    NEXT_TRACE.fetch_add(1, Ordering::Relaxed)
}

pub const CB_ASR_CANCEL: &str = "asr:cancel";
pub const CB_ASR_CONFIRM: &str = "asr:confirm";
pub const CB_ASR_QUEUE: &str = "asr:queue";

fn fmt_duration(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

fn fmt_estimated(secs: f64, cores: u32) -> String {
    let estimated_secs = (secs / cores.max(1) as f64) * 1.5;
    let m = (estimated_secs / 60.0) as u64;
    let s = estimated_secs as u64 % 60;
    format!("~{}:{:02}", m, s)
}

// Extract file_id, duration (seconds), and extension from a Telegram message.
// Duration comes from Telegram metadata — no download needed.
fn extract_file_info(message: &Message) -> Option<(String, f64, &'static str)> {
    if let Some(v) = &message.voice {
        return Some((v.file_id.clone(), v.duration as f64, "ogg"));
    }
    if let Some(a) = &message.audio {
        let ext = a.file_name.as_deref()
            .and_then(|n| n.rsplit('.').next())
            .unwrap_or("mp3");
        let ext = match ext.to_lowercase().as_str() {
            "mp3" => "mp3", "m4a" => "m4a", "aac" => "aac",
            "flac" => "flac", "wav" => "wav", "opus" => "opus",
            _ => "mp3",
        };
        return Some((a.file_id.clone(), a.duration as f64, ext));
    }
    if let Some(v) = &message.video {
        return Some((v.file_id.clone(), v.duration as f64, "mp4"));
    }
    if let Some(v) = &message.video_note {
        return Some((v.file_id.clone(), v.duration as f64, "mp4"));
    }
    if let Some(d) = &message.document {
        let ext = d.file_name.as_deref()
            .and_then(|n| n.rsplit('.').next())
            .unwrap_or("mp4");
        let ext = match ext.to_lowercase().as_str() {
            "ogg" => "ogg", "mp3" => "mp3", "m4a" => "m4a", "aac" => "aac",
            "flac" => "flac", "wav" => "wav", "opus" => "opus",
            "mp4" => "mp4", "mkv" => "mkv", "avi" => "avi",
            "mov" => "mov", "webm" => "webm",
            _ => "mp4",
        };
        // duration = 0 for documents (Telegram doesn't provide it)
        return Some((d.file_id.clone(), 0.0, ext));
    }
    None
}

pub async fn enter_asr(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    eprintln!("[asr trace={trace_id} event=enter] user_id={user_id} chat_id={chat_id}");

    flow_manager.set(user_id, FlowState::AwaitingAsrAudio);

    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(t("asr.send_audio_prompt"))
        .reply_markup(cancel_keyboard())
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[asr trace={trace_id} event=prompt_shown]"),
        Err(e) => eprintln!("[asr trace={trace_id} event=prompt_failed] err={e}"),
    }
}

pub async fn handle_asr_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    eprintln!("[asr trace={trace_id} event=cancel] user_id={user_id}");
    flow_manager.clear(user_id);
    let r = edit_to_ai_lab(api, chat_id, message_id).await;
    eprintln!("[asr trace={trace_id} event=cancel_done] ok={}", r.is_ok());
}

// Fast: reads duration from Telegram metadata (no download), checks CPU, shows prompt.
// Runs synchronously in the event loop.
pub async fn handle_asr_audio(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    eprintln!("[asr trace={trace_id} event=audio_received] user_id={user_id}");

    let Some((file_id, duration_secs, ext)) = extract_file_info(message) else {
        eprintln!("[asr trace={trace_id} event=no_file_id]");
        let _ = send_new(api, chat_id, &t("asr.error.invalid_audio")).await;
        return;
    };

    let filename = format!("audio.{ext}");

    eprintln!("[asr trace={trace_id} event=file_extracted] file_id={file_id} duration={duration_secs:.0}s ext={ext}");

    // Check available cores — fast HTTP call
    let cores = match get_cpu_status().await {
        Ok(s) => s.available_cores,
        Err(e) => {
            eprintln!("[asr trace={trace_id} event=cpu_status_failed] err={e}");
            0
        }
    };

    eprintln!("[asr trace={trace_id} event=cpu_status] cores={cores}");

    if cores == 0 {
        // Server busy — ask if user wants to queue
        crate::stats::record_event_user(user_id, "cpu", "queue", "asr", 0).await;
        flow_manager.set(user_id, FlowState::AwaitingAsrQueued {
            file_id,
            filename,
            duration_secs,
        });
        let dur_str = if duration_secs > 0.0 {
            fmt_duration(duration_secs)
        } else {
            t("asr.duration_unknown")
        };
        let text = tf("asr.queue_prompt", &[("duration", &dur_str)]);
        let _ = send_new_keyboard(api, chat_id, &text, queue_keyboard()).await;
    } else {
        // Cores available — show confirm prompt
        flow_manager.set(user_id, FlowState::AwaitingAsrConfirm {
            file_id,
            filename,
            duration_secs,
        });
        let dur_str = if duration_secs > 0.0 {
            fmt_duration(duration_secs)
        } else {
            t("asr.duration_unknown")
        };
        let est_str = if duration_secs > 0.0 {
            fmt_estimated(duration_secs, cores)
        } else {
            t("asr.estimated_unknown")
        };
        let text = tf("asr.confirm_prompt", &[
            ("duration", &dur_str),
            ("cores", &cores.to_string()),
            ("estimated", &est_str),
        ]);
        let _ = send_new_keyboard(api, chat_id, &text, confirm_keyboard()).await;
    }
}

// Called when user presses confirm button.
// Reads state synchronously, then spawns the heavy inference work.
pub async fn handle_asr_confirm(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
    is_queue: bool,
) {
    let trace_id = next_trace_id();

    let (file_id, filename, duration_secs) = match flow_manager.get(user_id) {
        FlowState::AwaitingAsrConfirm { file_id, filename, duration_secs } => (file_id, filename, duration_secs),
        FlowState::AwaitingAsrQueued { file_id, filename, duration_secs } => (file_id, filename, duration_secs),
        other => {
            eprintln!("[asr trace={trace_id} event=confirm_bad_state] user_id={user_id} state={other:?}");
            return;
        }
    };

    flow_manager.clear(user_id);

    let init_text = if is_queue {
        t("asr.queued_waiting")
    } else {
        tf("asr.processing", &[("seconds", "0")])
    };
    let _ = edit_status(api, chat_id, message_id, &init_text).await;

    eprintln!("[asr trace={trace_id} event=confirm] user_id={user_id} file_id={file_id} dur={duration_secs:.0}s is_queue={is_queue}");

    let api2 = api.clone();
    tokio::spawn(async move {
        run_asr_inference(api2, chat_id, message_id, user_id, file_id, filename, trace_id).await;
    });
}

async fn run_asr_inference(
    api: Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    file_id: String,
    filename: String,
    trace_id: u64,
) {
    let t_start = Instant::now();

    // Download file to temp
    let ext = filename.rsplit('.').next().unwrap_or("mp4");
    let work_dir = std::env::temp_dir().join(format!("asr_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();
    let audio_path = work_dir.join(format!("input.{ext}"));

    eprintln!("[asr trace={trace_id} event=download_start]");
    if let Err(e) = download_file(&api, &file_id, audio_path.to_str().unwrap(), trace_id).await {
        eprintln!("[asr trace={trace_id} event=download_failed] err={e}");
        std::fs::remove_dir_all(&work_dir).ok();
        crate::stats::record_event_user(user_id, "asr", "", "fail", 0).await;
        crate::stats::record_error_global("asr", &format!("download failed: {e}")).await;
        let _ = edit_status(&api, chat_id, message_id, &t("asr.error.download_failed")).await;
        return;
    }

    // Ticker: update elapsed time every 5s
    let api_tick = api.clone();
    let ticker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.tick().await;
        loop {
            interval.tick().await;
            let secs = t_start.elapsed().as_secs();
            let text = tf("asr.processing", &[("seconds", &secs.to_string())]);
            let _ = edit_status(&api_tick, chat_id, message_id, &text).await;
        }
    });

    eprintln!("[asr trace={trace_id} event=inference_start] path={}", audio_path.display());
    let result = transcribe_voice(&audio_path, user_id).await;
    ticker.abort();

    let elapsed = t_start.elapsed();
    std::fs::remove_dir_all(&work_dir).ok();

    match result {
        Ok(asr) => {
            eprintln!(
                "[asr trace={trace_id} event=inference_done] lang={} audio_dur={:.1}s elapsed={:.1}s",
                asr.language, asr.duration_seconds, elapsed.as_secs_f64()
            );

            let reply = tf("asr.result", &[
                ("text", &asr.text),
                ("language", &asr.language),
                ("audio_duration", &fmt_duration(asr.duration_seconds)),
                ("elapsed", &fmt_duration(elapsed.as_secs_f64())),
            ]);
            let _ = edit_status(&api, chat_id, message_id, &reply).await;

            if !asr.srt.is_empty() {
                let stem = filename.rsplit('.').nth(1).unwrap_or("subtitle");
                let srt_path = work_dir.parent()
                    .unwrap_or(std::path::Path::new("/tmp"))
                    .join(format!("asr_{trace_id}_{stem}.srt"));
                if std::fs::write(&srt_path, asr.srt.as_bytes()).is_ok() {
                    let doc_params = SendDocumentParams::builder()
                        .chat_id(chat_id)
                        .document(srt_path.clone())
                        .caption(t("asr.srt_caption"))
                        .build();
                    match api.send_document(&doc_params).await {
                        Ok(_) => eprintln!("[asr trace={trace_id} event=srt_sent]"),
                        Err(e) => eprintln!("[asr trace={trace_id} event=srt_send_failed] err={e}"),
                    }
                    std::fs::remove_file(&srt_path).ok();
                }
            }

            crate::stats::record_event_user(user_id, "asr", "", "ok", asr.duration_seconds.ceil() as i64).await;
        }
        Err(AsrError::ServiceUnavailable) => {
            eprintln!("[asr trace={trace_id} event=service_unavailable]");
            crate::stats::record_event_user(user_id, "asr", "", "fail", 0).await;
            crate::stats::record_error_global("asr", "service unavailable").await;
            let _ = edit_status(&api, chat_id, message_id, &t("asr.error.service_unavailable")).await;
        }
        Err(AsrError::Timeout) => {
            eprintln!("[asr trace={trace_id} event=timeout] elapsed={:.1}s", elapsed.as_secs_f64());
            crate::stats::record_event_user(user_id, "cpu", "timeout", "asr", 0).await;
            crate::stats::record_event_user(user_id, "asr", "", "timeout", 0).await;
            crate::stats::record_error_global("asr", "timeout").await;
            let _ = edit_status(&api, chat_id, message_id, &t("asr.error.timeout")).await;
        }
        Err(e) => {
            eprintln!("[asr trace={trace_id} event=inference_failed] err={e}");
            crate::stats::record_event_user(user_id, "asr", "", "fail", 0).await;
            crate::stats::record_error_global("asr", &format!("inference failed: {e}")).await;
            let _ = edit_status(&api, chat_id, message_id, &t("asr.error.processing_failed")).await;
        }
    }
}

async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: &str) {
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => {}
        Err(e) if e.to_string().contains("message is not modified") => {}
        Err(e) => eprintln!("[asr event=edit_status_failed] err={e}"),
    }
}

async fn send_new(api: &Bot, chat_id: i64, text: &str) {
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[asr event=send_new_failed] err={e}");
    }
}

async fn send_new_keyboard(
    api: &Bot,
    chat_id: i64,
    text: &str,
    keyboard: frankenstein::types::InlineKeyboardMarkup,
) {
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(keyboard))
        .build();
    if let Err(e) = api.send_message(&params).await {
        eprintln!("[asr event=send_keyboard_failed] err={e}");
    }
}

fn cancel_keyboard() -> frankenstein::types::InlineKeyboardMarkup {
    use frankenstein::types::{InlineKeyboardButton, InlineKeyboardMarkup};
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![inline_btn(&t("asr.cancel_button"), CB_ASR_CANCEL)]])
        .build()
}

fn confirm_keyboard() -> frankenstein::types::InlineKeyboardMarkup {
    use frankenstein::types::InlineKeyboardMarkup;
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            inline_btn(&t("asr.confirm_button"), CB_ASR_CONFIRM),
            inline_btn(&t("asr.cancel_button"), CB_ASR_CANCEL),
        ]])
        .build()
}

fn queue_keyboard() -> frankenstein::types::InlineKeyboardMarkup {
    use frankenstein::types::InlineKeyboardMarkup;
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            inline_btn(&t("asr.queue_button"), CB_ASR_QUEUE),
            inline_btn(&t("asr.cancel_button"), CB_ASR_CANCEL),
        ]])
        .build()
}

fn inline_btn(text: &str, cb: &str) -> frankenstein::types::InlineKeyboardButton {
    frankenstein::types::InlineKeyboardButton {
        text: text.to_string(),
        callback_data: Some(cb.to_string()),
        style: None,
        icon_custom_emoji_id: None,
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}

async fn download_file(
    api: &Bot,
    file_id: &str,
    dest: &str,
    trace_id: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use frankenstein::methods::GetFileParams;

    let file_info = api.get_file(&GetFileParams::builder().file_id(file_id).build()).await?;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    eprintln!("[asr trace={trace_id} event=file_path] {file_path}");

    // Local Bot API: file_path starts with '/'
    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        eprintln!("[asr trace={trace_id} event=local_copy] size={size}");
        return Ok(());
    }

    let token = std::env::var("BOT_TOKEN").map_err(|_| "BOT_TOKEN not set")?;
    let base = std::env::var("BOT_API_BASE_URL").ok();
    let url = match base.as_deref() {
        Some(b) => format!("{}/file/bot{token}/{file_path}", b.trim_end_matches('/')),
        None => format!("https://api.telegram.org/file/bot{token}/{file_path}"),
    };

    eprintln!("[asr trace={trace_id} event=http_download] prefix={}", &url[..url.len().min(60)]);
    let bytes = reqwest::get(&url).await?.bytes().await?;
    eprintln!("[asr trace={trace_id} event=http_done] bytes={}", bytes.len());
    std::fs::write(dest, &bytes)?;
    Ok(())
}
