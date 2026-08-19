//! Telegram request handling, paywall gating, input ingestion, and burn execution orchestration.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::Ordering,
};
use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, EditMessageTextParams, SendMessageParams, SendVideoParams},
    types::{Message, ReplyMarkup},
};

use crate::bot::files::download_telegram_file;
use crate::bot::send_text_md;
use crate::bot::transfer::send_file_with_upload_ticker;
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::rank::{effective_rank, paywall::block_feature, types::Rank};
use crate::stats::{record_error_global, record_event_global, record_event_user};
use crate::studio::compress::format_eta_hms;
use crate::studio::is_video_message_metadata;
use crate::studio::pipeline::{TempDirGuard, remove_active_job, spawn_download_ticker};
use crate::studio::trim;
use crate::validation::sanitize_filename;

use super::runner::{
    extract_thumbnail, read_log_tail, run_ffmpeg_burn, split_video_into_parts, upload_part_count,
};
use super::session::{
    BurnSession, SubtitleInputInfo, VideoInputInfo, abort_session, cancel_keyboard,
    job_cancel_keyboard, new_session, stop_download_ticker, try_claim_job,
};
use super::subtitle::{SubtitleFormat, build_filter_arg, convert_vtt_to_srt, detect_subtitle_format};
use super::{MAX_BURN_DURATION_SECS, MAX_UPLOAD_BYTES};

