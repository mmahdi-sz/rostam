use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};

use crate::bot::{edit_to_tools, send_text};
use crate::emoji::panel::btn_icon;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{t, tf, apply_premium_to_md};
use crate::log::next_trace_id;

const SEP_BASE: &str = "http://127.0.0.1:6589";

pub const CB_TOOLS_PDF_COMPRESS: &str = "tools:pdf_compress";
pub const CB_PDF_MODE_SIMPLE: &str = "pdf:mode:simple";
pub const CB_PDF_MODE_ADVANCED: &str = "pdf:mode:advanced";
pub const CB_PDF_LEVEL_PREFIX: &str = "pdf:level:";
pub const CB_PDF_CANCEL: &str = "pdf:cancel";

const LEVELS: &[(&str, &str)] = &[
    ("screen", "pdfcompress.level.screen"),
    ("ebook", "pdfcompress.level.ebook"),
    ("printer", "pdfcompress.level.printer"),
    ("prepress", "pdfcompress.level.prepress"),
];

fn mode_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(&t("pdfcompress.mode_simple_button"), CB_PDF_MODE_SIMPLE, "")],
            vec![btn_icon(&t("pdfcompress.mode_advanced_button"), CB_PDF_MODE_ADVANCED, "")],
            vec![btn_icon(&t("start.back"), CB_PDF_CANCEL, "back")],
        ])
        .build()
}

fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(&t("start.back"), CB_PDF_CANCEL, "back")]])
        .build()
}

fn level_keyboard() -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<frankenstein::types::InlineKeyboardButton>> = LEVELS
        .chunks(2)
        .map(|chunk| {
            chunk.iter()
                .map(|(key, label_key)| btn_icon(&t(label_key), &format!("{CB_PDF_LEVEL_PREFIX}{key}"), ""))
                .collect()
        })
        .collect();
    rows.push(vec![btn_icon(&t("start.back"), CB_PDF_CANCEL, "back")]);
    InlineKeyboardMarkup::builder().inline_keyboard(rows).build()
}

// ── menu / mode entry ──────────────────────────────────────────────────────────

pub async fn enter_pdf_compress(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("pdfcompress", trace_id, user_id, "clicked" => "tools:pdf_compress");
    flow_manager.clear(user_id);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id)
        .text(t("pdfcompress.mode_prompt"))
        .reply_markup(mode_keyboard())
        .build();
    let r = api.edit_message_text(&params).await;
    log_ev!("pdfcompress", trace_id, "mode_prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

pub async fn handle_pdf_mode_simple(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("pdfcompress", trace_id, "mode_simple_entered", "user_id" => user_id);
    flow_manager.set(user_id, FlowState::AwaitingPdfCompressFile);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id)
        .text(t("pdfcompress.send_file_prompt"))
        .reply_markup(cancel_keyboard())
        .build();
    let _ = api.edit_message_text(&params).await;
}

pub async fn handle_pdf_cancel(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("pdfcompress", trace_id, "cancel", "user_id" => user_id);
    flow_manager.clear(user_id);
    let _ = edit_to_tools(api, chat_id, message_id).await;
}

// ── file intake ────────────────────────────────────────────────────────────────

fn looks_like_pdf_upload(message: &Message) -> Option<(String, String, u64)> {
    let doc = message.document.as_ref()?;
    let name = doc.file_name.clone().unwrap_or_else(|| "document.pdf".to_string());
    let is_pdf_name = name.to_lowercase().ends_with(".pdf");
    let is_pdf_mime = doc.mime_type.as_deref() == Some("application/pdf");
    if !is_pdf_name && !is_pdf_mime {
        return None;
    }
    let size = doc.file_size.unwrap_or(0) as u64;
    Some((doc.file_id.clone(), name, size))
}

