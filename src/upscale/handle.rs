use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendPhotoParams},
    types::{InlineKeyboardMarkup, Message, InlineKeyboardButton, ButtonStyle},
};

use crate::bot::{send_text, send_text_md};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{t, tf};
use crate::youtube::log_trace;

const UPSCALE_BIN: &str = "files/realesrgan/realesrgan-ncnn-vulkan";
const MODEL_DIR: &str = "files/realesrgan/models";

fn next_trace_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub const CB_UPSCALE_CANCEL: &str = "upscale:cancel";
pub const CB_UPSCALE_MODEL_PREFIX: &str = "upscale:model:";

pub const UPSCALE_MODELS: &[(&str, u32, &str, &str)] = &[
    ("realesr-animevideov3-x2", 2, "upscale.model.anime_x2", "📺"),
    ("realesr-animevideov3-x3", 3, "upscale.model.anime_x3", "📺"),
    ("realesr-animevideov3-x4", 4, "upscale.model.anime_x4", "📺"),
    ("realesrgan-x4plus-anime", 4, "upscale.model.anime_pro", "🎨"),
    ("realesrgan-x4plus", 4, "upscale.model.general", "🖼️"),
];

/// Called from main.rs when `ai:upscale` is pressed.
/// Edits the AI Lab message to show the upscale model selection.
pub async fn enter_upscale(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();

    // Default selection
    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor: 4,
        model_name: "realesr-animevideov3-x4".to_string(),
    });

    let text = t("upscale.prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard_by_model("realesr-animevideov3-x4"))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => log_trace(trace_id, "upscale_prompt_shown", &format!("user_id={user_id} chat_id={chat_id}")),
        Err(e) => log_trace(trace_id, "upscale_prompt_failed", &e.to_string()),
    }
}

fn upscale_keyboard_by_model(active_model: &str) -> InlineKeyboardMarkup {
    let mut rows = vec![];

    for (model_name, _scale, label_key, icon) in UPSCALE_MODELS {
        let is_active = *model_name == active_model;
        let label = if is_active {
            format!("✅ {} {}", t(label_key), icon)
        } else {
            format!("{} {}", t(label_key), icon)
        };
        let btn_style = if is_active { Some(ButtonStyle::Primary) } else { None };
        rows.push(vec![InlineKeyboardButton {
            text: label,
            callback_data: Some(format!("{}{}", CB_UPSCALE_MODEL_PREFIX, model_name)),
            style: btn_style,
            icon_custom_emoji_id: None,
            url: None, login_url: None, web_app: None,
            switch_inline_query: None, switch_inline_query_current_chat: None,
            switch_inline_query_chosen_chat: None, copy_text: None,
            callback_game: None, pay: None,
        }]);
    }

    rows.push(vec![InlineKeyboardButton {
        text: t("upscale.cancel_button"),
        callback_data: Some(CB_UPSCALE_CANCEL.to_string()),
        style: Some(ButtonStyle::Danger),
        icon_custom_emoji_id: None,
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }]);

    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

/// Handles upscale model pick callback.
pub async fn handle_upscale_model_pick(
    api: &Bot,
    model_name: &str,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();

    // Find the model info
    let (scale_factor, _label_key) = UPSCALE_MODELS
        .iter()
        .find(|(name, ..)| *name == model_name)
        .map(|(_, s, lk, _)| (*s, *lk))
        .unwrap_or((4, ""));

    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor,
        model_name: model_name.to_string(),
    });

    log_trace(trace_id, "upscale_model_pick", &format!("user_id={user_id} model={model_name} scale={scale_factor}"));

    let text = t("upscale.prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard_by_model(model_name))
        .build();
    let _ = api.edit_message_text(&params).await;
}

/// Handles upscale cancel callback — back to AI Lab.
pub async fn handle_upscale_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    flow_manager.clear(user_id);
    let r = crate::bot::edit_to_ai_lab(api, chat_id, message_id).await;
    log_trace(next_trace_id(), "upscale_cancel_done", &format!("ok={}", r.is_ok()));
}