/// Enters the Hardsub Burn prompt, setting `FlowState::AwaitingStudioBurnInput`.
pub async fn enter_burn_prompt(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    log_actor_id!("studio_burn", trace_id, user_id, "clicked" => crate::bot::constants::CB_STUDIO_BURN);

    // Re-entering the prompt abandons whatever was half-uploaded before.
    if let FlowState::AwaitingStudioBurnInput { session } = flow_manager.get(user_id) {
        log_ev!("studio_burn", trace_id, "abort_previous_session", "user_id" => user_id);
        abort_session(&session);
        flow_manager.clear(user_id);
    }

    // Fail closed: without a DB the rank cannot be verified, so the feature stays locked.
    let Some(db) = database else {
        log_ev!("studio_burn", trace_id, "rank_check_unavailable", "=>" => "blocked");
        record_error_global("studio_burn", "rank check unavailable: no database").await;
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        return;
    };

    let rank = if let Ok(client) = db.get().await {
        effective_rank(&client, user_id).await
    } else {
        log_ev!("studio_burn", trace_id, "rank_check_checkout_failed", "=>" => "blocked");
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        return;
    };
    if rank.weight() < Rank::Esfandyar.weight() {
        log_ev!("studio_burn", trace_id, "paywall_blocked", "rank" => rank.as_str());
        record_event_user(user_id, "studio_burn", "paywall", "blocked", 1).await;
        record_event_global("studio_burn", "paywall", "blocked", 1).await;
        block_feature(
            api,
            chat_id,
            &t("studio.burn.feature_name"),
            Rank::Esfandyar,
        )
        .await;
        return;
    }

    if CpuBrokerGuard::is_user_busy(user_id).await {
        log_ev!("studio_burn", trace_id, "cpu_busy", "=>" => "blocked");
        let _ = send_text_md(api, chat_id, &t("active_job_running")).await;
        return;
    }

    let Some(session) = new_session(user_id, chat_id, message_id, trace_id) else {
        record_error_global("studio_burn", "work dir creation failed").await;
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        return;
    };

    flow_manager.set(user_id, FlowState::AwaitingStudioBurnInput { session });

    let text = apply_premium_to_md(&t("studio.burn.prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(cancel_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}

/// Re-arms the burn flow after a job ends: same prompt, same settings, as a new message.
pub async fn rearm_burn_prompt(api: &Bot, chat_id: i64, user_id: i64, flow_manager: &FlowManager) {
    let trace_id = next_trace_id();
    log_ev!("studio_burn", trace_id, "rearm", "user_id" => user_id);

    let text = apply_premium_to_md(&t("studio.burn.prompt"));
    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(cancel_keyboard()))
        .build();

    let sent = match api.send_message(&params).await {
        Ok(msg) => msg.result,
        Err(e) => {
            log_ev!("studio_burn", trace_id, "rearm_failed", "=>" => format!("fail err={e}"));
            flow_manager.clear(user_id);
            return;
        }
    };

    match new_session(user_id, chat_id, sent.message_id, trace_id) {
        Some(session) => flow_manager.set(user_id, FlowState::AwaitingStudioBurnInput { session }),
        None => {
            record_error_global("studio_burn", "rearm work dir creation failed").await;
            let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
            flow_manager.clear(user_id);
        }
    }
}

/// Deletes the status message, reports `text`, drops the job entry and re-arms the flow.
pub async fn finish_with_error(
    api: &Bot,
    flow_manager: &FlowManager,
    chat_id: i64,
    user_id: i64,
    status_msg_id: i32,
    text: &str,
) {
    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .build(),
        )
        .await;
    let _ = send_text_md(api, chat_id, text).await;
    remove_active_job(user_id);
    rearm_burn_prompt(api, chat_id, user_id, flow_manager).await;
}

/// Handles incoming video/document updates when flow state is `AwaitingStudioBurnInput`.
///
/// Subtitles are matched **first**: `is_video_message_metadata` treats unknown document mime
/// types as video, and Telegram sends `.srt` as `application/x-subrip`, so checking the video
/// branch first swallowed every subtitle upload.
pub async fn handle_input_message(
    api: &Bot,
    message: &Message,
    user_id: i64,
    session: Arc<Mutex<BurnSession>>,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
) {
    let trace_id = next_trace_id();
    let chat_id = message.chat.id;

    let sub_format = message
        .document
        .as_ref()
        .and_then(|doc| doc.file_name.as_deref().and_then(detect_subtitle_format));

    if let Some(fmt) = sub_format {
        handle_subtitle_input(
            api,
            message,
            user_id,
            fmt,
            session,
            flow_manager,
            database,
            trace_id,
        )
        .await;
        return;
    }

    if is_video_message_metadata(message) {
        handle_video_input(
            api,
            message,
            user_id,
            session,
            flow_manager,
            database,
            trace_id,
        )
        .await;
        return;
    }

    log_ev!("studio_burn", trace_id, "unsupported_input", "user_id" => user_id);
    let _ = send_text_md(api, chat_id, &t("studio.burn.error.unsupported_sub")).await;
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_video_input(
    api: &Bot,
    message: &Message,
    user_id: i64,
    session: Arc<Mutex<BurnSession>>,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
    trace_id: u64,
) {
    log_actor_id!("studio_burn", trace_id, user_id, "uploaded" => "video");
    let chat_id = message.chat.id;

    let file_id = message
        .video
        .as_ref()
        .map(|v| v.file_id.clone())
        .or_else(|| message.document.as_ref().map(|d| d.file_id.clone()))
        .unwrap_or_default();
    if file_id.is_empty() {
        log_ev!("studio_burn", trace_id, "video_missing_file_id", "=>" => "fail");
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        return;
    }

    let raw_name = message
        .video
        .as_ref()
        .and_then(|v| v.file_name.as_deref())
        .or_else(|| {
            message
                .document
                .as_ref()
                .and_then(|d| d.file_name.as_deref())
        })
        .unwrap_or("video.mp4");
    let display_name = {
        let cleaned = sanitize_filename(raw_name);
        if cleaned.is_empty() {
            "video.mp4".to_string()
        } else {
            cleaned
        }
    };

    let total_bytes = message
        .video
        .as_ref()
        .and_then(|v| v.file_size)
        .or_else(|| message.document.as_ref().and_then(|d| d.file_size))
        .unwrap_or(0);

    // First video wins: replacing one mid-download would leave two tickers on one message.
    let (status_msg_id, local_path) = {
        let Ok(mut s) = session.lock() else { return };
        if s.video_info.is_some() {
            log_ev!("studio_burn", trace_id, "duplicate_video_ignored", "user_id" => user_id);
            return;
        }
        let ext = Path::new(&display_name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4")
            .to_lowercase();
        let local_path = s.work_dir.join(format!("input.{ext}"));

        let dl_stop = spawn_download_ticker(
            api.clone(),
            chat_id,
            s.status_msg_id,
            local_path.clone(),
            total_bytes,
            "studio.burn",
            Some(s.cancel_flag.clone()),
        );
        s.dl_stop_flag = Some(dl_stop);
        s.video_info = Some(VideoInputInfo {
            display_name,
            total_bytes,
            local_path: local_path.clone(),
        });
        (s.status_msg_id, local_path)
    };

    let api_cl = api.clone();
    let session_cl = session.clone();
    let db_cl = database.clone();
    let fm_cl = flow_manager.clone();

    crate::app::spawn_user_task(async move {
        let dl_res = download_telegram_file(&api_cl, &file_id, &local_path).await;
        stop_download_ticker(&session_cl);

        let cancelled = session_cl
            .lock()
            .map(|s| s.cancel_flag.load(Ordering::Relaxed))
            .unwrap_or(true);
        if cancelled {
            log_ev!("studio_burn", trace_id, "download_cancelled", "user_id" => user_id);
            return;
        }

        if let Err(e) = dl_res {
            log_ev!("studio_burn", trace_id, "video_download_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("video download failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            let text = apply_premium_to_md(&t("studio.burn.error.download_failed"));
            abort_session(&session_cl);
            finish_with_error(&api_cl, &fm_cl, chat_id, user_id, status_msg_id, &text).await;
            return;
        }

        if let Ok(mut s) = session_cl.lock() {
            s.video_ready = true;
        }
        log_ev!("studio_burn", trace_id, "video_ready", "user_id" => user_id);

        if try_claim_job(&session_cl) {
            fm_cl.clear(user_id);
            execute_burn_job(&api_cl, session_cl, db_cl, fm_cl).await;
        } else {
            let text = apply_premium_to_md(&t("studio.burn.video_received_need_sub"));
            let params = EditMessageTextParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .text(&text)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(cancel_keyboard())
                .build();
            let _ = api_cl.edit_message_text(&params).await;
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub async fn handle_subtitle_input(
    api: &Bot,
    message: &Message,
    user_id: i64,
    fmt: SubtitleFormat,
    session: Arc<Mutex<BurnSession>>,
    flow_manager: &mut FlowManager,
    database: &Option<PostgresDatabase>,
    trace_id: u64,
) {
    log_actor_id!("studio_burn", trace_id, user_id, "uploaded" => "subtitle");
    let chat_id = message.chat.id;
    let Some(doc) = message.document.as_ref() else {
        return;
    };

    let (work_dir, status_msg_id) = {
        let Ok(s) = session.lock() else { return };
        (s.work_dir.clone(), s.status_msg_id)
    };
    let sub_path = work_dir.join(format!("sub.{}", fmt.ext()));

    if let Err(e) = download_telegram_file(api, &doc.file_id, &sub_path).await {
        log_ev!("studio_burn", trace_id, "sub_download_failed", "=>" => format!("fail err={e}"));
        record_error_global("studio_burn", format!("subtitle download failed: {e}")).await;
        let _ = send_text_md(api, chat_id, &t("studio.burn.error.download_failed")).await;
        return;
    }

    let replaced = {
        let Ok(mut s) = session.lock() else { return };
        let replaced = s.subtitle_info.is_some();
        s.subtitle_info = Some(SubtitleInputInfo {
            format: fmt,
            local_path: sub_path,
        });
        replaced
    };

    if try_claim_job(&session) {
        flow_manager.clear(user_id);
        let api_cl = api.clone();
        let db_cl = database.clone();
        let fm_cl = flow_manager.clone();
        crate::app::spawn_user_task(async move {
            execute_burn_job(&api_cl, session, db_cl, fm_cl).await;
        });
        return;
    }

    // Video absent, or still downloading — its waiter starts the job when it lands.
    let raw_text = if replaced {
        t("studio.burn.sub_replaced")
    } else {
        t("studio.burn.sub_received_need_video")
    };
    let video_downloading = session
        .lock()
        .map(|s| s.video_info.is_some() && !s.video_ready)
        .unwrap_or(false);
    if video_downloading {
        log_ev!("studio_burn", trace_id, "sub_stored_awaiting_download", "user_id" => user_id);
        return;
    }

    let text = apply_premium_to_md(&raw_text);
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(status_msg_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(cancel_keyboard())
        .build();
    let _ = api.edit_message_text(&params).await;
}

/// Executes the brokered ffmpeg burn: probe, cap, CPU broker, live ticker, upload, re-arm.
pub async fn execute_burn_job(
    api: &Bot,
    session: Arc<Mutex<BurnSession>>,
    _database: Option<PostgresDatabase>,
    flow_manager: FlowManager,
) {
    let (user_id, chat_id, status_msg_id, work_dir, video_info, sub_info, cancel_flag) = {
        let Ok(s) = session.lock() else { return };
        (
            s.user_id,
            s.chat_id,
            s.status_msg_id,
            s.work_dir.clone(),
            s.video_info.clone(),
            s.subtitle_info.clone(),
            s.cancel_flag.clone(),
        )
    };
    stop_download_ticker(&session);

    let _guard = TempDirGuard::new(work_dir.clone());
    let trace_id = next_trace_id();
    log_ev!("studio_burn", trace_id, "execute_start", "user_id" => user_id);

    macro_rules! fail {
        ($key:expr) => {{
            let text = apply_premium_to_md(&t($key));
            finish_with_error(api, &flow_manager, chat_id, user_id, status_msg_id, &text).await;
        }};
    }

    let (Some(v_info), Some(s_info)) = (video_info, sub_info) else {
        log_ev!("studio_burn", trace_id, "missing_inputs", "=>" => "fail");
        record_error_global("studio_burn", "job started without both inputs").await;
        fail!("studio.burn.error.burn_failed");
        return;
    };

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "pre_probe");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let video_bytes = std::fs::metadata(&v_info.local_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if video_bytes == 0 {
        log_ev!("studio_burn", trace_id, "video_missing", "expected_bytes" => v_info.total_bytes, "=>" => "fail");
        record_error_global("studio_burn", "downloaded video missing or empty").await;
        record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
        fail!("studio.burn.error.download_failed");
        return;
    }

    let meta = match trim::run_ffprobe(&v_info.local_path).await {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_burn", trace_id, "ffprobe_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("ffprobe failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            fail!("studio.burn.error.burn_failed");
            return;
        }
    };

    if meta.duration_secs > MAX_BURN_DURATION_SECS {
        log_ev!("studio_burn", trace_id, "too_long", "duration" => meta.duration_secs);
        record_event_user(user_id, "studio_burn", "burn", "too_long", 1).await;
        let text = apply_premium_to_md(&tf(
            "studio.burn.error.too_long",
            &[
                ("duration", &md_escape(&format_eta_hms(meta.duration_secs))),
                ("max", &md_escape(&format_eta_hms(MAX_BURN_DURATION_SECS))),
            ],
        ));
        finish_with_error(api, &flow_manager, chat_id, user_id, status_msg_id, &text).await;
        return;
    }

    if CpuBrokerGuard::is_user_busy(user_id).await {
        log_ev!("studio_burn", trace_id, "cpu_busy", "=>" => "blocked");
        fail!("active_job_running");
        return;
    }

    let final_sub_path = if s_info.format == SubtitleFormat::Vtt {
        let converted = work_dir.join("sub_converted.srt");
        if let Err(e) = convert_vtt_to_srt(&s_info.local_path, &converted).await {
            log_ev!("studio_burn", trace_id, "vtt_convert_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("vtt conversion failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            fail!("studio.burn.error.unsupported_sub");
            return;
        }
        converted
    } else {
        s_info.local_path.clone()
    };

    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "studio_burn").await;
    let threads_arg = if cpu_guard.cores().is_empty() {
        "2".to_string()
    } else {
        cpu_guard.cores().len().to_string()
    };

    // A user who cancels while queued must not lose the slot to a job that then runs anyway.
    if cancel_flag.load(Ordering::Relaxed) {
        cpu_guard.release().await;
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "post_acquire");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let filter_arg = build_filter_arg(s_info.format, &final_sub_path);
    let output_path = work_dir.join("output.mp4");
    let log_path = work_dir.join("ffmpeg.log");
    let display_stem = Path::new(&v_info.display_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("video")
        .to_string();
    let output_filename = format!("burned_{display_stem}.mp4");

    let job_start = Instant::now();
    let (tick_tx, mut tick_rx) = tokio::sync::watch::channel(String::new());
    let api_tick = api.clone();
    crate::app::spawn_user_task(async move {
        while tick_rx.changed().await.is_ok() {
            let text = tick_rx.borrow_and_update().clone();
            if text.is_empty() {
                continue;
            }
            let _ = api_tick
                .edit_message_text(
                    &EditMessageTextParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .text(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(job_cancel_keyboard())
                        .build(),
                )
                .await;
        }
    });

    let cores_cl = cpu_guard.cores().to_vec();
    let cancel_cl = cancel_flag.clone();
    let total_duration = meta.duration_secs.max(1);
    let ffmpeg_bin = crate::config::ffmpeg_path();
    let input_path = v_info.local_path.clone();
    let out_path_cl = output_path.clone();
    let log_path_cl = log_path.clone();
    let source_codec = meta.codec.clone();

    let burn_res = tokio::task::spawn_blocking(move || -> Result<(), String> {
        if !cores_cl.is_empty() {
            crate::moebius::cpu::pin_current_thread(&cores_cl, trace_id);
        }
        run_ffmpeg_burn(
            &ffmpeg_bin,
            &input_path,
            &filter_arg,
            &threads_arg,
            &source_codec,
            &out_path_cl,
            &log_path_cl,
            total_duration,
            job_start,
            &cancel_cl,
            tick_tx,
        )
    })
    .await;

    cpu_guard.release().await;

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "burn");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let burn_err = match burn_res {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        Err(e) => Some(format!("join error: {e}")),
    };
    if let Some(e) = burn_err {
        let tail = read_log_tail(&log_path);
        log_ev!("studio_burn", trace_id, "burn_failed", "=>" => format!("fail err={e} ffmpeg={tail}"));
        record_error_global("studio_burn", format!("burn failed: {e}; ffmpeg: {tail}")).await;
        record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
        fail!("studio.burn.error.burn_failed");
        return;
    }

    let output_bytes = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if output_bytes == 0 {
        let tail = read_log_tail(&log_path);
        log_ev!("studio_burn", trace_id, "output_empty", "=>" => format!("fail ffmpeg={tail}"));
        record_error_global("studio_burn", format!("empty output; ffmpeg: {tail}")).await;
        record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
        fail!("studio.burn.error.burn_failed");
        return;
    }
    let burn_duration_secs = job_start.elapsed().as_secs();
    log_ev!("studio_burn", trace_id, "burn_done", "secs" => burn_duration_secs, "bytes" => output_bytes);

    // Over the cap the output is cut into pieces instead of thrown away: a hardsub re-encode can
    // still cross 2000 MB even with the encoder matched to the source.
    let mut parts: Vec<PathBuf> = vec![output_path.clone()];
    if output_bytes > MAX_UPLOAD_BYTES {
        let part_count = upload_part_count(output_bytes, MAX_UPLOAD_BYTES);
        log_ev!("studio_burn", trace_id, "output_oversized", "bytes" => output_bytes, "parts" => part_count);

        let split_text = apply_premium_to_md(&tf(
            "studio.burn.status_splitting",
            &[("parts", &md_escape(&part_count.to_string()))],
        ));
        let _ = api
            .edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .text(&split_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(job_cancel_keyboard())
                    .build(),
            )
            .await;

        let ffmpeg_bin_split = crate::config::ffmpeg_path();
        let in_path = output_path.clone();
        let dir_cl = work_dir.to_path_buf();
        let split_res = tokio::task::spawn_blocking(move || {
            split_video_into_parts(
                &ffmpeg_bin_split,
                &in_path,
                &dir_cl,
                total_duration,
                part_count,
            )
        })
        .await;

        let split_parts = match split_res {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                log_ev!("studio_burn", trace_id, "split_failed", "=>" => format!("fail err={e}"));
                record_error_global("studio_burn", format!("split failed: {e}")).await;
                record_event_user(user_id, "studio_burn", "burn", "oversized", 1).await;
                fail!("studio.burn.error.oversized");
                return;
            }
            Err(e) => {
                log_ev!("studio_burn", trace_id, "split_failed", "=>" => format!("fail join={e}"));
                record_error_global("studio_burn", format!("split join failed: {e}")).await;
                record_event_user(user_id, "studio_burn", "burn", "oversized", 1).await;
                fail!("studio.burn.error.oversized");
                return;
            }
        };

        // Keyframe-aligned cuts are approximate; a piece still over the cap cannot be sent.
        if let Some(big) = split_parts
            .iter()
            .find(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > MAX_UPLOAD_BYTES)
        {
            log_ev!("studio_burn", trace_id, "split_part_oversized", "part" => big.display().to_string());
            record_event_user(user_id, "studio_burn", "burn", "oversized", 1).await;
            fail!("studio.burn.error.oversized");
            return;
        }

        record_event_user(user_id, "studio_burn", "burn", "split", 1).await;
        log_ev!("studio_burn", trace_id, "split_done", "parts" => split_parts.len());
        parts = split_parts;
    }

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_burn", trace_id, "cancelled", "stage" => "pre_upload");
        record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
        fail!("studio.burn.job_cancelled");
        return;
    }

    let total_parts = parts.len();
    let mut upload_err: Option<String> = None;

    for (idx, part_path) in parts.iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            log_ev!("studio_burn", trace_id, "cancelled", "stage" => "upload");
            record_event_user(user_id, "studio_burn", "burn", "cancelled", 1).await;
            fail!("studio.burn.job_cancelled");
            return;
        }

        let upload_text = apply_premium_to_md(&t("studio.burn.status_uploading"));
        let _ = api
            .edit_message_text(
                &EditMessageTextParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .text(&upload_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(job_cancel_keyboard())
                    .build(),
            )
            .await;

        let thumb_path = work_dir.join(format!("thumb_{idx}.jpg"));
        extract_thumbnail(part_path, &thumb_path).await;

        let caption = if total_parts > 1 {
            apply_premium_to_md(&tf(
                "studio.burn.job_done_part",
                &[
                    ("filename", &md_escape(&output_filename)),
                    ("burn_time", &md_escape(&format_eta_hms(burn_duration_secs))),
                    ("part", &md_escape(&(idx + 1).to_string())),
                    ("total", &md_escape(&total_parts.to_string())),
                ],
            ))
        } else {
            apply_premium_to_md(&tf(
                "studio.burn.job_done",
                &[
                    ("filename", &md_escape(&output_filename)),
                    ("burn_time", &md_escape(&format_eta_hms(burn_duration_secs))),
                ],
            ))
        };

        let mut params = SendVideoParams::builder()
            .chat_id(chat_id)
            .video(FileUpload::InputFile(InputFile {
                path: part_path.clone(),
            }))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .supports_streaming(true)
            .build();

        if let Ok(out_meta) = trim::run_ffprobe(part_path).await {
            if out_meta.width > 0 {
                params.width = Some(out_meta.width);
            }
            if out_meta.height > 0 {
                params.height = Some(out_meta.height);
            }
            if out_meta.duration_secs > 0 {
                params.duration = Some(out_meta.duration_secs as u32);
            }
        }
        if std::fs::metadata(&thumb_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            params.thumbnail = Some(FileUpload::InputFile(InputFile {
                path: thumb_path.clone(),
            }));
        }

        let send_res = send_file_with_upload_ticker::<_, Message>(
            api,
            "sendVideo",
            &params,
            part_path,
            chat_id,
            status_msg_id,
            "transfer.stage.sending_video",
            Some(cancel_flag.clone()),
        )
        .await;

        if let Err(send_error) = send_res {
            upload_err = Some(format!("part {}/{total_parts}: {send_error}", idx + 1));
            break;
        }
    }

    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .build(),
        )
        .await;

    match upload_err {
        None => {
            log_ev!("studio_burn", trace_id, "success", "burn_duration" => burn_duration_secs, "parts" => total_parts);
            record_event_user(user_id, "studio_burn", "burn_success", "ok", 1).await;
            record_event_global("studio_burn", "burn_success", "", 1).await;
        }
        Some(e) => {
            log_ev!("studio_burn", trace_id, "upload_failed", "=>" => format!("fail err={e}"));
            record_error_global("studio_burn", format!("upload failed: {e}")).await;
            record_event_user(user_id, "studio_burn", "burn", "fail", 1).await;
            let _ = send_text_md(api, chat_id, &t("studio.burn.error.burn_failed")).await;
        }
    }

    remove_active_job(user_id);
    rearm_burn_prompt(api, chat_id, user_id, &flow_manager).await;
}
