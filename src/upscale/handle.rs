use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendMessageParams, SendPhotoParams},
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};

use crate::bot::send_text;
use crate::emoji::{FlowManager, FlowState};
use crate::emoji::panel::{btn_icon, btn_icon_plain, btn_icon_success, btn_icon_danger};
use crate::i18n::{t, tf, apply_premium_to_md};
use crate::youtube::log_trace;

const UPSCALE_BIN: &str = "files/realesrgan/realesrgan-ncnn-vulkan";
const MODEL_DIR: &str = "files/realesrgan/models";
const SEP_BASE: &str = "http://127.0.0.1:6589";

fn next_trace_id() -> u64 {
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub const CB_UPSCALE_CANCEL: &str = "upscale:cancel";
pub const CB_UPSCALE_MODEL_PREFIX: &str = "upscale:model:";
pub const CB_UPSCALE_ANIME_TOGGLE: &str = "upscale:anime_toggle";

const ANIME_MODELS: &[(&str, u32, &str)] = &[
    ("realesrgan-x4plus-anime", 4, "upscale.model.anime_pro"),
    ("realesr-animevideov3-x4", 4, "upscale.model.anime_x4"),
    ("realesr-animevideov3-x3", 3, "upscale.model.anime_x3"),
    ("realesr-animevideov3-x2", 2, "upscale.model.anime_x2"),
];

// ── active cancel flags ───────────────────────────────────────────────────────
static ACTIVE_UPSCALES: OnceLock<Mutex<HashMap<i64, Arc<AtomicBool>>>> = OnceLock::new();

fn active_upscales() -> &'static Mutex<HashMap<i64, Arc<AtomicBool>>> {
    ACTIVE_UPSCALES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_upscale(user_id: i64) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    active_upscales().lock().unwrap().insert(user_id, flag.clone());
    flag
}

fn unregister_upscale(user_id: i64) {
    active_upscales().lock().unwrap().remove(&user_id);
}

pub fn cancel_upscale(user_id: i64) -> bool {
    if let Some(flag) = active_upscales().lock().unwrap().get(&user_id) {
        flag.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
}

// ── CPU broker ────────────────────────────────────────────────────────────────

async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{SEP_BASE}/cpu/acquire"))
        .form(&[("user_id", user_id.to_string()), ("is_vip", "false".to_string())])
        .timeout(Duration::from_secs(120))
        .send()
        .await;
    match res {
        Ok(r) => {
            let json: serde_json::Value = r.json().await.unwrap_or_default();
            let cores: Vec<i32> = json
                .get("cores")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            eprintln!("[upscale trace={trace_id} event=cpu_acquired] cores={cores:?}");
            cores
        }
        Err(e) => {
            eprintln!("[upscale trace={trace_id} event=cpu_acquire_failed] err={e}");
            vec![]
        }
    }
}

async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    if cores.is_empty() { return; }
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "cores": cores });
    let r = client
        .post(format!("{SEP_BASE}/cpu/release"))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    eprintln!("[upscale trace={trace_id} event=cpu_released] cores={cores:?} ok={}", r.is_ok());
}

// Pin a subprocess (by PID) to the given CPU core list via sched_setaffinity.
fn pin_pid_to_cores(pid: Option<u32>, cores: &[i32], trace_id: u64) {
    let Some(pid) = pid else { return; };
    #[cfg(target_os = "linux")]
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for &c in cores {
            if c >= 0 && (c as usize) < libc::CPU_SETSIZE as usize {
                libc::CPU_SET(c as usize, &mut set);
            }
        }
        let ret = libc::sched_setaffinity(
            pid as libc::pid_t,
            std::mem::size_of::<libc::cpu_set_t>(),
            &set,
        );
        eprintln!("[upscale trace={trace_id} event=pin_affinity] pid={pid} cores={cores:?} ret={ret}");
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, cores, trace_id);
    }
}

