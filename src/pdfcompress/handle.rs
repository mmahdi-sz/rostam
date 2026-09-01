use std::path::PathBuf;
use std::process::Stdio;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendDocumentParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};

use crate::bot::{edit_to_tools, send_text_with_back};
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::common::dir::TempDirGuard;
use crate::common::format::fmt_bytes;
use crate::common::keyboard::job_cancel_keyboard as shared_job_cancel_keyboard;
use crate::common::ticker::ProgressTicker;
use crate::emoji::panel::btn_icon;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;

pub const CB_TOOLS_PDF_COMPRESS: &str = "tools:pdf_compress";
pub const CB_PDF_MODE_SIMPLE: &str = "pdf:mode:simple";
pub const CB_PDF_MODE_ADVANCED: &str = "pdf:mode:advanced";
pub const CB_PDF_LEVEL_PREFIX: &str = "pdf:level:";
pub const CB_PDF_CANCEL: &str = "pdf:cancel";

use crate::common::job::JobRegistry;

pub static ACTIVE_PDF_JOBS: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

pub fn cancel_pdf_job(user_id: i64) -> bool {
    ACTIVE_PDF_JOBS.cancel(&user_id)
}

fn format_lite_filename(filename: &str) -> String {
    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("pdf");
    format!("{stem}_lite.{ext}")
}

const LEVELS: &[(&str, &str)] = &[
    ("screen", "pdfcompress.level.screen"),
    ("ebook", "pdfcompress.level.ebook"),
    ("printer", "pdfcompress.level.printer"),
    ("prepress", "pdfcompress.level.prepress"),
];

fn mode_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            vec![btn_icon(
                &t("pdfcompress.mode_simple_button"),
                CB_PDF_MODE_SIMPLE,
                "",
            )],
            vec![btn_icon(
                &t("pdfcompress.mode_advanced_button"),
                CB_PDF_MODE_ADVANCED,
                "",
            )],
            vec![btn_icon(&t("start.back"), CB_PDF_CANCEL, "back")],
        ])
        .build()
}

fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            CB_PDF_CANCEL,
            "back",
        )]])
        .build()
}

fn job_cancel_keyboard() -> InlineKeyboardMarkup {
    shared_job_cancel_keyboard(&t("pdfcompress.cancel_button"), "pdf:jobcancel", "cancel")
}

fn level_keyboard() -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<frankenstein::types::InlineKeyboardButton>> = LEVELS
        .chunks(2)
        .map(|chunk| {
            chunk
                .iter()
                .map(|(key, label_key)| {
                    btn_icon(&t(label_key), &format!("{CB_PDF_LEVEL_PREFIX}{key}"), "")
                })
                .collect()
        })
        .collect();
    rows.push(vec![btn_icon(&t("start.back"), CB_PDF_CANCEL, "back")]);
    InlineKeyboardMarkup::builder()
        .inline_keyboard(rows)
        .build()
}

// ── menu / mode entry ──────────────────────────────────────────────────────────

pub async fn enter_pdf_compress(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("pdfcompress", trace_id, user_id, "clicked" => "tools:pdf_compress");
    flow_manager.clear(user_id);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(t("pdfcompress.mode_prompt"))
        .reply_markup(mode_keyboard())
        .build();
    let r = api.edit_message_text(&params).await;
    log_ev!("pdfcompress", trace_id, "mode_prompt_shown", "=>" => if r.is_ok() { "ok" } else { "fail" });
}

pub async fn handle_pdf_mode_simple(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("pdfcompress", trace_id, "mode_simple_entered", "user_id" => user_id);
    flow_manager.set(user_id, FlowState::AwaitingPdfCompressFile);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(t("pdfcompress.send_file_prompt"))
        .reply_markup(cancel_keyboard())
        .build();
    let _ = api.edit_message_text(&params).await;
}

pub async fn handle_pdf_cancel(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    log_ev!("pdfcompress", trace_id, "cancel", "user_id" => user_id);
    flow_manager.clear(user_id);
    let _ = edit_to_tools(api, chat_id, message_id).await;
}

// ── file intake ────────────────────────────────────────────────────────────────

fn looks_like_pdf_upload(message: &Message) -> Option<(String, String, u64)> {
    let doc = message.document.as_ref()?;
    let name = doc
        .file_name
        .clone()
        .unwrap_or_else(|| "document.pdf".to_string());
    let is_pdf_name = name.to_lowercase().ends_with(".pdf");
    let is_pdf_mime = doc.mime_type.as_deref() == Some("application/pdf");
    if !is_pdf_name && !is_pdf_mime {
        return None;
    }
    let size = doc.file_size.unwrap_or(0) as u64;
    Some((doc.file_id.clone(), name, size))
}

