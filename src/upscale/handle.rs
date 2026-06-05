use std::path::PathBuf;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendPhotoParams},
    types::{InlineKeyboardMarkup, Message},
};

use crate::bot::send_text;
use crate::emoji::{FlowManager, FlowState};
use crate::emoji::panel::{btn_icon, btn_icon_success, btn_icon_danger};
use crate::i18n::{t, tf, apply_premium_to_md};
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

// (model_name, scale, i18n_key)
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

fn upscale_keyboard(anime_expanded: bool, active_model: &str) -> InlineKeyboardMarkup {
    use frankenstein::types::InlineKeyboardButton;
    let mut rows: Vec<Vec<InlineKeyboardButton>> = vec![];

    let general_active = active_model == "realesrgan-x4plus";
    let general_cb = format!("{}{}", CB_UPSCALE_MODEL_PREFIX, "realesrgan-x4plus");
    rows.push(vec![if general_active {
        btn_icon_success(&t("upscale.model.general"), &general_cb, "sparkles")
    } else {
        btn_icon(&t("upscale.model.general"), &general_cb, "sparkles")
    }]);

    let anime_header_active = is_anime_model(active_model);
    let anime_label = if anime_expanded {
        format!("{} ▲", t("upscale.model.anime_group"))
    } else {
        format!("{} ▼", t("upscale.model.anime_group"))
    };
    rows.push(vec![if anime_header_active {
        btn_icon_success(&anime_label, CB_UPSCALE_ANIME_TOGGLE, "quality_high")
    } else {
        btn_icon(&anime_label, CB_UPSCALE_ANIME_TOGGLE, "quality_high")
    }]);

    if anime_expanded {
        for (model_name, _scale, label_key) in ANIME_MODELS {
            let is_active = *model_name == active_model;
            let cb = format!("{}{}", CB_UPSCALE_MODEL_PREFIX, model_name);
            rows.push(vec![if is_active {
                btn_icon_success(&t(label_key), &cb, "")
            } else {
                btn_icon(&t(label_key), &cb, "")
            }]);
        }
    }

    rows.push(vec![btn_icon_danger(&t("upscale.cancel_button"), CB_UPSCALE_CANCEL, "cancel")]);

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
    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor: 4,
        model_name: "realesrgan-x4plus".to_string(),
        anime_expanded: false,
    });

    eprintln!("[upscale trace={trace_id} event=enter] user_id={user_id} chat_id={chat_id} model=realesrgan-x4plus anime_expanded=false");

    let text = apply_premium_to_md(&t("upscale.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard(false, "realesrgan-x4plus"))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[upscale trace={trace_id} event=prompt_shown] ok"),
        Err(e) => eprintln!("[upscale trace={trace_id} event=prompt_failed] err={e}"),
    }
}

/// Toggle زیرمنوی انیمه — state واقعی از FlowState خونده می‌شه.
pub async fn handle_upscale_anime_toggle(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();

    let (active_model, was_expanded, scale_factor) = match flow_manager.get(user_id) {
        FlowState::AwaitingUpscaleImage { model_name, anime_expanded, scale_factor } => {
            eprintln!("[upscale trace={trace_id} event=anime_toggle_read_state] user_id={user_id} model={model_name} anime_expanded={anime_expanded} scale={scale_factor}");
            (model_name, anime_expanded, scale_factor)
        }
        other => {
            eprintln!("[upscale trace={trace_id} event=anime_toggle_wrong_state] user_id={user_id} state={other:?} — defaulting");
            ("realesrgan-x4plus".to_string(), false, 4u32)
        }
    };

    let now_expanded = !was_expanded;
    eprintln!("[upscale trace={trace_id} event=anime_toggle] user_id={user_id} was_expanded={was_expanded} now_expanded={now_expanded} model={active_model}");

    // State رو با expanded جدید ذخیره می‌کنیم
    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor,
        model_name: active_model.clone(),
        anime_expanded: now_expanded,
    });

    let text = apply_premium_to_md(&t("upscale.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard(now_expanded, &active_model))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[upscale trace={trace_id} event=anime_toggle_keyboard_updated] now_expanded={now_expanded}"),
        Err(e) => eprintln!("[upscale trace={trace_id} event=anime_toggle_edit_failed] err={e}"),
    }
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
    let is_anime = is_anime_model(model_name);

    // بعد از انتخاب مدل انیمه، submenu باز می‌مونه؛ انتخاب عمومی می‌بنده
    let anime_expanded = is_anime;

    eprintln!("[upscale trace={trace_id} event=model_pick] user_id={user_id} model={model_name} scale={scale_factor} is_anime={is_anime} anime_expanded={anime_expanded}");

    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor,
        model_name: model_name.to_string(),
        anime_expanded,
    });

    let text = apply_premium_to_md(&t("upscale.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard(anime_expanded, model_name))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[upscale trace={trace_id} event=model_pick_keyboard_updated] model={model_name} anime_expanded={anime_expanded}"),
        Err(e) => eprintln!("[upscale trace={trace_id} event=model_pick_edit_failed] err={e}"),
    }
}