// ── keyboards ─────────────────────────────────────────────────────────────────

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
        btn_icon_plain(&t("upscale.model.general"), &general_cb, "sparkles")
    }]);

    let anime_label = if anime_expanded {
        format!("{} ▲", t("upscale.model.anime_group"))
    } else {
        format!("{} ▼", t("upscale.model.anime_group"))
    };
    rows.push(vec![btn_icon_plain(&anime_label, CB_UPSCALE_ANIME_TOGGLE, "quality_high")]);

    if anime_expanded {
        for (model_name, _scale, label_key) in ANIME_MODELS {
            let is_active = *model_name == active_model;
            let cb = format!("{}{}", CB_UPSCALE_MODEL_PREFIX, model_name);
            rows.push(vec![if is_active {
                btn_icon_success(&t(label_key), &cb, "")
            } else {
                btn_icon_plain(&t(label_key), &cb, "")
            }]);
        }
    }

    rows.push(vec![btn_icon_danger(&t("upscale.cancel_button"), CB_UPSCALE_CANCEL, "cancel")]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

fn upscale_status_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![
            btn_icon_danger(&t("upscale.cancel_button"), CB_UPSCALE_CANCEL, "cancel"),
        ]])
        .build()
}

// ── entry / model selection ───────────────────────────────────────────────────

pub async fn enter_upscale(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor: 4, model_name: "realesrgan-x4plus".to_string(), anime_expanded: false,
    });
    eprintln!("[upscale trace={trace_id} event=enter] user_id={user_id} chat_id={chat_id}");
    let text = apply_premium_to_md(&t("upscale.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id).text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(upscale_keyboard(false, "realesrgan-x4plus"))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[upscale trace={trace_id} event=prompt_shown]"),
        Err(e) => eprintln!("[upscale trace={trace_id} event=prompt_failed] err={e}"),
    }
}

pub async fn handle_upscale_anime_toggle(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let (active_model, was_expanded, scale_factor) = match flow_manager.get(user_id) {
        FlowState::AwaitingUpscaleImage { model_name, anime_expanded, scale_factor } =>
            (model_name, anime_expanded, scale_factor),
        _ => ("realesrgan-x4plus".to_string(), false, 4u32),
    };
    let now_expanded = !was_expanded;
    eprintln!("[upscale trace={trace_id} event=anime_toggle] now_expanded={now_expanded}");
    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor, model_name: active_model.clone(), anime_expanded: now_expanded,
    });
    let text = apply_premium_to_md(&t("upscale.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id).text(&text)
        .parse_mode(ParseMode::MarkdownV2).reply_markup(upscale_keyboard(now_expanded, &active_model))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[upscale trace={trace_id} event=toggle_done]"),
        Err(e) => eprintln!("[upscale trace={trace_id} event=toggle_failed] err={e}"),
    }
}