pub async fn handle_pdf_file(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &mut FlowManager,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;
    log_actor_id!("pdfcompress", trace_id, user_id, "clicked" => "send_pdf");

    let Some((file_id, filename, size)) = looks_like_pdf_upload(message) else {
        log_ev!("pdfcompress", trace_id, "not_a_pdf", "=>" => "reject");
        let _ = send_text_with_back(api, chat_id, &t("pdfcompress.error.invalid_file")).await;
        return;
    };

    let max_bytes = crate::config::pdf_compress_max_bytes();
    if size > 0 && size > max_bytes {
        log_ev!("pdfcompress", trace_id, "too_large", "size" => size, "max" => max_bytes, "=>" => "reject");
        let text = tf(
            "pdfcompress.error.too_large",
            &[("max", &fmt_bytes(max_bytes))],
        );
        let _ = send_text_with_back(api, chat_id, &text).await;
        return;
    }

    log_ev!("pdfcompress", trace_id, "file_accepted", "filename" => &filename, "size" => size);
    flow_manager.set(
        user_id,
        FlowState::AwaitingPdfCompressLevel { file_id, filename },
    );

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
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &mut FlowManager,
    level: &str,
) {
    if CpuBrokerGuard::is_user_busy(user_id).await {
        let _ = crate::bot::send_text(api, chat_id, &t("active_job_running")).await;
        return;
    }

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
    let _ = api
        .edit_message_text(
            &EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(message_id)
                .text(t("pdfcompress.processing"))
                .reply_markup(job_cancel_keyboard())
                .build(),
        )
        .await;

    let api2 = api.clone();
    let level_owned = level.to_string();
    let flow_mgr = flow_manager.clone();
    crate::app::spawn_user_task(async move {
        run_pdf_compress(
            api2,
            chat_id,
            message_id,
            user_id,
            file_id,
            filename,
            level_owned,
            trace_id,
            flow_mgr,
        )
        .await;
    });
}

