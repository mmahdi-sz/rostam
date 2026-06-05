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
pub const CB_UPSCALE_ANIME_TOGGLE: &str = "upscale:anime_toggle";

// (model_name, scale, i18n_key) — ترتیب نمایش در submenu
const ANIME_MODELS: &[(&str, u32, &str)] = &[
    ("realesrgan-x4plus-anime", 4, "upscale.model.anime_pro"),
    ("realesr-animevideov3-x4", 4, "upscale.model.anime_x4"),
    ("realesr-animevideov3-x3", 3, "upscale.model.anime_x3"),
    ("realesr-animevideov3-x2", 2, "upscale.model.anime_x2"),
];

fn is_anime_model(model_name: &str) -> bool {
    ANIME_MODELS.iter().any(|(name, ..)| *name == model_name)
}

fn scale_for_model(model_name: &str) -> u32 {
    ANIME_MODELS.iter()
        .find(|(name, ..)| *name == model_name)
        .map(|(_, s, _)| *s)
        .unwrap_or(4)
}

fn btn(text: impl Into<String>, cb: impl Into<String>, style: Option<ButtonStyle>) -> InlineKeyboardButton {
    InlineKeyboardButton {
        text: text.into(),
        callback_data: Some(cb.into()),
        style,
        icon_custom_emoji_id: None,
        url: None, login_url: None, web_app: None,
        switch_inline_query: None, switch_inline_query_current_chat: None,
        switch_inline_query_chosen_chat: None, copy_text: None,
        callback_game: None, pay: None,
    }
}

fn upscale_keyboard(anime_expanded: bool, active_model: &str) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = vec![];

    // دکمه عمومی
    let general_active = active_model == "realesrgan-x4plus";
    rows.push(vec![btn(
        if general_active { format!("✅ {}", t("upscale.model.general")) } else { format!("🖼️ {}", t("upscale.model.general")) },
        format!("{}{}", CB_UPSCALE_MODEL_PREFIX, "realesrgan-x4plus"),
        if general_active { Some(ButtonStyle::Primary) } else { None },
    )]);

    // دکمه انیمه و کارتون (toggle)
    let anime_header_active = is_anime_model(active_model);
    rows.push(vec![btn(
        if anime_expanded {
            format!("🎨 {} ▲", t("upscale.model.anime_group"))
        } else {
            format!("🎨 {} ▼", t("upscale.model.anime_group"))
        },
        CB_UPSCALE_ANIME_TOGGLE,
        if anime_header_active { Some(ButtonStyle::Primary) } else { None },
    )]);

    // زیرمنوی انیمه
    if anime_expanded {
        for (model_name, _scale, label_key) in ANIME_MODELS {
            let is_active = *model_name == active_model;
            rows.push(vec![btn(
                if is_active { format!("  ✅ {}", t(label_key)) } else { format!("  └ {}", t(label_key)) },
                format!("{}{}", CB_UPSCALE_MODEL_PREFIX, model_name),
                if is_active { Some(ButtonStyle::Primary) } else { None },
            )]);
        }
    }

    // دکمه لغو
    rows.push(vec![btn(t("upscale.cancel_button"), CB_UPSCALE_CANCEL, Some(ButtonStyle::Danger))]);

    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

/// Called from main.rs when `ai:upscale` is pressed.
pub async fn enter_upscale(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    // پیش‌فرض: عمومی x4، منوی انیمه بسته
    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor: 4,
        model_name: "realesrgan-x4plus".to_string(),
    });

    let text = t("upscale.prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard(false, "realesrgan-x4plus"))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => log_trace(trace_id, "upscale_prompt_shown", &format!("user_id={user_id} chat_id={chat_id}")),
        Err(e) => log_trace(trace_id, "upscale_prompt_failed", &e.to_string()),
    }
}

/// Toggle زیرمنوی انیمه (بدون تغییر مدل انتخابی).
pub async fn handle_upscale_anime_toggle(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let (active_model, was_expanded) = match flow_manager.get(user_id) {
        FlowState::AwaitingUpscaleImage { model_name, .. } => {
            let expanded = is_anime_model(&model_name);
            (model_name, expanded)
        }
        _ => ("realesrgan-x4plus".to_string(), false),
    };
    // toggle: اگه بسته بود باز کن، اگه باز بود ببند
    let now_expanded = !was_expanded;
    log_trace(trace_id, "upscale_anime_toggle", &format!(
        "user_id={user_id} expanded={now_expanded} active={active_model}"
    ));
    let text = t("upscale.prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard(now_expanded, &active_model))
        .build();
    let _ = api.edit_message_text(&params).await;
}

/// انتخاب مدل توسط کاربر.
pub async fn handle_upscale_model_pick(
    api: &Bot,
    model_name: &str,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let scale_factor = if model_name == "realesrgan-x4plus" { 4 } else { scale_for_model(model_name) };

    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor,
        model_name: model_name.to_string(),
    });

    log_trace(trace_id, "upscale_model_pick", &format!("user_id={user_id} model={model_name} scale={scale_factor}"));

    // بعد از انتخاب مدل، زیرمنو باز می‌مونه اگه انیمه انتخاب شده
    let expanded = is_anime_model(model_name);
    let text = t("upscale.prompt");
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard(expanded, model_name))
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