pub async fn handle_upscale_model_pick(
    api: &Bot, model_name: &str, chat_id: i64, message_id: i32, user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let scale_factor = if model_name == "realesrgan-x4plus" { 4 } else { scale_for_model(model_name) };
    let anime_expanded = is_anime_model(model_name);
    eprintln!("[upscale trace={trace_id} event=model_pick] model={model_name}");
    flow_manager.set(user_id, FlowState::AwaitingUpscaleImage {
        scale_factor, model_name: model_name.to_string(), anime_expanded,
    });
    let text = apply_premium_to_md(&t("upscale.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id).text(&text)
        .parse_mode(ParseMode::MarkdownV2).reply_markup(upscale_keyboard(anime_expanded, model_name))
        .build();
    match api.edit_message_text(&params).await {
        Ok(_) => eprintln!("[upscale trace={trace_id} event=model_pick_done]"),
        Err(e) if e.to_string().contains("message is not modified") => {},
        Err(e) => eprintln!("[upscale trace={trace_id} event=model_pick_failed] err={e}"),
    }
}

pub async fn handle_upscale_cancel(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    if cancel_upscale(user_id) {
        // Active processing — main task detects the flag and edits the message itself.
        eprintln!("[upscale trace={trace_id} event=cancel_active] user_id={user_id}");
        return;
    }
    // Model-selection screen — go back to AI Lab.
    eprintln!("[upscale trace={trace_id} event=cancel_prompt] user_id={user_id}");

    let r = crate::bot::edit_to_ai_lab(api, chat_id, message_id).await;
    eprintln!("[upscale trace={trace_id} event=cancel_done] ok={}", r.is_ok());
}

// ── main processing ───────────────────────────────────────────────────────────

pub async fn handle_upscale_image(
    api: Bot, message: frankenstein::types::Message, user_id: i64, scale_factor: u32,
    model_name: String,
) {

    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    let api = &api;
    eprintln!("[upscale trace={trace_id} event=image_received] user_id={user_id} model={model_name} scale={scale_factor}");

    let file_id = message
        .photo.as_ref().and_then(|p| p.last()).map(|p| &p.file_id)
        .or_else(|| message.document.as_ref().map(|d| &d.file_id));
    let Some(file_id) = file_id else {
        let _ = send_text(api, chat_id, &t("upscale.unsupported_format")).await;
        return;
    };
    let is_doc  = message.document.is_some();
    let orig_ext = if is_doc { detect_doc_ext(&message) } else { "jpg".to_string() };

    // ── status message with cancel button ─────────────────────────────────────
    let status_msg_id: Option<i32> = {
        let params = SendMessageParams::builder()
            .chat_id(chat_id).text(t("upscale.preparing"))
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(upscale_status_keyboard()))
            .build();
        api.send_message(&params).await.ok().map(|r| r.result.message_id)
    };
    eprintln!("[upscale trace={trace_id} event=status_sent] msg_id={status_msg_id:?}");

    // ── cancel flag + elapsed timer ───────────────────────────────────────────
    let cancel_flag = register_upscale(user_id);
    let done_flag   = Arc::new(AtomicBool::new(false));

    if let Some(smid) = status_msg_id {
        let api_t    = api.clone();
        let done_t   = done_flag.clone();
        let cancel_t = cancel_flag.clone();
        let start_t  = std::time::Instant::now();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if done_t.load(Ordering::Relaxed) || cancel_t.load(Ordering::Relaxed) { break; }
                let s = start_t.elapsed().as_secs();
                let elapsed = format!("{:02}:{:02}", s / 60, s % 60);
                let text = tf("upscale.processing", &[("elapsed", &elapsed)]);
                let params = EditMessageTextParams::builder()
                    .chat_id(chat_id).message_id(smid).text(&text)
                    .reply_markup(upscale_status_keyboard()).build();
                let _ = api_t.edit_message_text(&params).await;
            }
        });
    }

    // ── acquire CPU cores ─────────────────────────────────────────────────────
    let cores = acquire_cpu(user_id, trace_id).await;

    // ── download ──────────────────────────────────────────────────────────────
    let work_dir    = std::env::temp_dir().join(format!("upscale_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();
    let input_path  = work_dir.join(format!("input.{orig_ext}"));
    let output_path = work_dir.join(format!("output.{orig_ext}"));

    let dl_result = download_file(api, file_id, input_path.to_str().unwrap()).await;
    if let Err(e) = dl_result.map_err(|e| e.to_string()) {
        eprintln!("[upscale trace={trace_id} event=download_failed] err={e}");
        done_flag.store(true, Ordering::Relaxed);
        unregister_upscale(user_id);
        release_cpu(cores, trace_id).await;
        edit_or_send(api, chat_id, status_msg_id, &t("upscale.download_failed")).await;
        clean_up(&work_dir);
        return;
    }

    // ── run realesrgan ────────────────────────────────────────────────────────
    let input_str      = input_path.to_str().unwrap().to_string();
    let output_str     = output_path.to_str().unwrap().to_string();
    let model_owned    = model_name.to_string();
    let cancel_for_run = cancel_flag.clone();
    let cores_for_run  = cores.clone();

    let result = tokio::task::spawn_blocking(move || {
        run_upscale(&input_str, &output_str, &model_owned, scale_factor, trace_id, cancel_for_run, &cores_for_run)
    }).await;

    done_flag.store(true, Ordering::Relaxed);
    unregister_upscale(user_id);
    release_cpu(cores, trace_id).await;

    let processing_secs = match result {
        Ok(Ok(s)) => {
            eprintln!("[upscale trace={trace_id} event=upscale_done] elapsed={s:.1}s");
            s
        }
        Ok(Err(ref e)) if e == "cancelled" => {
            eprintln!("[upscale trace={trace_id} event=upscale_cancelled]");
            edit_or_send(api, chat_id, status_msg_id, &t("upscale.cancelled")).await;
            clean_up(&work_dir);
            return;
        }
        Ok(Err(e)) => {
            eprintln!("[upscale trace={trace_id} event=upscale_failed] err={e}");
            edit_or_send(api, chat_id, status_msg_id, &t("upscale.upscale_failed")).await;
            clean_up(&work_dir);
            return;
        }
        Err(e) => {
            eprintln!("[upscale trace={trace_id} event=spawn_failed] err={e}");
            edit_or_send(api, chat_id, status_msg_id, &t("upscale.upscale_failed")).await;
            clean_up(&work_dir);
            return;
        }
    };

    // ── delete status, send result ────────────────────────────────────────────
    if let Some(smid) = status_msg_id {
        let _ = api.delete_message(
            &frankenstein::methods::DeleteMessageParams::builder()
                .chat_id(chat_id).message_id(smid).build()
        ).await;
    }

    let scale_str      = escape_md(&format!("{}x", scale_factor));
    let processing_str = escape_md(&format!("{:.1}", processing_secs));
    let full_caption   = apply_premium_to_md(&format!(
        "{}\n\n{}",
        t("upscale.result_caption"),
        tf("upscale.report", &[("scale", &scale_str), ("processing", &processing_str)]),
    ));

    if is_doc || orig_ext != "jpg" {
        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&full_caption).parse_mode(ParseMode::MarkdownV2).build();
        let r = api.send_document(&params).await;
        eprintln!("[upscale trace={trace_id} event=send_doc] ok={}", r.is_ok());
    } else {
        let params = SendPhotoParams::builder()
            .chat_id(chat_id)
            .photo(PathBuf::from(output_path.to_str().unwrap()))
            .caption(&full_caption).parse_mode(ParseMode::MarkdownV2).build();
        let r = api.send_photo(&params).await;
        eprintln!("[upscale trace={trace_id} event=send_photo] ok={}", r.is_ok());
    }

    clean_up(&work_dir);
    eprintln!("[upscale trace={trace_id} event=done]");
}

