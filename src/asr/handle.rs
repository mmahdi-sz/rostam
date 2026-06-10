use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::EditMessageTextParams,
    types::Message,
};

use crate::bot::{send_text, edit_to_ai_lab};
use crate::config;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{t, tf};

use super::client::{transcribe_voice, AsrError};

static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

fn next_trace_id() -> u64 {
    NEXT_TRACE.fetch_add(1, Ordering::Relaxed)
}

pub const CB_ASR_CANCEL: &str = "asr:cancel";

pub async fn enter_asr(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    eprintln!("[asr trace={trace_id} event=enter] user_id={user_id} chat_id={chat_id}");

    let admin_id = config::admin_user_id();
    if admin_id != Some(user_id) {
        eprintln!("[asr trace={trace_id} event=access_denied] user_id={user_id}");
        let _ = api.answer_callback_query(
            &frankenstein::methods::AnswerCallbackQueryParams::builder()
                .callback_query_id(String::new())
                .build(),
        ).await;
        let _ = send_text(api, chat_id, &t("asr.error.access_denied")).await;
        return;
    }

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
    eprintln!("[asr trace={trace_id} event=cancel] user_id={user_id} chat_id={chat_id}");
    flow_manager.clear(user_id);
    let r = edit_to_ai_lab(api, chat_id, message_id).await;
    eprintln!("[asr trace={trace_id} event=cancel_done] ok={}", r.is_ok());
}

pub async fn handle_asr_audio(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    eprintln!(
        "[asr trace={trace_id} event=audio_received] user_id={user_id} chat_id={chat_id} \
         has_voice={} has_audio={} has_doc={}",
        message.voice.is_some(), message.audio.is_some(), message.document.is_some()
    );

    let file_id = message.voice.as_ref().map(|v| v.file_id.clone())
        .or_else(|| message.audio.as_ref().map(|a| a.file_id.clone()))
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));

    let Some(file_id) = file_id else {
        eprintln!("[asr trace={trace_id} event=no_file_id]");
        let _ = send_text(api, chat_id, &t("asr.error.invalid_audio")).await;
        return;
    };

    flow_manager.clear(user_id);

    // Send initial processing message
    let init_text = tf("asr.processing", &[("seconds", "0")]);
    let send_params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&init_text)
        .build();
    let status_msg_id = match api.send_message(&send_params).await {
        Ok(resp) => resp.result.message_id,
        Err(e) => {
            eprintln!("[asr trace={trace_id} event=send_processing_failed] err={e}");
            return;
        }
    };

    eprintln!("[asr trace={trace_id} event=download_start] file_id={file_id}");
    let work_dir = std::env::temp_dir().join(format!("asr_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();

    let ext = detect_ext(message);
    let audio_path = work_dir.join(format!("input.{ext}"));

    let download_err = download_file(api, &file_id, audio_path.to_str().unwrap(), trace_id)
        .await
        .err()
        .map(|e| e.to_string());
    if let Some(e_str) = download_err {
        eprintln!("[asr trace={trace_id} event=download_failed] err={e_str}");
        let _ = edit_status(api, chat_id, status_msg_id, &t("asr.error.download_failed")).await;
        std::fs::remove_dir_all(&work_dir).ok();
        return;
    }

    let file_size = std::fs::metadata(&audio_path).map(|m| m.len()).unwrap_or(0);
    eprintln!("[asr trace={trace_id} event=download_done] size={file_size} path={}", audio_path.display());

    // Spawn inference + timer ticker
    let api2 = api.clone();
    let work_dir2 = work_dir.clone();
    let audio_path2 = audio_path.clone();

    let t_start = Instant::now();

    // Ticker: edit status every 2s while inference runs
    let api_tick = api2.clone();
    let ticker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        interval.tick().await; // first tick fires immediately, skip it
        loop {
            interval.tick().await;
            let secs = t_start.elapsed().as_secs();
            let text = tf("asr.processing", &[("seconds", &secs.to_string())]);
            let _ = edit_status(&api_tick, chat_id, status_msg_id, &text).await;
        }
    });

    eprintln!("[asr trace={trace_id} event=inference_start]");
    let result = transcribe_voice(&audio_path2).await;
    ticker.abort();

    let elapsed = t_start.elapsed();
    std::fs::remove_dir_all(&work_dir2).ok();

    match result {
        Ok(asr) => {
            eprintln!(
                "[asr trace={trace_id} event=inference_done] lang={} tokens_approx={} \
                 duration={:.1}s elapsed={:.1}s",
                asr.language, asr.text.split_whitespace().count(),
                asr.duration_seconds, elapsed.as_secs_f64()
            );
            let elapsed_secs = elapsed.as_secs();
            let reply = tf("asr.result", &[
                ("text", &asr.text),
                ("language", &asr.language),
                ("audio_duration", &format!("{:.1}", asr.duration_seconds)),
                ("elapsed", &elapsed_secs.to_string()),
            ]);
            let _ = edit_status(api, chat_id, status_msg_id, &reply).await;
        }
        Err(AsrError::ServiceUnavailable) => {
            eprintln!("[asr trace={trace_id} event=service_unavailable]");
            let _ = edit_status(api, chat_id, status_msg_id, &t("asr.error.service_unavailable")).await;
        }
        Err(AsrError::Timeout) => {
            eprintln!("[asr trace={trace_id} event=timeout] elapsed={:.1}s", elapsed.as_secs_f64());
            let _ = edit_status(api, chat_id, status_msg_id, &t("asr.error.timeout")).await;
        }
        Err(e) => {
            eprintln!("[asr trace={trace_id} event=inference_failed] err={e}");
            let _ = edit_status(api, chat_id, status_msg_id, &t("asr.error.processing_failed")).await;
        }
    }
}

