//! Telegram handler and pipeline worker for the package format converter feature (`pkg`).

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{EditMessageTextParams, SendMessageParams},
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};

use super::detect::{PkgFormat, detect_by_magic, detect_pkg_format};
use super::engine::{ConvertError, TargetFmt, run_convert_sync};
use super::validate::{MAX_INPUT_FILE_BYTES, validate_package};
use crate::bot::constants::{CB_PKG_CANCEL, CB_PKG_JOBCANCEL, CB_TOOLS_PKG};
use crate::bot::transfer::AsyncTelegramApiMetered;
use crate::bot::{download_telegram_file, send_text_md};
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::common::dir::TempDirGuard;
use crate::common::format::format_clock;
use crate::common::keyboard::job_cancel_keyboard as shared_job_cancel_keyboard;
use crate::common::ticker::ProgressTicker;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::panel::{btn_icon, btn_icon_success};
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::log::next_trace_id;
use crate::rank::{self, quota::QuotaKind, types::Rank};

use crate::common::job::JobRegistry;

static ACTIVE_PKG_JOBS: LazyLock<JobRegistry<i64>> = LazyLock::new(JobRegistry::new);

pub fn cancel_pkg_job(user_id: i64) -> bool {
    ACTIVE_PKG_JOBS.cancel(&user_id)
}

pub const STAGE_DOWNLOADING: u8 = 0;
pub const STAGE_VALIDATING: u8 = 1;
pub const STAGE_CONVERTING: u8 = 2;
pub const STAGE_UPLOADING: u8 = 3;

#[derive(Default)]
pub struct PkgJobStage(AtomicU8);

impl PkgJobStage {
    pub fn set(&self, stage: u8) {
        self.0.store(stage, Ordering::Relaxed);
    }
    pub fn get(&self) -> u8 {
        self.0.load(Ordering::Relaxed)
    }
}

// ── Keyboards ──────────────────────────────────────────────────────────────────

fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon(
            &t("start.back"),
            CB_PKG_CANCEL,
            "back",
        )]])
        .build()
}

fn job_cancel_keyboard() -> InlineKeyboardMarkup {
    shared_job_cancel_keyboard(&t("pkg.cancel_btn"), CB_PKG_JOBCANCEL, "cancel")
}

fn convert_choice_keyboard(src: PkgFormat) -> InlineKeyboardMarkup {
    let mut row = Vec::new();
    match src {
        PkgFormat::Deb => {
            row.push(btn_icon_success(
                &t("pkg.convert_btn_rpm"),
                "pkg:convert:deb:rpm",
                "redhat_linux_logo",
            ));
            row.push(btn_icon_success(
                &t("pkg.convert_btn_pacman"),
                "pkg:convert:deb:pacman",
                "arch_linux_logo",
            ));
        }
        PkgFormat::Rpm => {
            row.push(btn_icon_success(
                &t("pkg.convert_btn_deb"),
                "pkg:convert:rpm:deb",
                "debian_linux_logo",
            ));
            row.push(btn_icon_success(
                &t("pkg.convert_btn_pacman"),
                "pkg:convert:rpm:pacman",
                "arch_linux_logo",
            ));
        }
        PkgFormat::Pacman => {
            row.push(btn_icon_success(
                &t("pkg.convert_btn_deb"),
                "pkg:convert:pacman:deb",
                "debian_linux_logo",
            ));
            row.push(btn_icon_success(
                &t("pkg.convert_btn_rpm"),
                "pkg:convert:pacman:rpm",
                "redhat_linux_logo",
            ));
        }
    }

    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![
            row,
            vec![btn_icon(&t("start.back"), CB_PKG_CANCEL, "back")],
        ])
        .build()
}

// ── Entry Point ────────────────────────────────────────────────────────────────