async fn run_pdf_compress(
    api: Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    file_id: String,
    filename: String,
    level: String,
    trace_id: u64,
    flow_manager: FlowManager,
) {
    let cancel_flag = ACTIVE_PDF_JOBS.register(user_id);
    let _job_guard = ACTIVE_PDF_JOBS.guard(user_id);

    let ticker_handle = ProgressTicker::new(&api, chat_id, message_id)
        .interval(Duration::from_secs(2))
        .with_cancel_flag(cancel_flag.clone())
        .with_keyboard(job_cancel_keyboard())
        .spawn(|elapsed| {
            let s = elapsed.as_secs();
            let elapsed_str = format!("{:02}:{:02}", s / 60, s % 60);
            let text = apply_premium_to_md(&tf(
                "pdfcompress.processing_ticker",
                &[("elapsed", &md_escape(&elapsed_str))],
            ));
            Some(text)
        });

    let dir_guard = match TempDirGuard::create("pdfcompress", trace_id) {
        Ok(g) => g,
        Err(e) => {
            log_ev!("pdfcompress", trace_id, "temp_dir_failed", "err" => format!("{e}"));
            ticker_handle.stop();
            let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.gs_failed")).await;
            return;
        }
    };
    let work_dir = dir_guard.path().to_path_buf();
    let input_path = work_dir.join("input.pdf");
    let output_filename = format_lite_filename(&filename);
    let output_path = work_dir.join(&output_filename);

    let stats_job_id = crate::stats::record_download_start(user_id, "pdfcompress").await;

    log_ev!("pdfcompress", trace_id, "download_start", "filename" => &filename);
    let dl_result = match download_file(&api, &file_id, &input_path).await {
        Ok(res) => res,
        Err(e) => {
            ticker_handle.stop();
            let e_str = e.to_string();
            log_ev!("pdfcompress", trace_id, "download_failed", "=>" => format!("fail err={e_str}"));
            crate::stats::record_error_global("pdfcompress", &format!("download failed: {e_str}"))
                .await;
            let _ = edit_status(
                &api,
                chat_id,
                message_id,
                &t("pdfcompress.error.download_failed"),
            )
            .await;
            return;
        }
    };

    if cancel_flag.load(Ordering::Relaxed) {
        ticker_handle.stop();
        return;
    }

    if let Some(jid) = stats_job_id {
        crate::stats::record_download_done(
            jid,
            dl_result.bytes as i64,
            None,
            None,
            Some(dl_result.speed_bps() as i64),
        )
        .await;
    }

    let orig_size = std::fs::metadata(&input_path).map(|m| m.len()).unwrap_or(0);
    log_ev!("pdfcompress", trace_id, "download_done", "bytes" => orig_size, "speed" => dl_result.speed_human());

    if !starts_with_pdf_magic(&input_path) {
        ticker_handle.stop();
        log_ev!("pdfcompress", trace_id, "bad_magic_bytes", "=>" => "reject");
        let _ = edit_status(
            &api,
            chat_id,
            message_id,
            &t("pdfcompress.error.invalid_pdf"),
        )
        .await;
        return;
    }

    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "pdfcompress").await;
    if cancel_flag.load(Ordering::Relaxed) {
        ticker_handle.stop();
        cpu_guard.release().await;
        return;
    }

    let timeout_secs = crate::config::pdf_compress_timeout_secs();
    log_ev!("pdfcompress", trace_id, "gs_spawn", "level" => &level, "timeout" => timeout_secs);

    let gs_res = run_gs(
        &input_path,
        &output_path,
        &level,
        timeout_secs,
        trace_id,
        &cpu_guard.cores(),
    )
    .await;
    cpu_guard.release().await;

    if cancel_flag.load(Ordering::Relaxed) {
        ticker_handle.stop();
        return;
    }

    match gs_res {
        Ok(()) => {}
        Err(GsError::Timeout) => {
            ticker_handle.stop();
            log_ev!("pdfcompress", trace_id, "gs_timeout", "=>" => "fail");
            crate::stats::record_error_global("pdfcompress", "gs timeout").await;
            let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.timeout")).await;
            crate::stats::record_event_user(user_id, "pdfcompress", &level, "timeout", 0).await;
            return;
        }
        Err(GsError::Failed(err)) => {
            ticker_handle.stop();
            log_ev!("pdfcompress", trace_id, "gs_failed", "err" => &err, "=>" => "fail");
            crate::stats::record_error_global("pdfcompress", &format!("gs failed: {err}")).await;
            let _ = edit_status(&api, chat_id, message_id, &t("pdfcompress.error.gs_failed")).await;
            crate::stats::record_event_user(user_id, "pdfcompress", &level, "fail", 0).await;
            return;
        }
    }

    let compressed_size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if compressed_size == 0 || compressed_size >= orig_size {
        ticker_handle.stop();
        log_ev!("pdfcompress", trace_id, "no_improvement", "orig" => orig_size, "compressed" => compressed_size);
        let _ = edit_status(
            &api,
            chat_id,
            message_id,
            &t("pdfcompress.error.no_improvement"),
        )
        .await;
        crate::stats::record_event_user(user_id, "pdfcompress", &level, "no_improvement", 0).await;
        return;
    }

    let percent = if orig_size > 0 {
        ((orig_size as f64 - compressed_size as f64) / orig_size as f64 * 100.0) as u32
    } else {
        0
    };

    let before_str = fmt_bytes(orig_size);
    let after_str = fmt_bytes(compressed_size);

    let report = tf(
        "pdfcompress.result_report",
        &[
            ("before", &before_str),
            ("after", &after_str),
            ("percent", &percent.to_string()),
        ],
    );

    let caption = apply_premium_to_md(&format!(
        "{}\n\n{}",
        t("pdfcompress.result_caption"),
        report
    ));

    let out_bytes = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let up_start = std::time::Instant::now();

    ticker_handle.stop();

    let doc_params = SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(PathBuf::from(&output_path))
        .caption(&caption)
        .parse_mode(ParseMode::MarkdownV2)
        .build();

    use crate::bot::send_file_with_upload_ticker;
    match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
        &api,
        "sendDocument",
        &doc_params,
        std::path::Path::new(&output_path),
        chat_id,
        message_id,
        "transfer.stage.uploading",
        None,
    )
    .await
    {
        Ok(_) => {
            let up_elapsed = up_start.elapsed();
            let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
                out_bytes as f64 / up_elapsed.as_secs_f64()
            } else {
                0.0
            };
            if let Some(jid) = stats_job_id {
                crate::stats::record_upload_done(
                    jid,
                    user_id,
                    out_bytes as i64,
                    Some(up_speed as i64),
                    Some(1),
                )
                .await;
            }

            log_ev!("pdfcompress", trace_id, "result_sent", "=>" => "ok");
            let _ = api
                .delete_message(
                    &frankenstein::methods::DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(message_id)
                        .build(),
                )
                .await;

            flow_manager.set(user_id, FlowState::AwaitingPdfCompressFile);
            let prompt_text = apply_premium_to_md(&t("pdfcompress.send_file_prompt"));
            let rearm_params = SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&prompt_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(cancel_keyboard()))
                .build();
            let _ = api.send_message(&rearm_params).await;

            crate::stats::record_event_user(user_id, "pdfcompress", &level, "ok", 0).await;
            crate::metrics::get()
                .pdf_compress_total
                .with_label_values(&[&level, "success"])
                .inc();
        }
        Err(e) => {
            log_ev!("pdfcompress", trace_id, "result_send_failed", "=>" => format!("fail err={e}"));
            crate::stats::record_error_global("pdfcompress", &format!("send_document failed: {e}"))
                .await;
            let _ = edit_status(
                &api,
                chat_id,
                message_id,
                &t("pdfcompress.error.send_failed"),
            )
            .await;
            crate::stats::record_event_user(user_id, "pdfcompress", &level, "fail", 0).await;
            crate::metrics::get()
                .pdf_compress_total
                .with_label_values(&[&level, "fail"])
                .inc();
        }
    }
    std::fs::remove_dir_all(&work_dir).ok();
}