/// Handles upscale cancel callback — back to AI Lab.
pub async fn handle_upscale_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    eprintln!("[upscale trace={trace_id} event=cancel] user_id={user_id} chat_id={chat_id}");
    flow_manager.clear(user_id);
    let r = crate::bot::edit_to_ai_lab(api, chat_id, message_id).await;
    eprintln!("[upscale trace={trace_id} event=cancel_done] ok={}", r.is_ok());
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

    eprintln!("[upscale trace={trace_id} event=image_received] user_id={user_id} chat_id={chat_id} model={model_name} scale={scale_factor} has_photo={} has_doc={}",
        message.photo.is_some(), message.document.is_some());

    // Get file_id from photo (largest) or document
    let file_id = message
        .photo
        .as_ref()
        .and_then(|photos| photos.last())
        .map(|p| &p.file_id)
        .or_else(|| message.document.as_ref().map(|d| &d.file_id));

    let Some(file_id) = file_id else {
        eprintln!("[upscale trace={trace_id} event=no_file_id]");
        let _ = send_text(api, chat_id, &t("upscale.unsupported_format")).await;
        return;
    };

    let is_doc = message.document.is_some();
    let orig_ext = if is_doc {
        detect_doc_ext(message)
    } else {
        "jpg".to_string()
    };

    eprintln!("[upscale trace={trace_id} event=file_info] file_id={file_id} is_doc={is_doc} ext={orig_ext}");

    let _ = send_text(api, chat_id, &t("upscale.preparing")).await;

    let work_dir = std::env::temp_dir().join(format!("upscale_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();

    let input_path = work_dir.join(format!("input.{orig_ext}"));
    let output_path = work_dir.join(format!("output.{orig_ext}"));

    eprintln!("[upscale trace={trace_id} event=work_dir] path={}", work_dir.display());

    // 1. Download
    eprintln!("[upscale trace={trace_id} event=download_start] file_id={file_id} dest={}", input_path.display());
    if let Err(e) = download_file(api, file_id, input_path.to_str().unwrap()).await {
        eprintln!("[upscale trace={trace_id} event=download_failed] err={e}");
        let _ = send_text(api, chat_id, &t("upscale.download_failed")).await;
        clean_up(&work_dir);
        return;
    }
    let file_size = std::fs::metadata(input_path.to_str().unwrap()).map(|m| m.len()).unwrap_or(0);
    eprintln!("[upscale trace={trace_id} event=download_done] size={file_size}");

    // 2. Run upscale — blocking binary, offload to threadpool so async runtime stays free
    eprintln!("[upscale trace={trace_id} event=upscale_start] model={model_name} scale={scale_factor} input={} output={}", input_path.display(), output_path.display());
    let input_str = input_path.to_str().unwrap().to_string();
    let output_str = output_path.to_str().unwrap().to_string();
    let model_name_owned = model_name.to_string();
    let processing_secs = match tokio::task::spawn_blocking(move || {
        run_upscale(&input_str, &output_str, &model_name_owned, scale_factor, trace_id)
    }).await {
        Ok(Ok(s)) => {
            eprintln!("[upscale trace={trace_id} event=upscale_done] elapsed={s:.1}s");
            s
        }
        Ok(Err(e)) => {
            eprintln!("[upscale trace={trace_id} event=upscale_failed] err={e}");
            let _ = send_text(api, chat_id, &t("upscale.upscale_failed")).await;
            clean_up(&work_dir);
            return;
        }
        Err(e) => {
            eprintln!("[upscale trace={trace_id} event=upscale_spawn_failed] err={e}");
            let _ = send_text(api, chat_id, &t("upscale.upscale_failed")).await;
            clean_up(&work_dir);
            return;
        }
    };

    let output_size = std::fs::metadata(output_path.to_str().unwrap()).map(|m| m.len()).unwrap_or(0);
    eprintln!("[upscale trace={trace_id} event=output_ready] size={output_size} is_doc={is_doc} ext={orig_ext}");

    // 3. Send upscaled image
    let scale_str = escape_md(&format!("{}x", scale_factor));
    let processing_str = escape_md(&format!("{:.1}", processing_secs));
    let full_caption = apply_premium_to_md(&format!(
        "{}\n\n{}",
        t("upscale.result_caption"),
        tf("upscale.report", &[("scale", &scale_str), ("processing", &processing_str)]),
    ));

    if is_doc || orig_ext != "jpg" {
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&full_caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = api.send_document(&params).await;
        eprintln!("[upscale trace={trace_id} event=send_doc] ok={}", r.is_ok());
        if let Err(e) = r { eprintln!("[upscale trace={trace_id} event=send_doc_err] err={e}"); }
    } else {
        let params = SendPhotoParams::builder()
            .chat_id(chat_id)
            .photo(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&full_caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        let r = api.send_photo(&params).await;
        eprintln!("[upscale trace={trace_id} event=send_photo] ok={}", r.is_ok());
        if let Err(e) = r { eprintln!("[upscale trace={trace_id} event=send_photo_err] err={e}"); }
    }

    clean_up(&work_dir);
    eprintln!("[upscale trace={trace_id} event=cleanup_done]");
}

fn run_upscale(input: &str, output: &str, model_name: &str, scale: u32, trace_id: u64) -> Result<f64, String> {
    use std::time::Instant;
    let start = Instant::now();

    eprintln!("[upscale trace={trace_id} event=realesrgan_spawn] bin={UPSCALE_BIN} model={model_name} scale={scale}");

    let result = std::process::Command::new(UPSCALE_BIN)
        .args([
            "-i", input,
            "-o", output,
            "-n", model_name,
            "-s", &scale.to_string(),
            "-m", MODEL_DIR,
        ])
        .output()
        .map_err(|e| format!("realesrgan spawn failed: {e}"))?;

    let elapsed = start.elapsed().as_secs_f64();
    eprintln!("[upscale trace={trace_id} event=realesrgan_exit] status={} elapsed={elapsed:.1}s stderr_bytes={}",
        result.status, result.stderr.len());

    if !result.stderr.is_empty() {
        let stderr_preview = String::from_utf8_lossy(&result.stderr);
        let preview: String = stderr_preview.chars().take(300).collect();
        eprintln!("[upscale trace={trace_id} event=realesrgan_stderr] {preview}");
    }

    if !result.status.success() {
        return Err(format!("realesrgan exited with status {}", result.status));
    }

    if !std::path::Path::new(output).exists() {
        return Err("realesrgan did not produce output file".into());
    }

    Ok(elapsed)
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

    eprintln!("[upscale event=download_file_path] file_path={file_path}");

    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        eprintln!("[upscale event=download_local_copy] size={size}");
        return Ok(());
    }

    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{}/{file_path}", crate::config::bot_token()?)
    } else {
        format!("https://api.telegram.org/file/bot{}/{file_path}", crate::config::bot_token()?)
    };

    eprintln!("[upscale event=download_http] url_prefix={}", &url[..url.len().min(60)]);
    let response = reqwest::get(&url).await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    eprintln!("[upscale event=download_http_done] status={status} bytes={}", bytes.len());
    std::fs::write(dest, &bytes)?;
    Ok(())
}

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