pub async fn handle_pdf_file(
    api: &Bot, message: &Message, user_id: i64, flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("pdfcompress", trace_id, user_id, "clicked" => "send_pdf");

    let Some((file_id, filename, size)) = looks_like_pdf_upload(message) else {
        log_ev!("pdfcompress", trace_id, "not_a_pdf", "=>" => "reject");
        let _ = send_text(api, chat_id, &t("pdfcompress.error.invalid_file")).await;
        return;
    };

    let max_bytes = crate::config::pdf_compress_max_bytes();
    if size > 0 && size > max_bytes {
        log_ev!("pdfcompress", trace_id, "too_large", "size" => size, "max" => max_bytes, "=>" => "reject");
        let text = tf("pdfcompress.error.too_large", &[("max", &fmt_bytes(max_bytes))]);
        let _ = send_text(api, chat_id, &text).await;
        return;
    }

    log_ev!("pdfcompress", trace_id, "file_accepted", "filename" => &filename, "size" => size);
    flow_manager.set(user_id, FlowState::AwaitingPdfCompressLevel { file_id, filename });

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(t("pdfcompress.level_prompt"))
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(level_keyboard()))
        .build();
    let r = api.send_message(&params).await;
    log_ev!("pdfcompress", trace_id, "level_prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

// ── level selection → download + compress ──────────────────────────────────────

pub async fn handle_pdf_level(
    api: &Bot, chat_id: i64, message_id: i32, user_id: i64, flow_manager: &mut FlowManager, level: &str,
) {
    let trace_id = next_trace_id();

    let (file_id, filename) = match flow_manager.get(user_id) {
        FlowState::AwaitingPdfCompressLevel { file_id, filename } => (file_id, filename),
        other => {
            log_ev!("pdfcompress", trace_id, "level_bad_state", "state" => format!("{other:?}"));
            return;
        }
    };
    flow_manager.clear(user_id);

    if !LEVELS.iter().any(|(key, _)| *key == level) {
        log_ev!("pdfcompress", trace_id, "unknown_level", "level" => level);
        return;
    }

    log_ev!("pdfcompress", trace_id, "level_chosen", "level" => level, "filename" => &filename);
    let _ = api.edit_message_text(
        &EditMessageTextParams::builder()
            .chat_id(chat_id).message_id(message_id)
            .text(t("pdfcompress.processing"))
            .build(),
    ).await;

    let api2 = api.clone();
    let level_owned = level.to_string();
    tokio::spawn(async move {
        run_pdf_compress(api2, chat_id, message_id, user_id, file_id, filename, level_owned, trace_id).await;
    });
}

async fn run_pdf_compress(
    api: Bot, chat_id: i64, message_id: i32, user_id: i64,
    file_id: String, filename: String, level: String, trace_id: u64,
) {
    let work_dir = std::env::temp_dir().join(format!("pdfcompress_{trace_id}"));
    std::fs::create_dir_all(&work_dir).ok();
    let input_path = work_dir.join("input.pdf");
    let output_path = work_dir.join("output.pdf");

    log_ev!("pdfcompress", trace_id, "download_start", "filename" => &filename);
    if let Err(e) = download_file(&api, &file_id, &input_path).await.map_err(|e| e.to_string()) {
        log_ev!("pdfcompress", trace_id, "download_failed", "=>" => format!("fail err={e}"));
        std::fs::remove_dir_all(&work_dir).ok();
        crate::stats::record_error_global("pdfcompress", &format!("download failed: {e}")).await;
        let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.download_failed")).await;
        return;
    }

    // Real validation happens on downloaded bytes, not on filename/mime — those
    // are trivially spoofable and gs is never invoked on anything that fails this.
    if !starts_with_pdf_magic(&input_path) {
        log_ev!("pdfcompress", trace_id, "bad_magic_bytes", "=>" => "reject");
        std::fs::remove_dir_all(&work_dir).ok();
        let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.invalid_pdf")).await;
        return;
    }

    let cores = acquire_cpu(user_id, trace_id).await;
    let timeout_secs = crate::config::pdf_compress_timeout_secs();

    log_ev!("pdfcompress", trace_id, "gs_spawn", "level" => &level, "timeout" => timeout_secs);
    let gs_result = run_gs(&input_path, &output_path, &level, timeout_secs, trace_id, &cores).await;
    release_cpu(cores, trace_id).await;

    let before_size = std::fs::metadata(&input_path).map(|m| m.len()).unwrap_or(0);

    match gs_result {
        Ok(()) => {}
        Err(GsError::Timeout) => {
            log_ev!("pdfcompress", trace_id, "gs_timeout");
            std::fs::remove_dir_all(&work_dir).ok();
            crate::stats::record_error_global("pdfcompress", "gs timeout").await;
            let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.timeout")).await;
            return;
        }
        Err(GsError::Failed(msg)) => {
            log_ev!("pdfcompress", trace_id, "gs_failed", "=>" => format!("fail err={msg}"));
            std::fs::remove_dir_all(&work_dir).ok();
            crate::stats::record_error_global("pdfcompress", &format!("gs failed: {msg}")).await;
            let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.gs_failed")).await;
            return;
        }
    }

    let after_size = std::fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
    let percent = if before_size > 0 {
        100.0 - (after_size as f64 / before_size as f64 * 100.0)
    } else {
        0.0
    };
    log_ev!("pdfcompress", trace_id, "gs_done", "before" => before_size, "after" => after_size, "percent" => format!("{percent:.1}"));

    // gs's PDFSETTINGS presets are tuned for scanned/raster-heavy PDFs — on an
    // already well-optimized, mostly-vector/text document they can re-encode
    // images/fonts into something bigger than the source. Never ship a "compressed"
    // file that's actually larger; that's worse than doing nothing.
    if after_size >= before_size {
        log_ev!("pdfcompress", trace_id, "no_improvement", "=>" => "abort_send");
        std::fs::remove_dir_all(&work_dir).ok();
        let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.no_improvement")).await;
        crate::stats::record_event_user(user_id, "pdfcompress", &level, "no_gain", 0).await;
        return;
    }

    let caption = apply_premium_to_md(&format!(
        "{}\n\n{}",
        t("pdfcompress.result_caption"),
        tf("pdfcompress.result_report", &[
            ("before", &escape_md(&fmt_bytes(before_size))),
            ("after", &escape_md(&fmt_bytes(after_size))),
            ("percent", &escape_md(&format!("{percent:.0}"))),
        ]),
    ));
    let doc_params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(PathBuf::from(&output_path))
        .caption(&caption)
        .parse_mode(ParseMode::MarkdownV2)
        .build();
    match api.send_document(&doc_params).await {
        Ok(_) => {
            log_ev!("pdfcompress", trace_id, "result_sent", "=>" => "ok");
            let _ = api.delete_message(
                &frankenstein::methods::DeleteMessageParams::builder()
                    .chat_id(chat_id).message_id(message_id).build(),
            ).await;
            crate::stats::record_event_user(user_id, "pdfcompress", &level, "ok", 0).await;
        }
        Err(e) => {
            log_ev!("pdfcompress", trace_id, "result_send_failed", "=>" => format!("fail err={e}"));
            crate::stats::record_error_global("pdfcompress", &format!("send_document failed: {e}")).await;
            let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.send_failed")).await;
            crate::stats::record_event_user(user_id, "pdfcompress", &level, "fail", 0).await;
        }
    }
    std::fs::remove_dir_all(&work_dir).ok();
}

enum GsError {
    Timeout,
    Failed(String),
}

// TODO: no privilege drop / rlimit sandboxing around this subprocess yet (low-priv
// uid or an rlimit crate to cap memory/CPU). Magic-byte validation + timeout are in
// place as the minimum bar; revisit if PDF compression becomes a bigger attack surface.
async fn run_gs(
    input: &std::path::Path, output: &std::path::Path, level: &str,
    timeout_secs: u64, trace_id: u64, cores: &[i32],
) -> Result<(), GsError> {
    let mut cmd = tokio::process::Command::new("gs");
    cmd.arg("-sDEVICE=pdfwrite")
        .arg("-dCompatibilityLevel=1.4")
        .arg(format!("-dPDFSETTINGS=/{level}"))
        .arg("-dSAFER")
        .arg("-dNOPAUSE")
        .arg("-dBATCH")
        .arg("-dQUIET")
        .arg("-dDetectDuplicateImages=true")
        .arg("-dRemoveUnusedResources=true")
        .arg("-dSubsetFonts=true")
        .arg("-dCompressFonts=true")
        .arg(format!("-sOutputFile={}", output.display()))
        .arg(input)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| GsError::Failed(format!("spawn: {e}")))?;

    if !cores.is_empty() {
        if let Some(pid) = child.id() {
            pin_pid_to_cores(pid, cores, trace_id);
        }
    }

    let wait = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let status = match wait {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(GsError::Failed(format!("wait: {e}"))),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await; // reap — avoid a zombie process
            return Err(GsError::Timeout);
        }
    };

    log_ev!("pdfcompress", trace_id, "gs_exit", "status" => status);
    if !status.success() {
        return Err(GsError::Failed(format!("exit {status}")));
    }
    if !output.exists() {
        return Err(GsError::Failed("no output".to_string()));
    }
    Ok(())
}