pub async fn enter_pkgconvert(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    log_actor_id!("pkgconvert", trace_id, user_id, "clicked" => CB_TOOLS_PKG);
    flow_manager.set(user_id, FlowState::AwaitingPkgFile);

    let text = apply_premium_to_md(&t("pkg.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(cancel_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}

pub async fn send_pkg_prompt(api: &Bot, chat_id: i64, user_id: i64, flow_manager: &FlowManager) {
    let trace_id = next_trace_id();
    log_actor_id!("pkgconvert", trace_id, user_id, "rearm" => "pkg_prompt");
    flow_manager.set(user_id, FlowState::AwaitingPkgFile);

    let text = apply_premium_to_md(&t("pkg.prompt"));
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(cancel_keyboard()))
        .build();

    let _ = api.send_message(&params).await;
}

// ── File Intake Handler ────────────────────────────────────────────────────────

pub async fn handle_pkg_file(
    api: &Bot,
    message: &Message,
    user_id: i64,
    flow_manager: &FlowManager,
    _database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    let doc = match &message.document {
        Some(d) => d,
        None => return,
    };

    let filename = doc
        .file_name
        .clone()
        .unwrap_or_else(|| "package.deb".to_string());
    let file_size = doc.file_size.unwrap_or(0) as u64;

    log_ev!("pkgconvert", trace_id, "file_received", "filename" => &filename, "size" => file_size);

    if file_size > MAX_INPUT_FILE_BYTES {
        log_ev!("pkgconvert", trace_id, "file_too_large", "size" => file_size, "max" => MAX_INPUT_FILE_BYTES);
        let _ = send_text_md(
            api,
            chat_id,
            &tf("pkg.error.file_too_large", &[("max", "200 MB")]),
        )
        .await;
        send_pkg_prompt(api, chat_id, user_id, flow_manager).await;
        return;
    }

    let fn_lower = filename.to_lowercase();
    let src_fmt = if fn_lower.ends_with(".deb") {
        Some(PkgFormat::Deb)
    } else if fn_lower.ends_with(".rpm") {
        Some(PkgFormat::Rpm)
    } else if fn_lower.ends_with(".pkg.tar.zst") || fn_lower.ends_with(".tar.zst") {
        Some(PkgFormat::Pacman)
    } else {
        None
    };

    let src_fmt = match src_fmt {
        Some(f) => f,
        None => {
            log_ev!("pkgconvert", trace_id, "unsupported_ext", "filename" => &filename);
            let _ = send_text_md(api, chat_id, &t("pkg.error.unsupported_format")).await;
            send_pkg_prompt(api, chat_id, user_id, flow_manager).await;
            return;
        }
    };

    flow_manager.set(
        user_id,
        FlowState::AwaitingPkgConvertChoice {
            file_id: doc.file_id.clone(),
            filename: filename.clone(),
            file_size,
            src_fmt,
        },
    );

    let escaped_fmt = crate::i18n::md_escape(src_fmt.display_ext());
    let text = apply_premium_to_md(&tf("pkg.detected", &[("fmt", &escaped_fmt)]));
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(convert_choice_keyboard(
            src_fmt,
        )))
        .build();

    if let Err(e) = api.send_message(&params).await {
        log_ev!("pkgconvert", trace_id, "send_detected_failed", "err" => format!("{e:?}"));
    }
}

// ── Callback Handler ───────────────────────────────────────────────────────────

pub async fn handle_pkg_jobcancel(user_id: i64, api: &Bot, chat_id: i64, message_id: i32) {
    let trace_id = next_trace_id();
    log_ev!("pkgconvert", trace_id, "job_cancel_clicked", "user_id" => user_id);
    cancel_pkg_job(user_id);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&apply_premium_to_md(&t("pkg.cancelled")))
        .parse_mode(ParseMode::MarkdownV2)
        .build();
    let _ = api.edit_message_text(&params).await;
}