async fn edit_status(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    text: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let params = frankenstein::methods::EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("message is not modified") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn cancel_keyboard() -> frankenstein::types::InlineKeyboardMarkup {
    use frankenstein::types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup};
    let icon_id = t("emoji.panel.icons.cancel");
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![InlineKeyboardButton {
            text: t("asr.cancel_button"),
            callback_data: Some(CB_ASR_CANCEL.to_string()),
            style: Some(ButtonStyle::Danger),
            icon_custom_emoji_id: if icon_id.is_empty() || icon_id.starts_with('!') {
                None
            } else {
                Some(icon_id)
            },
            url: None, login_url: None, web_app: None,
            switch_inline_query: None, switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None, copy_text: None,
            callback_game: None, pay: None,
        }]])
        .build()
}

fn detect_ext(message: &Message) -> String {
    if message.voice.is_some() {
        return "ogg".to_string();
    }
    if let Some(doc) = &message.document {
        if let Some(name) = &doc.file_name {
            if let Some(ext) = name.rsplit('.').next() {
                return ext.to_lowercase();
            }
        }
    }
    if let Some(audio) = &message.audio {
        if let Some(name) = &audio.file_name {
            if let Some(ext) = name.rsplit('.').next() {
                return ext.to_lowercase();
            }
        }
    }
    "mp3".to_string()
}

async fn download_file(
    api: &Bot,
    file_id: &str,
    dest: &str,
    trace_id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    use frankenstein::methods::GetFileParams;

    let file_info = api.get_file(&GetFileParams::builder().file_id(file_id).build()).await?;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    eprintln!("[asr trace={trace_id} event=file_path] file_path={file_path}");

    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        eprintln!("[asr trace={trace_id} event=local_copy] size={size}");
        return Ok(());
    }

    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{}/{file_path}", crate::config::bot_token()?)
    } else {
        format!("https://api.telegram.org/file/bot{}/{file_path}", crate::config::bot_token()?)
    };

    eprintln!("[asr trace={trace_id} event=http_download] url_prefix={}", &url[..url.len().min(60)]);
    let response = reqwest::get(&url).await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    eprintln!("[asr trace={trace_id} event=http_done] status={status} bytes={}", bytes.len());
    std::fs::write(dest, &bytes)?;
    Ok(())
}