// ── small helpers ─────────────────────────────────────────────────────────────

async fn edit_or_send(api: &Bot, chat_id: i64, msg_id: Option<i32>, text: &str) {
    if let Some(mid) = msg_id {
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id).message_id(mid).text(text).build();
        let _ = api.edit_message_text(&params).await;
    } else {
        let _ = send_text(api, chat_id, text).await;
    }
}

fn run_upscale(
    input: &str, output: &str, model_name: &str,
    scale: u32, trace_id: u64, cancel: Arc<AtomicBool>, cores: &[i32],
) -> Result<f64, String> {
    use std::time::Instant;
    let start = Instant::now();
    eprintln!("[upscale trace={trace_id} event=realesrgan_spawn] model={model_name} scale={scale}");
    let mut child = std::process::Command::new(UPSCALE_BIN)
        .args(["-i", input, "-o", output, "-n", model_name, "-s", &scale.to_string(), "-m", MODEL_DIR])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;

    if !cores.is_empty() {
        pin_pid_to_cores(Some(child.id()), cores, trace_id);
    }

    loop {
        if cancel.load(Ordering::Relaxed) {
            child.kill().ok();
            child.wait().ok();
            eprintln!("[upscale trace={trace_id} event=realesrgan_killed]");
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let elapsed = start.elapsed().as_secs_f64();
                eprintln!("[upscale trace={trace_id} event=realesrgan_exit] status={status} elapsed={elapsed:.1}s");
                if !status.success() { return Err(format!("exit {status}")); }
                if !std::path::Path::new(output).exists() { return Err("no output".into()); }
                return Ok(elapsed);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e)   => return Err(format!("wait: {e}")),
        }
    }
}


fn detect_doc_ext(message: &Message) -> String {
    if let Some(doc) = &message.document {
        if let Some(name) = &doc.file_name {
            if let Some(ext) = name.rsplit('.').next() { return ext.to_lowercase(); }
        }
        if let Some(mime) = &doc.mime_type {
            return match mime.as_str() {
                "image/jpeg" | "image/jpg" => "jpg",
                "image/png"  => "png",
                "image/webp" => "webp",
                "image/bmp"  => "bmp",
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
    if file_path.starts_with('/') {
        std::fs::copy(&file_path, dest)?;
        return Ok(());
    }
    let url = if let Some(base) = crate::config::bot_api_base_url() {
        let base = base.trim_end_matches('/');
        format!("{base}/file/bot{}/{file_path}", crate::config::bot_token()?)
    } else {
        format!("https://api.telegram.org/file/bot{}/{file_path}", crate::config::bot_token()?)
    };
    let bytes = reqwest::get(&url).await?.bytes().await?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

fn escape_md(s: &str) -> String {
    s.chars().map(|c| match c {
        '*' | '\\' | '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>'
        | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' => format!("\\{c}"),
        other => other.to_string(),
    }).collect()
}

fn clean_up(dir: &std::path::Path) {
    std::fs::remove_dir_all(dir).ok();
}