fn pin_pid_to_cores(pid: u32, cores: &[i32], trace_id: u64) {
    if cores.is_empty() { return; }
    let cores_str = cores.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",");
    let status = std::process::Command::new("taskset")
        .arg("-cp")
        .arg(&cores_str)
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    log_ev!("pdfcompress", trace_id, "pin_pid", "pid" => pid, "cores" => &cores_str, "status" => status.is_ok());
}

fn starts_with_pdf_magic(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut buf = [0u8; 5];
    if f.read_exact(&mut buf).is_err() { return false; }
    &buf == b"%PDF-"
}

fn escape_md(s: &str) -> String {
    s.chars().map(|c| match c {
        '*' | '\\' | '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>'
        | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' => format!("\\{c}"),
        other => other.to_string(),
    }).collect()
}

fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else {
        format!("{:.1} MB", mb)
    }
}

async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: &str) {
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id).message_id(message_id).text(text).build();
    let _ = api.edit_message_text(&params).await;
}

async fn download_file(api: &Bot, file_id: &str, dest: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
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

// ── CPU broker (same pattern as upscale/asr) ────────────────────────────────────

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

async fn acquire_cpu(user_id: i64, trace_id: u64) -> Vec<i32> {
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
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
            log_ev!("pdfcompress", trace_id, "cpu_acquired", "cores" => format!("{cores:?}"));
            cores
        }
        Err(e) => {
            log_ev!("pdfcompress", trace_id, "cpu_acquire_failed", "=>" => format!("fail err={e}"));
            vec![]
        }
    }
}

async fn release_cpu(cores: Vec<i32>, trace_id: u64) {
    if cores.is_empty() { return; }
    let client = HTTP_CLIENT.get_or_init(reqwest::Client::new);
    let body = serde_json::json!({ "cores": cores });
    let r = client
        .post(format!("{SEP_BASE}/cpu/release"))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    log_ev!("pdfcompress", trace_id, "cpu_released", "cores" => format!("{cores:?}"), "=>" => if r.is_ok() { "ok" } else { "fail" });
}
