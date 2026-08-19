//! Worker pipeline and CPU broker execution for file compression.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    methods::{DeleteMessageParams, SendDocumentParams, SendMessageParams},
    types::ReplyMarkup,
};

use super::config::CompressConfig;
use super::engine::{CompressError, run_compress};
use super::handle::ACTIVE_FC_JOBS;
use super::progress::{done_inline_keyboard, job_cancel_keyboard, render_progress, JobProgress};
use crate::bot::{download_telegram_file, send_text_with_back};
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::common::dir::TempDirGuard;
use crate::common::format::fmt_bytes;
use crate::common::ticker::ProgressTicker;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::flow::CompressFileEntry;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, t, tf};
use crate::rank::{self, quota::QuotaKind};

#[allow(clippy::too_many_arguments)]
pub async fn start_compression_task(
    api: &Bot,
    chat_id: i64,
    prompt_msg_id: i32,
    user_id: i64,
    config: CompressConfig,
    files: Vec<CompressFileEntry>,
    trace_id: u64,
    database: &Option<PostgresDatabase>,
    flow_manager: &FlowManager,
) {
    if CpuBrokerGuard::is_user_busy(user_id).await {
        let _ = crate::bot::send_text_md(api, chat_id, &t("active_job_running")).await;
        return;
    }

    // Remove reply keyboard first
    let remove_kb = frankenstein::types::ReplyKeyboardRemove::builder()
        .remove_keyboard(true)
        .build();
    let _ = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text("⏳")
                .reply_markup(ReplyMarkup::ReplyKeyboardRemove(remove_kb))
                .build(),
        )
        .await;

    // Quota Paywall Check
    let Some(db) = database.as_ref() else {
        crate::log_ev!("filecompress", trace_id, "db_connect_failed", "err" => "no_db");
        let _ = send_text_with_back(api, chat_id, &t("fc.error.compress_failed")).await;
        return;
    };

    let mut reserved = false;
    {
        let client = match db.get().await {
            Ok(c) => c,
            Err(e) => {
                crate::log_ev!("filecompress", trace_id, "quota_checkout_failed", "err" => format!("{e}"));
                crate::rank::paywall::quota_db_error(api, chat_id, "filecompress", &format!("{e}")).await;
                return;
            }
        };

        let rank = rank::effective_rank(&client, user_id).await;
        let daily_limit = rank.compress_cpu_daily_secs();
        let monthly_limit = rank.compress_cpu_monthly_secs();

        let next_rank = match rank {
            crate::rank::types::Rank::Dalavar => Some(crate::rank::types::Rank::Sepahbod),
            crate::rank::types::Rank::Sepahbod | crate::rank::types::Rank::Esfandyar => {
                Some(crate::rank::types::Rank::Sohrab)
            }
            crate::rank::types::Rank::Sohrab => Some(crate::rank::types::Rank::Rostam),
            crate::rank::types::Rank::Rostam => None,
        };

        // Quota unit is CPU seconds, unknown until job finishes. Reserve 1 second
        // so check + debit remains atomic, then settle remainder with add_usage after completion.
        for (kind, window, limit, label_key, event) in [
            (
                QuotaKind::CompressCpuDaily,
                86400i64,
                daily_limit,
                "fc.error.quota_daily",
                "paywall_daily_blocked",
            ),
            (
                QuotaKind::CompressCpuMonthly,
                2592000i64,
                monthly_limit,
                "fc.error.quota_monthly",
                "paywall_monthly_blocked",
            ),
        ] {
            match rank::quota::reserve_usage(&client, user_id, kind, 1, window, limit as i64).await {
                Ok(Some(used)) => {
                    crate::log_ev!("filecompress", trace_id, "quota_reserved", "kind" => kind.as_str(), "used" => used, "limit" => limit);
                }
                Ok(None) => {
                    crate::log_ev!("filecompress", trace_id, event, "limit" => limit, "=>" => "blocked");
                    if reserved {
                        if let Err(e) = rank::quota::refund_usage(
                            &client,
                            user_id,
                            QuotaKind::CompressCpuDaily,
                            1,
                            86400,
                        )
                        .await
                        {
                            crate::log_ev!("filecompress", trace_id, "quota_refund", "err" => format!("{e}"), "=>" => "fail");
                            crate::stats::record_error_global("filecompress", "quota_refund_failed").await;
                        }
                    }
                    let label = t(label_key);
                    if let Some(nr) = next_rank {
                        crate::rank::paywall::block_limit(api, chat_id, &label, nr).await;
                    } else {
                        let _ = send_text_with_back(api, chat_id, &label).await;
                    }
                    return;
                }
                Err(e) => {
                    // fail closed — notify user on DB error
                    crate::log_ev!("filecompress", trace_id, "quota_reserve", "err" => format!("{e}"), "=>" => "fail");
                    if reserved {
                        if let Err(re) = rank::quota::refund_usage(
                            &client,
                            user_id,
                            QuotaKind::CompressCpuDaily,
                            1,
                            86400,
                        )
                        .await
                        {
                            crate::log_ev!("filecompress", trace_id, "quota_refund", "err" => format!("{re}"), "=>" => "fail");
                            crate::stats::record_error_global("filecompress", "quota_refund_failed").await;
                        }
                    }
                    crate::rank::paywall::quota_db_error(api, chat_id, "filecompress", &format!("{e}"))
                        .await;
                    return;
                }
            }
            reserved = true;
        }
    }

    let progress = Arc::new(JobProgress::new(files.len()));
    progress.set_downloading(1);

    let progress_msg = match api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&apply_premium_to_md(&render_progress(&progress, 0)))
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(job_cancel_keyboard()))
                .build(),
        )
        .await
    {
        Ok(m) => m.result.message_id,
        Err(_) => prompt_msg_id,
    };

    // Cancel flag + staged progress ticker on the status message
    let cancel_flag = ACTIVE_FC_JOBS.register(user_id);
    let timer_progress = progress.clone();
    let ticker_handle = ProgressTicker::new(api, chat_id, progress_msg)
        .interval(Duration::from_secs(3))
        .with_cancel_flag(cancel_flag.clone())
        .with_keyboard(job_cancel_keyboard())
        .spawn(move |elapsed| {
            let text = apply_premium_to_md(&render_progress(
                &timer_progress,
                elapsed.as_secs(),
            ));
            Some(text)
        });

    let api_clone = api.clone();
    let db_clone = database.clone();
    let fm = flow_manager.clone();
    crate::app::spawn_user_task(async move {
        run_filecompress_worker(
            api_clone,
            chat_id,
            progress_msg,
            user_id,
            config,
            files,
            trace_id,
            db_clone,
            fm,
            cancel_flag,
            ticker_handle,
            progress,
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_filecompress_worker(
    api: Bot,
    chat_id: i64,
    progress_msg_id: i32,
    user_id: i64,
    config: CompressConfig,
    files: Vec<CompressFileEntry>,
    trace_id: u64,
    database: Option<PostgresDatabase>,
    flow_manager: FlowManager,
    cancel_flag: Arc<AtomicBool>,
    ticker: crate::common::ProgressTickerHandle,
    progress: Arc<JobProgress>,
) {
    let _job_guard = ACTIVE_FC_JOBS.guard(user_id);
    let job_started = std::time::Instant::now();
    // Stop ticker on exit
    macro_rules! stop_timer {
        () => {{
            ticker.stop();
        }};
    }

    // Called only when database is present and both windows are reserved
    macro_rules! refund {
        ($why:expr) => {
            if let Some(db) = database.as_ref() {
                crate::log_ev!("filecompress", trace_id, "quota_refund", "why" => $why);
                if let Ok(client) = db.get().await {
                    for (kind, window) in [
                        (QuotaKind::CompressCpuDaily, 86400i64),
                        (QuotaKind::CompressCpuMonthly, 2592000i64),
                    ] {
                        if let Err(e) =
                            rank::quota::refund_usage(&client, user_id, kind, 1, window).await
                        {
                            crate::log_ev!("filecompress", trace_id, "quota_refund", "err" => format!("{e}"), "=>" => "fail");
                            crate::stats::record_error_global("filecompress", "quota_refund_failed")
                                .await;
                        }
                    }
                }
            }
        };
    }

    // Re-arm: send the file-compress prompt so user isn't stranded.
    macro_rules! re_arm_flow {
        () => {{
            let upload_text = apply_premium_to_md(&t("fc.upload_prompt").replace("{count}", "0"));
            let send_res = api
                .send_message(
                    &SendMessageParams::builder()
                        .chat_id(chat_id)
                        .text(&upload_text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(done_inline_keyboard()))
                        .build(),
                )
                .await;
            if let Ok(m) = send_res {
                flow_manager.set(
                    user_id,
                    FlowState::AwaitingCompressFiles {
                        config: Box::new(config.clone()),
                        files: Vec::new(),
                        prompt_msg_id: m.result.message_id,
                    },
                );
            }
        }};
    }

    let dir_guard = match TempDirGuard::create("filecompress", trace_id) {
        Ok(g) => g,
        Err(e) => {
            crate::log_ev!("filecompress", trace_id, "mkdir_failed", "err" => format!("{e}"));
            stop_timer!();
            refund!("mkdir_failed");
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.compress_failed")).await;
            re_arm_flow!();
            return;
        }
    };
    let work_dir = dir_guard.path().to_path_buf();

    let mut local_input_paths = Vec::new();
    let stats_job_id = crate::stats::record_download_start(user_id, "filecompress").await;
    let mut total_dl_bytes = 0u64;

    for (idx, entry) in files.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            crate::log_ev!("filecompress", trace_id, "cancelled_during_download", "idx" => idx);
            stop_timer!();
            refund!("cancelled");
            return;
        }
        let local_path = work_dir.join(&entry.filename);
        progress.set_downloading(idx + 1);
        let dl_res = match download_telegram_file(&api, &entry.file_id, &local_path).await {
            Ok(res) => res,
            Err(e) => {
                crate::log_ev!("filecompress", trace_id, "download_failed", "err" => format!("{e}"));
                stop_timer!();
                refund!("download_failed");
                let _ = send_text_with_back(&api, chat_id, &t("fc.error.download_failed")).await;
                re_arm_flow!();
                return;
            }
        };
        total_dl_bytes += dl_res.bytes;
        let size = std::fs::metadata(&local_path).map(|m| m.len()).unwrap_or(0);
        crate::log_ev!("filecompress", trace_id, "download_done", "idx" => idx, "bytes" => size, "speed" => dl_res.speed_human(), "=>" => "ok");
        local_input_paths.push(local_path);
    }

    if let Some(jid) = stats_job_id {
        crate::stats::record_download_done(jid, total_dl_bytes as i64, None, None, None).await;
    }

    if cancel_flag.load(Ordering::Relaxed) {
        crate::log_ev!("filecompress", trace_id, "cancelled_before_engine", "user_id" => user_id);
        stop_timer!();
        refund!("cancelled");
        return;
    }

    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "filecompress").await;
    let timeout = Duration::from_secs(1800); // 30 minutes max

    // Downloads and the broker queue are behind us; the ETA clock starts here.
    progress.set_compressing(job_started.elapsed().as_secs());
    crate::log_ev!("filecompress", trace_id, "engine_start", "files" => local_input_paths.len());
    let compress_res = run_compress(
        &work_dir,
        &config,
        &local_input_paths,
        timeout,
        &cpu_guard.cores(),
        trace_id,
        &cancel_flag,
        &progress,
    )
    .await;

    cpu_guard.release().await;
    stop_timer!();

    // User clicked cancel mid-job: discard output and refund quota.
    if cancel_flag.load(Ordering::Relaxed) {
        crate::log_ev!("filecompress", trace_id, "cancelled_mid_job", "user_id" => user_id);
        refund!("cancelled");
        return;
    }

    let result = match compress_res {
        Ok(r) => r,
        Err(CompressError::Timeout) => {
            crate::log_ev!("filecompress", trace_id, "timeout");
            refund!("timeout");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.timeout")).await;
            re_arm_flow!();
            return;
        }
        Err(e) => {
            crate::log_ev!("filecompress", trace_id, "compress_failed", "err" => format!("{e}"));
            refund!("compress_failed");
            std::fs::remove_dir_all(&work_dir).ok();
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.compress_failed")).await;
            re_arm_flow!();
            return;
        }
    };

    // Settlement: 1 second deducted during reservation, settle remainder with add_usage.
    let cpu_secs_used = result.cpu_secs.ceil() as i64;
    let cpu_secs_delta = (cpu_secs_used - 1).max(0);
    if let Some(db) = database.as_ref() {
        if cpu_secs_delta > 0 {
            if let Ok(client) = db.get().await {
                for (kind, window) in [
                    (QuotaKind::CompressCpuDaily, 86400i64),
                    (QuotaKind::CompressCpuMonthly, 2592000i64),
                ] {
                    if let Err(e) =
                        rank::quota::add_usage(&client, user_id, kind, cpu_secs_delta, window).await
                    {
                        crate::log_ev!("filecompress", trace_id, "quota_settle", "kind" => kind.as_str(), "err" => format!("{e}"), "=>" => "fail");
                        crate::stats::record_error_global("filecompress", "quota_settle_failed").await;
                    }
                }
            }
        }
        crate::log_ev!("filecompress", trace_id, "quota_settled", "cpu_secs" => cpu_secs_used);

        crate::stats::record_event_user(
            user_id,
            "filecompress",
            config.fmt.as_str(),
            "ok",
            result.output_total_bytes as i64,
        )
        .await;
    }

    let input_fmt = fmt_bytes(result.input_total_bytes);
    let output_fmt = fmt_bytes(result.output_total_bytes);
    let reduction_pct = if result.input_total_bytes > 0 {
        ((1.0 - (result.output_total_bytes as f64 / result.input_total_bytes as f64)) * 100.0)
            .max(0.0)
    } else {
        0.0
    };

    let raw_report = tf(
        "fc.result_report",
        &[
            ("before", &escape_md(&input_fmt)),
            ("after", &escape_md(&output_fmt)),
            ("percent", &escape_md(&format!("{reduction_pct:.1}"))),
            ("cpu_time", &escape_md(&format!("{:.1}s", result.cpu_secs))),
        ],
    );

    let part_count = result.output_paths.len();

    for (idx, path) in result.output_paths.iter().enumerate() {
        let out_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if out_bytes == 0 || out_bytes > 2000 * 1024 * 1024 {
            crate::log_ev!("filecompress", trace_id, "output_oversized_or_missing", "bytes" => out_bytes);
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.send_failed")).await;
            break;
        }

        let raw_caption_text = if part_count > 1 {
            let part_label = tf(
                "fc.result_part_caption",
                &[
                    ("part", &(idx + 1).to_string()),
                    ("total", &part_count.to_string()),
                ],
            );
            format!(
                "{}\n\n{}\n{}",
                t("fc.result_caption"),
                raw_report,
                part_label
            )
        } else {
            format!("{}\n\n{}", t("fc.result_caption"), raw_report)
        };

        let part_caption = apply_premium_to_md(&raw_caption_text);
        let up_start = std::time::Instant::now();

        let params = SendDocumentParams::builder()
            .chat_id(chat_id)
            .document(PathBuf::from(path))
            .caption(&part_caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();

        use crate::bot::send_file_with_upload_ticker;
        if let Err(e) = send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            &api,
            "sendDocument",
            &params,
            std::path::Path::new(path),
            chat_id,
            progress_msg_id,
            "transfer.stage.sending_document",
            None,
        )
        .await
        {
            crate::log_ev!("filecompress", trace_id, "send_failed", "err" => format!("{e}"));
            let _ = send_text_with_back(&api, chat_id, &t("fc.error.send_failed")).await;
            break;
        }

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
                Some(part_count as i32),
            )
            .await;
        }
    }

    // Delete progress message
    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(progress_msg_id)
                .build(),
        )
        .await;

    std::fs::remove_dir_all(&work_dir).ok();

    // Re-arm flow with same config so user can send next batch immediately
    let upload_text = apply_premium_to_md(&t("fc.upload_prompt").replace("{count}", "0"));
    let send_res = api
        .send_message(
            &SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&upload_text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(ReplyMarkup::InlineKeyboardMarkup(done_inline_keyboard()))
                .build(),
        )
        .await;
    if let Ok(m) = send_res {
        flow_manager.set(
            user_id,
            FlowState::AwaitingCompressFiles {
                config: Box::new(config),
                files: Vec::new(),
                prompt_msg_id: m.result.message_id,
            },
        );
    }
}

fn escape_md(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '*' | '\\' | '_' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '='
            | '|' | '{' | '}' | '.' | '!' => format!("\\{c}"),
            other => other.to_string(),
        })
        .collect()
}