pub async fn handle_pkg_callback(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    action: &str,
    _cb_id: &str,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_ev!("pkgconvert", trace_id, "callback", "action" => action, "user_id" => user_id);

    if action == "cancel" {
        flow_manager.clear(user_id);
        let _ = crate::bot::edit_to_dev_cafe(api, chat_id, message_id).await;
        return;
    }

    let parts: Vec<&str> = action.split(':').collect();
    if parts.len() == 2 {
        let src_fmt = match PkgFormat::from_str(parts[0]) {
            Some(f) => f,
            None => return,
        };
        let dst_fmt = match TargetFmt::from_str(parts[1]) {
            Some(f) => f,
            None => return,
        };

        let (file_id, filename, _file_size) = match flow_manager.get(user_id) {
            FlowState::AwaitingPkgConvertChoice {
                file_id,
                filename,
                file_size,
                src_fmt: s_fmt,
            } if s_fmt == src_fmt => (file_id, filename, file_size),
            _ => {
                log_ev!("pkgconvert", trace_id, "stale_choice_callback", "user_id" => user_id);
                return;
            }
        };

        flow_manager.clear(user_id);

        // Paywall & Quota checks
        let Some(db) = database.as_ref() else {
            let _ = send_text_md(api, chat_id, &t("pkg.error.system_error")).await;
            return;
        };
        let (rank, daily_limit, reserve_res) = {
            let client = match db.get().await {
                Ok(c) => c,
                Err(e) => {
                    log_ev!("pkgconvert", trace_id, "quota_checkout", "err" => format!("{e}"), "=>" => "fail");
                    let _ = send_text_md(api, chat_id, &t("pkg.error.system_error")).await;
                    return;
                }
            };
            let rank = rank::effective_rank(&client, user_id).await;
            let daily_limit = rank.pkgconvert_daily_count();
            let res = if daily_limit > 0 {
                rank::quota::reserve_usage(
                    &client,
                    user_id,
                    QuotaKind::PkgConvertDaily,
                    1,
                    86400,
                    daily_limit as i64,
                )
                .await
            } else {
                Ok(None)
            };
            (rank, daily_limit, res)
        };

        if daily_limit == 0 {
            log_ev!("pkgconvert", trace_id, "paywall_blocked", "rank" => rank.as_str());
            rank::paywall::block_feature(api, chat_id, &t("pkg.paywall"), Rank::Sepahbod).await;
            return;
        }

        if CpuBrokerGuard::is_user_busy(user_id).await {
            let _ = send_text_md(api, chat_id, &t("active_job_running")).await;
            return;
        }

        match reserve_res {
            Ok(Some(used)) => {
                log_ev!("pkgconvert", trace_id, "quota_reserved", "used" => used, "limit" => daily_limit);
            }
            Ok(None) => {
                log_ev!("pkgconvert", trace_id, "quota_blocked", "limit" => daily_limit);
                if let Some(nr) = rank.pkgconvert_next_rank() {
                    rank::paywall::block_limit(api, chat_id, &t("pkg.quota_exceeded"), nr).await;
                } else {
                    let _ = send_text_md(api, chat_id, &t("pkg.quota_exceeded")).await;
                }
                return;
            }
            Err(e) => {
                log_ev!("pkgconvert", trace_id, "quota_error", "err" => format!("{e}"));
                rank::paywall::quota_db_error(api, chat_id, "pkgconvert", &format!("{e}")).await;
                return;
            }
        }

        let stage = Arc::new(PkgJobStage::default());
        stage.set(STAGE_DOWNLOADING);

        let initial_text = apply_premium_to_md(&t("pkg.stage.downloading"));
        let edit_params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(&initial_text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(job_cancel_keyboard())
            .build();

        let progress_msg_id = match api.edit_message_text(&edit_params).await {
            Ok(m) => match m.result {
                frankenstein::response::MessageOrBool::Message(msg) => msg.message_id,
                _ => message_id,
            },
            Err(_) => message_id,
        };

        let cancel_flag = ACTIVE_PKG_JOBS.register(user_id);

        let timer_stage = stage.clone();
        let ticker_handle = ProgressTicker::new(api, chat_id, progress_msg_id)
            .interval(Duration::from_secs(3))
            .with_cancel_flag(cancel_flag.clone())
            .with_keyboard(job_cancel_keyboard())
            .spawn(move |elapsed| {
                let elapsed_str = format_clock(elapsed.as_secs());
                let text_raw = match timer_stage.get() {
                    STAGE_DOWNLOADING => t("pkg.stage.downloading"),
                    STAGE_VALIDATING => t("pkg.stage.validating"),
                    STAGE_CONVERTING => tf("pkg.stage.converting", &[("elapsed", &elapsed_str)]),
                    STAGE_UPLOADING => t("pkg.stage.uploading"),
                    _ => t("pkg.stage.converting"),
                };
                Some(apply_premium_to_md(&text_raw))
            });

        let api_worker = api.clone();
        let db_worker = database.clone();
        let fm_worker = flow_manager.clone();

        crate::app::spawn_user_task(async move {
            run_pkg_worker(
                api_worker,
                chat_id,
                progress_msg_id,
                user_id,
                file_id,
                filename,
                src_fmt,
                dst_fmt,
                trace_id,
                db_worker,
                fm_worker,
                cancel_flag,
                ticker_handle,
                stage,
            )
            .await;
        });
    }
}

// ── Pipeline Worker Task ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_pkg_worker(
    api: Bot,
    chat_id: i64,
    _progress_msg_id: i32,
    user_id: i64,
    file_id: String,
    filename: String,
    src_fmt: PkgFormat,
    dst_fmt: TargetFmt,
    trace_id: u64,
    database: Option<PostgresDatabase>,
    flow_manager: FlowManager,
    cancel: Arc<AtomicBool>,
    ticker: crate::common::ProgressTickerHandle,
    stage: Arc<PkgJobStage>,
) {
    let _job_guard = ACTIVE_PKG_JOBS.guard(user_id);

    macro_rules! stop_timer {
        () => {{
            ticker.stop();
        }};
    }

    macro_rules! refund {
        ($why:expr) => {
            if let Some(db) = database.as_ref() {
                log_ev!("pkgconvert", trace_id, "quota_refund", "why" => $why);
                if let Ok(client) = db.get().await {
                    if let Err(e) = rank::quota::refund_usage(
                        &client,
                        user_id,
                        QuotaKind::PkgConvertDaily,
                        1,
                        86400,
                    )
                    .await
                    {
                        log_ev!("pkgconvert", trace_id, "quota_refund", "err" => format!("{e}"), "=>" => "fail");
                        crate::stats::record_error_global("pkgconvert", "quota_refund_failed").await;
                    }
                }
            }
        };
    }

    macro_rules! re_arm {
        () => {{
            send_pkg_prompt(&api, chat_id, user_id, &flow_manager).await;
        }};
    }

    let dir_guard = match TempDirGuard::create("pkgconvert", trace_id) {
        Ok(g) => g,
        Err(e) => {
            log_ev!("pkgconvert", trace_id, "mkdir_failed", "err" => format!("{e}"));
            stop_timer!();
            refund!("mkdir_failed");
            let _ = send_text_md(&api, chat_id, &t("pkg.error.system_error")).await;
            re_arm!();
            return;
        }
    };
    let work_dir = dir_guard.path().to_path_buf();

    // 1. Download Stage
    stage.set(STAGE_DOWNLOADING);
    if cancel.load(Ordering::Relaxed) {
        log_ev!("pkgconvert", trace_id, "cancelled_before_download");
        stop_timer!();
        refund!("cancelled");
        return;
    }

    let input_path = work_dir.join(&filename);
    let dl_res = match download_telegram_file(&api, &file_id, &input_path).await {
        Ok(res) => res,
        Err(e) => {
            log_ev!("pkgconvert", trace_id, "download_failed", "err" => format!("{e}"));
            stop_timer!();
            refund!("download_failed");
            let _ = send_text_md(&api, chat_id, &t("pkg.error.system_error")).await;
            re_arm!();
            return;
        }
    };
    log_ev!("pkgconvert", trace_id, "download_done", "bytes" => dl_res.bytes);

    // 2. Validation Stage
    stage.set(STAGE_VALIDATING);
    if cancel.load(Ordering::Relaxed) {
        log_ev!("pkgconvert", trace_id, "cancelled_before_validation");
        stop_timer!();
        refund!("cancelled");
        return;
    }

    // Double-check magic bytes against extension
    if detect_pkg_format(&input_path, &filename).is_none()
        && detect_by_magic(&input_path) != Some(src_fmt)
    {
        log_ev!("pkgconvert", trace_id, "magic_mismatch", "filename" => &filename);
        stop_timer!();
        refund!("magic_mismatch");
        let _ = send_text_md(&api, chat_id, &t("pkg.error.format_mismatch")).await;
        re_arm!();
        return;
    }

    if let Err(e) = validate_package(&input_path, src_fmt, trace_id).await {
        log_ev!("pkgconvert", trace_id, "validation_failed", "err" => format!("{e}"));
        stop_timer!();
        refund!("validation_failed");

        let err_key = match e {
            super::validate::ValidateError::TooLarge(_) => "pkg.error.archive_too_large",
            super::validate::ValidateError::TooManyEntries(_) => "pkg.error.too_many_entries",
            super::validate::ValidateError::PathTraversal(_) => {
                crate::stats::record_error_global("pkgconvert", "path_traversal").await;
                "pkg.error.malicious_archive"
            }
            super::validate::ValidateError::SymlinkEscape(_, _) => {
                crate::stats::record_error_global("pkgconvert", "symlink_escape").await;
                "pkg.error.malicious_archive"
            }
            super::validate::ValidateError::FileTooLarge(_) => "pkg.error.archive_too_large",
            super::validate::ValidateError::Timeout => {
                crate::stats::record_error_global("pkgconvert", "validate_timeout").await;
                "pkg.error.validate_timeout"
            }
            super::validate::ValidateError::ParseError(_) => "pkg.error.parse_failed",
        };

        let _ = send_text_md(&api, chat_id, &t(err_key)).await;
        re_arm!();
        return;
    }

    // 3. CPU Broker & Worker Conversion Stage
    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "pkg").await;
    if cancel.load(Ordering::Relaxed) {
        log_ev!("pkgconvert", trace_id, "cancelled_after_cpu_acquire");
        cpu_guard.release().await;
        stop_timer!();
        refund!("cancelled");
        return;
    }

    stage.set(STAGE_CONVERTING);
    let cancel_worker = cancel.clone();
    let work_dir_worker = work_dir.clone();
    let input_path_worker = input_path.clone();
    let guard_cores = cpu_guard.cores().to_vec();

    let convert_res = tokio::task::spawn_blocking(move || {
        if !guard_cores.is_empty() {
            crate::moebius::cpu::pin_current_thread(&guard_cores, trace_id);
        }
        run_convert_sync(
            &work_dir_worker,
            &input_path_worker,
            src_fmt,
            dst_fmt,
            Duration::from_secs(120),
            trace_id,
            &cancel_worker,
        )
    })
    .await;

    cpu_guard.release().await;
    stop_timer!();

    if cancel.load(Ordering::Relaxed) {
        log_ev!("pkgconvert", trace_id, "cancelled_post_convert");
        refund!("cancelled");
        std::fs::remove_dir_all(&work_dir).ok();
        return;
    }

    let result = match convert_res {
        Ok(Ok(res)) => res,
        Ok(Err(ConvertError::Cancelled)) => {
            refund!("cancelled");
            std::fs::remove_dir_all(&work_dir).ok();
            return;
        }
        Ok(Err(ConvertError::Timeout)) => {
            log_ev!("pkgconvert", trace_id, "convert_timeout");
            crate::stats::record_error_global("pkgconvert", "convert_timeout").await;
            refund!("convert_timeout");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_md(&api, chat_id, &t("pkg.error.convert_timeout")).await;
            re_arm!();
            return;
        }
        Ok(Err(ConvertError::SpawnFailed(err))) => {
            log_ev!("pkgconvert", trace_id, "spawn_failed", "err" => &err);
            crate::stats::record_error_global("pkgconvert", "spawn_failed").await;
            refund!("spawn_failed");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_md(&api, chat_id, &t("pkg.error.system_error")).await;
            re_arm!();
            return;
        }
        Ok(Err(ConvertError::NoOutput)) => {
            log_ev!("pkgconvert", trace_id, "no_output");
            crate::stats::record_error_global("pkgconvert", "no_output").await;
            refund!("no_output");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_md(&api, chat_id, &t("pkg.error.no_output")).await;
            re_arm!();
            return;
        }
        Ok(Err(ConvertError::ProcessFailed { exit_code, stderr })) => {
            log_ev!("pkgconvert", trace_id, "process_failed", "code" => exit_code, "err" => &stderr);
            crate::stats::record_error_global("pkgconvert", "convert_failed").await;
            refund!("convert_failed");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_md(&api, chat_id, &t("pkg.error.convert_failed")).await;
            re_arm!();
            return;
        }
        Err(panic_err) => {
            log_ev!("pkgconvert", trace_id, "thread_panic", "err" => format!("{panic_err}"));
            refund!("thread_panic");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_md(&api, chat_id, &t("pkg.error.system_error")).await;
            re_arm!();
            return;
        }
    };

    // 4. Upload Stage
    stage.set(STAGE_UPLOADING);
    let output_bytes = std::fs::metadata(&result.output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let size_str = crate::stats::fmt_bytes(output_bytes as i64);

    let out_filename = result
        .output_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("package{}", dst_fmt.ext()));

    let install_cmd = match dst_fmt {
        TargetFmt::Deb => format!("sudo dpkg -i {out_filename}"),
        TargetFmt::Rpm => format!("sudo rpm -i {out_filename}"),
        TargetFmt::Pacman => format!("sudo pacman -U {out_filename}"),
    };

    let esc_src = crate::i18n::md_escape(src_fmt.display_ext());
    let esc_dst = crate::i18n::md_escape(dst_fmt.ext());
    let esc_size = crate::i18n::md_escape(&size_str);
    let esc_cmd = crate::i18n::md_escape(&install_cmd);

    let caption_raw = match &result.deps_warning {
        Some(warn) => tf(
            "pkg.success_deps_warn",
            &[
                ("src", &esc_src),
                ("dst", &esc_dst),
                ("warn", &crate::i18n::md_escape(warn)),
                ("cmd", &esc_cmd),
            ],
        ),
        None => tf(
            "pkg.success",
            &[
                ("src", &esc_src),
                ("dst", &esc_dst),
                ("size", &esc_size),
                ("cmd", &esc_cmd),
            ],
        ),
    };

    let doc_params = frankenstein::methods::SendDocumentParams::builder()
        .chat_id(chat_id)
        .document(result.output_path.clone())
        .caption(&apply_premium_to_md(&caption_raw))
        .parse_mode(ParseMode::MarkdownV2)
        .build();

    let upload_res = api.send_document_metered(&doc_params).await;
    std::fs::remove_dir_all(&work_dir).ok();

    if upload_res.is_ok() {
        log_ev!("pkgconvert", trace_id, "upload_done", "bytes" => output_bytes);
        crate::stats::record_event_user(
            user_id,
            "pkgconvert",
            dst_fmt.as_str(),
            "ok",
            output_bytes as i64,
        )
        .await;
    } else {
        log_ev!("pkgconvert", trace_id, "upload_failed");
    }

    re_arm!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkgconvert_cancel_lifecycle() {
        let user_id = 999_888_001;
        let flag = ACTIVE_PKG_JOBS.register(user_id);
        assert!(ACTIVE_PKG_JOBS.is_active(&user_id));
        assert!(!flag.load(Ordering::SeqCst));

        // cancel job
        let cancelled = cancel_pkg_job(user_id);
        assert!(cancelled);
        assert!(flag.load(Ordering::SeqCst));
        assert!(!ACTIVE_PKG_JOBS.is_active(&user_id));

        // guard drop unregister test
        let user_id_2 = 999_888_002;
        let (flag2, _guard) = ACTIVE_PKG_JOBS.register_with_guard(user_id_2);
        assert!(ACTIVE_PKG_JOBS.is_active(&user_id_2));
        assert!(!flag2.load(Ordering::SeqCst));
        drop(_guard);
        assert!(!ACTIVE_PKG_JOBS.is_active(&user_id_2));
    }
}