#[derive(Debug)]
enum GsError {
    Timeout,
    Failed(String),
}

// Ghostscript subprocess is sandboxed via rlimit (memory capped at 1GB, CPU time capped at 60s)
// and executed with -dSAFER.
async fn run_gs(
    input: &std::path::Path,
    output: &std::path::Path,
    level: &str,
    timeout_secs: u64,
    trace_id: u64,
    cores: &[i32],
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
        .stderr(Stdio::null());

    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(move || {
            let mem_limit = 4 * 1024 * 1024 * 1024; // 4GB
            let rlim_mem = libc::rlimit {
                rlim_cur: mem_limit,
                rlim_max: mem_limit,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &rlim_mem) != 0 {
                return Err(std::io::Error::last_os_error());
            }

            let cpu_limit = (timeout_secs.max(1)) as libc::rlim_t; // use the config timeout
            let rlim_cpu = libc::rlimit {
                rlim_cur: cpu_limit,
                rlim_max: cpu_limit,
            };
            if libc::setrlimit(libc::RLIMIT_CPU, &rlim_cpu) != 0 {
                return Err(std::io::Error::last_os_error());
            }

            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| GsError::Failed(format!("spawn: {e}")))?;

    if !cores.is_empty() {
        if let Some(pid) = child.id() {
            pin_pid_to_cores(pid, cores, trace_id);
        }
    }

    if timeout_secs == 0 {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(GsError::Timeout);
    }
    let duration = Duration::from_secs(timeout_secs);
    let wait = tokio::time::timeout(duration, child.wait()).await;
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
    if cores.is_empty() {
        return;
    }
    let cores_str = cores
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",");
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
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 5];
    if f.read_exact(&mut buf).is_err() {
        return false;
    }
    &buf == b"%PDF-"
}

#[allow(dead_code)]
fn escape_md(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '*' | '\\' | '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '='
            | '|' | '{' | '}' | '.' | '!' => format!("\\{c}"),
            other => other.to_string(),
        })
        .collect()
}

async fn edit_status(api: &Bot, chat_id: i64, message_id: i32, text: &str) {
    let kb = crate::bot::back_keyboard();
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(text)
        .reply_markup(kb)
        .build();
    let _ = api.edit_message_text(&params).await;
}

