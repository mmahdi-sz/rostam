use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams},
    types::{ButtonStyle, InlineKeyboardButton, InlineKeyboardMarkup, Message},
};

use crate::bot::{send_text, edit_to_ai_lab};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::t;

static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

fn next_trace_id() -> u64 {
    NEXT_TRACE.fetch_add(1, Ordering::Relaxed)
}

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
            url: None, login_url: None, web_app: None,
            switch_inline_query: None, switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None, copy_text: None,
            callback_game: None, pay: None,
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
    eprintln!("[gwm trace={trace_id} event=enter] user_id={user_id} chat_id={chat_id}");

    let text = t("gemini_wm.prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .reply_markup(cancel_keyboard())
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[gwm trace={trace_id} event=prompt_shown]"),
        Err(e) => eprintln!("[gwm trace={trace_id} event=prompt_failed] err={e}"),
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
    eprintln!("[gwm trace={trace_id} event=cancel] user_id={user_id} chat_id={chat_id}");
    flow_manager.clear(user_id);
    let r = edit_to_ai_lab(api, chat_id, message_id).await;
    eprintln!("[gwm trace={trace_id} event=cancel_done] ok={}", r.is_ok());
}

pub async fn handle_gwm_image(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    eprintln!(
        "[gwm trace={trace_id} event=image_received] user_id={user_id} chat_id={chat_id} \
         has_photo={} has_doc={}",
        message.photo.is_some(), message.document.is_some()
    );

    // Get file_id and extension from photo (largest) or document.
    let file_id = message.photo.as_ref()
        .and_then(|photos| photos.last())
        .map(|p| p.file_id.clone())
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()));

    let Some(file_id) = file_id else {
        eprintln!("[gwm trace={trace_id} event=no_file_id]");
        let _ = send_text(api, chat_id, &t("gemini_wm.error.invalid_image")).await;
        return;
    };

    let ext = detect_ext(message);
    eprintln!("[gwm trace={trace_id} event=file_info] file_id={file_id} ext={ext}");

    flow_manager.clear(user_id);

    let _ = send_text(api, chat_id, &t("gemini_wm.processing")).await;

    // Download image.
    eprintln!("[gwm trace={trace_id} event=download_start] file_id={file_id}");
    let work_dir = std::env::temp_dir().join(format!("gwm_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();
    let input_path = work_dir.join(format!("input.{ext}"));

    if let Err(e) = download_file(api, &file_id, input_path.to_str().unwrap(), trace_id).await {
        eprintln!("[gwm trace={trace_id} event=download_failed] err={e}");
        let _ = send_text(api, chat_id, &t("gemini_wm.error.download_failed")).await;
        std::fs::remove_dir_all(&work_dir).ok();
        return;
    }
    let file_size = std::fs::metadata(&input_path).map(|m| m.len()).unwrap_or(0);
    eprintln!("[gwm trace={trace_id} event=download_done] size={file_size}");

    let image_bytes = match std::fs::read(&input_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[gwm trace={trace_id} event=read_failed] err={e}");
            let _ = send_text(api, chat_id, &t("gemini_wm.error.processing_failed")).await;
            std::fs::remove_dir_all(&work_dir).ok();
            return;
        }
    };
    std::fs::remove_dir_all(&work_dir).ok();

    // Run watermark removal.
    eprintln!("[gwm trace={trace_id} event=remove_start]");
    let t_start = std::time::Instant::now();
    let result_bytes = match super::remove::remove_watermark(image_bytes, ext.clone(), user_id, trace_id).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[gwm trace={trace_id} event=remove_failed] err={e}");
            let _ = send_text(api, chat_id, &t("gemini_wm.error.processing_failed")).await;
            return;
        }
    };
    let elapsed = t_start.elapsed().as_secs_f64();
    eprintln!("[gwm trace={trace_id} event=remove_done] elapsed={elapsed:.1}s output_bytes={}", result_bytes.len());

    // Write result and send as document.
    let out_path = std::env::temp_dir().join(format!("gwm_out_{trace_id}.{ext}"));
    if let Err(e) = std::fs::write(&out_path, &result_bytes) {
        eprintln!("[gwm trace={trace_id} event=write_failed] err={e}");
        let _ = send_text(api, chat_id, &t("gemini_wm.error.processing_failed")).await;
        return;
    }

    let p = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(PathBuf::from(&out_path))
        .caption(t("gemini_wm.result_caption"))
        .build();
    match api.send_document(&p).await {
        Ok(_) => eprintln!("[gwm trace={trace_id} event=result_sent]"),
        Err(e) => {
            eprintln!("[gwm trace={trace_id} event=result_failed] err={e}");
            let _ = send_text(api, chat_id, &t("gemini_wm.error.processing_failed")).await;
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
            }.to_string();
        }
    }
    "jpg".to_string()
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

    eprintln!("[gwm trace={trace_id} event=file_path] file_path={file_path}");

    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        eprintln!("[gwm trace={trace_id} event=local_copy] size={size}");
        return Ok(());
    }

    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{}/{file_path}", crate::config::bot_token()?)
    } else {
        format!("https://api.telegram.org/file/bot{}/{file_path}", crate::config::bot_token()?)
    };

    eprintln!("[gwm trace={trace_id} event=http_download] url_prefix={}", &url[..url.len().min(60)]);
    let response = reqwest::get(&url).await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    eprintln!("[gwm trace={trace_id} event=http_done] status={status} bytes={}", bytes.len());
    std::fs::write(dest, &bytes)?;
    Ok(())
}