/// Processes an image message when user is in AwaitingUpscaleImage.
pub async fn handle_upscale_image(
    api: &Bot,
    message: &Message,
    user_id: i64,
    scale_factor: u32,
    model_name: &str,
    flow_manager: &mut FlowManager,
) {
    flow_manager.clear(user_id);
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    // Get file_id from photo (largest) or document
    let file_id = message
        .photo
        .as_ref()
        .and_then(|photos| photos.last())
        .map(|p| &p.file_id)
        .or_else(|| message.document.as_ref().map(|d| &d.file_id));

    let Some(file_id) = file_id else {
        let _ = send_text(api, chat_id, &t("upscale.unsupported_format")).await;
        return;
    };

    let is_doc = message.document.is_some();
    let orig_ext = if is_doc {
        detect_doc_ext(message)
    } else {
        "jpg".to_string()
    };

    log_trace(trace_id, "upscale_image_received", &format!(
        "user_id={user_id} chat_id={chat_id} doc={is_doc} ext={orig_ext} model={model_name} scale={scale_factor}"
    ));

    let _ = send_text(api, chat_id, &t("upscale.preparing")).await;

    let work_dir = std::env::temp_dir().join(format!("upscale_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();

    let input_path = work_dir.join(format!("input.{}", orig_ext));
    let output_path = work_dir.join(format!("output.{}", orig_ext));

    // 1. Download
    if let Err(e) = download_file(api, file_id, input_path.to_str().unwrap()).await {
        log_trace(trace_id, "upscale_download_failed", &format!("err={e}"));
        let _ = send_text(api, chat_id, &t("upscale.download_failed")).await;
        clean_up(&work_dir);
        return;
    }
    let file_size = std::fs::metadata(input_path.to_str().unwrap()).map(|m| m.len()).unwrap_or(0);
    log_trace(trace_id, "upscale_downloaded", &format!("size={file_size}"));

    // 2. Run upscale
    let processing_secs = match run_upscale(
        input_path.to_str().unwrap(),
        output_path.to_str().unwrap(),
        model_name,
        scale_factor,
    ) {
        Ok(s) => {
            log_trace(trace_id, "upscale_done", &format!("elapsed={s:.1}s"));
            s
        }
        Err(e) => {
            log_trace(trace_id, "upscale_failed", &format!("err={e}"));
            let _ = send_text(api, chat_id, &t("upscale.upscale_failed")).await;
            clean_up(&work_dir);
            return;
        }
    };

    // 3. Send upscaled image
    let caption = t("upscale.result_caption");
    let scale_str = escape_md(&format!("{}x", scale_factor));
    let processing_str = escape_md(&format!("{:.1}", processing_secs));
    let report = tf("upscale.report", &[
        ("scale", &scale_str),
        ("processing", &processing_str),
    ]);

    if is_doc || orig_ext != "jpg" {
        // Send as document for non-jpg formats
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&format!("{}\n\n{}", caption, report))
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = api.send_document(&params).await;
        log_trace(trace_id, "upscale_doc_sent", &format!("ok={}", r.is_ok()));
    } else {
        // Send as photo
        let params = SendPhotoParams::builder()
            .chat_id(chat_id)
            .photo(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&format!("{}\n\n{}", caption, report))
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = api.send_photo(&params).await;
        log_trace(trace_id, "upscale_photo_sent", &format!("ok={}", r.is_ok()));
    }

    clean_up(&work_dir);
}

fn run_upscale(input: &str, output: &str, model_name: &str, scale: u32) -> Result<f64, Box<dyn std::error::Error>> {
    use std::time::Instant;
    let start = Instant::now();

    let status = std::process::Command::new(UPSCALE_BIN)
        .args([
            "-i", input,
            "-o", output,
            "-n", model_name,
            "-s", &scale.to_string(),
            "-m", MODEL_DIR,
        ])
        .status()
        .map_err(|e| format!("realesrgan spawn failed: {e}"))?;

    if !status.success() {
        return Err("realesrgan exited with non-zero status".into());
    }

    if !std::path::Path::new(output).exists() {
        return Err("realesrgan did not produce output file".into());
    }

    Ok(start.elapsed().as_secs_f64())
}

fn detect_doc_ext(message: &Message) -> String {
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

async fn download_file(api: &Bot, file_id: &str, dest: &str) -> Result<(), Box<dyn std::error::Error>> {
    use frankenstein::methods::GetFileParams;

    let file_info = api.get_file(&GetFileParams::builder().file_id(file_id).build()).await?;
    let file_path = file_info.result.file_path.ok_or("no file_path")?;

    log_trace(next_trace_id(), "upscale_file_path", &format!("file_path={file_path}"));

    // Local Bot API returns an absolute filesystem path in --local mode.
    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        log_trace(next_trace_id(), "upscale_file_local_copy", &format!("size={}", std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0)));
        return Ok(());
    }

    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{}/{file_path}", crate::config::bot_token()?)
    } else {
        format!("https://api.telegram.org/file/bot{}/{file_path}", crate::config::bot_token()?)
    };

    let response = reqwest::get(&url).await?;
    let bytes = response.bytes().await?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

/// Escape MarkdownV2 special characters in dynamic text.
fn escape_md(s: &str) -> String {
    s.chars().map(|c| match c {
        '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' => {
            format!("\\{c}")
        }
        other => other.to_string(),
    }).collect()
}

fn clean_up(dir: &std::path::Path) {
    std::fs::remove_dir_all(dir).ok();
}