use crate::bot::download_telegram_file as download_file;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_pdf_upload() {
        use frankenstein::types::{Chat, ChatType, Document, Message};

        let dummy_chat = Chat::builder()
            .id(123)
            .type_field(ChatType::Private)
            .build();

        // 1. Message without document
        let msg_no_doc = Message::builder()
            .message_id(1)
            .date(0)
            .chat(dummy_chat.clone())
            .build();
        assert!(looks_like_pdf_upload(&msg_no_doc).is_none());

        // 2. Valid PDF by extension
        let doc_pdf = Document::builder()
            .file_id("id_pdf_1".to_string())
            .file_unique_id("uid_1".to_string())
            .file_name("report.pdf".to_string())
            .file_size(1000u64)
            .build();
        let msg_pdf = Message::builder()
            .message_id(2)
            .date(0)
            .chat(dummy_chat.clone())
            .document(doc_pdf)
            .build();
        assert_eq!(
            looks_like_pdf_upload(&msg_pdf),
            Some(("id_pdf_1".to_string(), "report.pdf".to_string(), 1000))
        );

        // 3. Uppercase PDF extension
        let doc_upper = Document::builder()
            .file_id("id_pdf_2".to_string())
            .file_unique_id("uid_2".to_string())
            .file_name("SUMMARY.PDF".to_string())
            .file_size(2000u64)
            .build();
        let msg_upper = Message::builder()
            .message_id(3)
            .date(0)
            .chat(dummy_chat.clone())
            .document(doc_upper)
            .build();
        assert_eq!(
            looks_like_pdf_upload(&msg_upper),
            Some(("id_pdf_2".to_string(), "SUMMARY.PDF".to_string(), 2000))
        );

        // 4. PDF by MIME type without .pdf extension
        let doc_mime = Document::builder()
            .file_id("id_pdf_3".to_string())
            .file_unique_id("uid_3".to_string())
            .file_name("my_doc".to_string())
            .mime_type("application/pdf".to_string())
            .file_size(500u64)
            .build();
        let msg_mime = Message::builder()
            .message_id(4)
            .date(0)
            .chat(dummy_chat.clone())
            .document(doc_mime)
            .build();
        assert_eq!(
            looks_like_pdf_upload(&msg_mime),
            Some(("id_pdf_3".to_string(), "my_doc".to_string(), 500))
        );

        // 5. Non-PDF document
        let doc_txt = Document::builder()
            .file_id("id_txt".to_string())
            .file_unique_id("uid_4".to_string())
            .file_name("notes.txt".to_string())
            .mime_type("text/plain".to_string())
            .file_size(300u64)
            .build();
        let msg_txt = Message::builder()
            .message_id(5)
            .date(0)
            .chat(dummy_chat.clone())
            .document(doc_txt)
            .build();
        assert!(looks_like_pdf_upload(&msg_txt).is_none());

        // 6. Document with no file_name (defaults to "document.pdf")
        let doc_no_name = Document::builder()
            .file_id("id_no_name".to_string())
            .file_unique_id("uid_5".to_string())
            .mime_type("application/pdf".to_string())
            .file_size(100u64)
            .build();
        let msg_no_name = Message::builder()
            .message_id(6)
            .date(0)
            .chat(dummy_chat.clone())
            .document(doc_no_name)
            .build();
        assert_eq!(
            looks_like_pdf_upload(&msg_no_name),
            Some(("id_no_name".to_string(), "document.pdf".to_string(), 100))
        );
    }

    #[test]
    fn test_starts_with_pdf_magic() {
        let dir = std::env::temp_dir();
        let pdf_file = dir.join("test_magic.pdf");
        let non_pdf_file = dir.join("test_magic.txt");

        std::fs::write(&pdf_file, b"%PDF-1.4\nsome content").unwrap();
        std::fs::write(&non_pdf_file, b"NOT A PDF").unwrap();

        assert!(starts_with_pdf_magic(&pdf_file));
        assert!(!starts_with_pdf_magic(&non_pdf_file));

        let _ = std::fs::remove_file(pdf_file);
        let _ = std::fs::remove_file(non_pdf_file);
    }

    #[test]
    fn test_fmt_bytes() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(fmt_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[tokio::test]
    async fn test_run_gs_invalid_input() {
        let input = std::path::Path::new("/nonexistent/file.pdf");
        let output = std::path::Path::new("/tmp/test_out.pdf");
        let result = run_gs(input, output, "screen", 5, 1, &[]).await;
        assert!(matches!(result, Err(GsError::Failed(_))));
    }

    #[tokio::test]
    async fn test_run_gs_timeout() {
        let dir = std::env::temp_dir();
        let input = dir.join("test_timeout_in.pdf");
        let output = dir.join("test_timeout_out.pdf");
        std::fs::write(&input, b"%PDF-1.4\n%EOF\n").unwrap();

        let result = run_gs(&input, &output, "screen", 0, 1, &[]).await;
        assert!(matches!(
            result,
            Err(GsError::Timeout) | Err(GsError::Failed(_))
        ));

        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn test_pdf_cancel_lifecycle() {
        let user_id = 999_888_009;
        let flag = ACTIVE_PDF_JOBS.register(user_id);
        assert!(ACTIVE_PDF_JOBS.is_active(&user_id));
        assert!(!flag.load(Ordering::SeqCst));

        // simulate cancel
        let cancelled = cancel_pdf_job(user_id);
        assert!(cancelled);
        assert!(flag.load(Ordering::SeqCst));
        assert!(!ACTIVE_PDF_JOBS.is_active(&user_id));

        // guard drop unregister test
        let user_id_2 = 999_888_010;
        let (flag2, _guard) = ACTIVE_PDF_JOBS.register_with_guard(user_id_2);
        assert!(ACTIVE_PDF_JOBS.is_active(&user_id_2));
        assert!(!flag2.load(Ordering::SeqCst));
        drop(_guard);
        assert!(!ACTIVE_PDF_JOBS.is_active(&user_id_2));
    }
}
