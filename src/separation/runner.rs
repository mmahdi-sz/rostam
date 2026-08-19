use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use frankenstein::client_reqwest::Bot;

use crate::common::ticker::ProgressTicker;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::FlowManager;
use crate::i18n::{apply_premium_to_md, t, tf};

use super::client::separate_audio;
use super::error::SeparationError;
use super::format::{delete_message, format_clock};
use super::keyboards::queue_cancel_keyboard;
use super::log_trace;
use super::quota::refund_quota;
use super::types::SeparationMode;
use super::upload::deliver_separation_results;

pub struct SeparationTaskParams {
    pub api: Bot,
    pub chat_id: i64,
    pub message_id: i32,
    pub user_id: i64,
    pub database: Option<PostgresDatabase>,
    pub flow_manager: FlowManager,
    pub audio_bytes: Vec<u8>,
    pub audio_filename: String,
    pub mode: SeparationMode,
    pub mode_label: &'static str,
    pub reserved: bool,
    pub reserve_secs: i64,
    pub audio_duration_secs: u64,
    pub tmp_dir: PathBuf,
    pub cancel_flag: Arc<AtomicBool>,
    pub stats_job_id: Option<i64>,
    pub trace_id: u64,
}

pub async fn run_separation_task(params: SeparationTaskParams) {
    let SeparationTaskParams {
        api,
        chat_id,
        message_id,
        user_id,
        database,
        flow_manager,
        audio_bytes,
        audio_filename,
        mode,
        mode_label,
        reserved,
        reserve_secs,
        audio_duration_secs,
        tmp_dir,
        cancel_flag,
        stats_job_id,
        trace_id,
    } = params;

    let eta_total = audio_duration_secs.saturating_mul(match mode {
        SeparationMode::Fast => 3,
        SeparationMode::Quality => 5,
    });

    let ticker_handle = ProgressTicker::new(&api, chat_id, message_id)
        .interval(Duration::from_secs(5))
        .with_cancel_flag(cancel_flag.clone())
        .with_keyboard(queue_cancel_keyboard())
        .spawn(move |elapsed| {
            let el_secs = elapsed.as_secs();
            let remaining = eta_total.saturating_sub(el_secs);
            let text = tf(
                "separation.progress",
                &[
                    ("elapsed", &format_clock(el_secs)),
                    ("remaining", &format_clock(remaining)),
                ],
            );
            Some(apply_premium_to_md(&text))
        });

    // Race separation against cancel signal (cancel aborts the HTTP request via drop).
    let op_started = std::time::Instant::now();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled2 = cancelled.clone();
    let cancel_check = cancel_flag.clone();
    let sep_result = tokio::select! {
        r = tokio::time::timeout(
            Duration::from_secs(35 * 60),
            separate_audio(audio_bytes, &audio_filename, mode, user_id, false),
        ) => {
            match r {
                Ok(r) => Some(r),
                Err(_) => None, // 35-min timeout
            }
        }
        _ = async move {
            loop {
                tokio::time::sleep(Duration::from_millis(300)).await;
                if cancel_check.load(Ordering::Relaxed) { break; }
            }
            cancelled2.store(true, Ordering::Relaxed);
        } => { None }
    };

    // Stop ticker now that we have a result.
    ticker_handle.stop();
    // The whole track sat in RAM twice (read buffer + multipart copy); hand
    // the pages back — separation talks to a remote service, so no broker
    // release happens here to do it.
    crate::moebius::cpu::trim_memory();

    // Clear flow directly here rather than via channel to avoid wiping re-armed state.
    flow_manager.clear(user_id);

    if cancelled.load(Ordering::Relaxed) {
        log_trace(trace_id, "cancelled_in_queue", "");
        std::fs::remove_dir_all(&tmp_dir).ok();
        refund_quota(&database, user_id, reserve_secs, reserved, trace_id, "cancelled_in_queue").await;
        return;
    }

    let result = match sep_result {
        None => {
            log_trace(trace_id, "queue_timeout", "");
            crate::stats::record_event_user(user_id, "cpu", "timeout", "separation", 0).await;
            crate::stats::record_event_user(user_id, "separation", mode_label, "timeout", 0).await;
            crate::stats::record_error_global("separation", "queue timeout (35min)").await;
            let _ = crate::bot::send_text_with_ai_back(
                &api,
                chat_id,
                &t("separation.error.queue_timeout"),
            )
            .await;
            let _ = delete_message(&api, chat_id, message_id).await;
            std::fs::remove_dir_all(&tmp_dir).ok();
            refund_quota(&database, user_id, reserve_secs, reserved, trace_id, "queue_timeout").await;
            return;
        }
        Some(r) => r,
    };

    match result {
        Ok(result) => {
            deliver_separation_results(
                &api,
                chat_id,
                message_id,
                user_id,
                &tmp_dir,
                result,
                stats_job_id,
                audio_duration_secs,
                mode_label,
                op_started,
                &flow_manager,
                trace_id,
            )
            .await;
        }
        Err(e) => {
            log_trace(trace_id, "separate_error", &format!("err={e}"));
            crate::stats::record_event_user(user_id, "separation", mode_label, "fail", 0).await;
            crate::metrics::get()
                .separation_requests_total
                .with_label_values(&["fail"])
                .inc();
            crate::stats::record_error_global(
                "separation",
                &format!("processing error: {e:?}"),
            )
            .await;
            let _ = delete_message(&api, chat_id, message_id).await;
            let key = match &e {
                SeparationError::ServiceUnavailable => "separation.error.service_unavailable",
                SeparationError::InvalidAudio => "separation.error.invalid_audio",
                SeparationError::Timeout => "separation.error.timeout",
                SeparationError::ProcessingFailed(_) => "separation.error.processing_failed",
            };
            let _ = crate::bot::send_text_with_ai_back(&api, chat_id, &t(key)).await;
            std::fs::remove_dir_all(&tmp_dir).ok();
            refund_quota(&database, user_id, reserve_secs, reserved, trace_id, "separate_error").await;
        }
    }
}
