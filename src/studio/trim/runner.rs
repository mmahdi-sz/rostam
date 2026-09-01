use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{DeleteMessageParams, EditMessageTextParams, SendVideoParams},
};

use crate::bot::{
    files::download_telegram_file,
    messaging::{send_text_md, send_text_md_with_keyboard},
};
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::rank::quota::add_traffic;
use crate::stats::record_event_user;
use crate::studio::pipeline::{
    TempDirGuard, job_guard, register_active_job, spawn_download_ticker,
};

use super::handle::{cancel_keyboard, job_cancel_keyboard};
use super::probe::run_ffprobe;
use super::range::{CutRange, format_timestamp};

/// Executes brokered ffmpeg multi-cut job with ticker, cancel flag, and re-arm.
pub async fn execute_trim_job(
    api: &Bot,
    chat_id: i64,
    user_id: i64,
    file_id: &str,
    filename: &str,
    duration_secs: u64,
    ranges: Vec<CutRange>,
    flow_manager: &FlowManager,
    database: Option<PostgresDatabase>,
) {
    if CpuBrokerGuard::is_user_busy(user_id).await {
        let _ = crate::bot::send_text_md(api, chat_id, &t("active_job_running")).await;
        return;
    }

    let trace_id = next_trace_id();
    log_ev!("studio_trim", trace_id, "execute_start", "ranges_count" => ranges.len());

    let wall_start = Instant::now(); // measures total wall time including download

    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_active_job(user_id, cancel_flag.clone());
    let _job_guard = job_guard(user_id);

    // 1. Initial ticker message
    let status_raw = tf(
        "studio.trim.status_downloading",
        &[("elapsed", &md_escape("0s")), ("detail", "")],
    );
    let initial_status = apply_premium_to_md(&status_raw);
    let params = frankenstein::methods::SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&initial_status)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(frankenstein::types::ReplyMarkup::InlineKeyboardMarkup(
            job_cancel_keyboard(),
        ))
        .build();

    let status_msg_id = match api.send_message(&params).await {
        Ok(m) => m.result.message_id,
        Err(_) => 0,
    };

    let work_dir = std::env::temp_dir().join(format!("studio_trim_run_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_trim", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        let _ = send_text_md(api, chat_id, &t("studio.trim.error.trim_failed")).await;
        return;
    }
    let _guard = TempDirGuard::new(work_dir.clone());

    let source_video = work_dir.join(filename);
    let stats_job_id = crate::stats::record_download_start(user_id, "studio_trim").await;

    // Ingest source video
    let dl_stop_flag = spawn_download_ticker(
        api.clone(),
        chat_id,
        status_msg_id,
        source_video.clone(),
        0,
        "studio.trim",
        Some(cancel_flag.clone()),
    );

    let dl_result = match download_telegram_file(api, file_id, &source_video).await {
        Ok(res) => {
            dl_stop_flag.store(true, Ordering::Relaxed);
            res
        }
        Err(e) => {
            dl_stop_flag.store(true, Ordering::Relaxed);
            log_ev!("studio_trim", trace_id, "download_failed", "=>" => format!("fail err={e}"));
            let _ = api
                .delete_message(
                    &DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .build(),
                )
                .await;
            let _ = send_text_md(api, chat_id, &t("studio.trim.error.download_failed")).await;
            return;
        }
    };

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

    if cancel_flag.load(Ordering::Relaxed) {
        log_ev!("studio_trim", trace_id, "job_cancelled_post_dl", "user_id" => user_id);
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.job_cancelled")).await;
        return;
    }

    // Probe input media metadata for thumbnail dimensioning
    let _meta = run_ffprobe(&source_video).await;

    // Acquire CPU broker cores
    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "studio_trim").await;
    let threads_arg = if !cpu_guard.cores().is_empty() {
        cpu_guard.cores().len().to_string()
    } else {
        "2".to_string()
    };

    if cancel_flag.load(Ordering::Relaxed) {
        cpu_guard.release().await;

        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = send_text_md(api, chat_id, &t("studio.trim.job_cancelled")).await;
        return;
    }

    let bot_username = crate::config::bot_username().to_string();

    // download_secs = time from very beginning to after download+acquire_cpu
    let download_secs = wall_start.elapsed().as_secs();
    let job_start = Instant::now(); // ticker counts from here
    let total_ranges = ranges.len();
    let mut total_upload_secs = 0u64;

    // Single outer live ticker — runs across all cuts
    let stop_ticker = Arc::new(AtomicBool::new(false));
    let current_cut = Arc::new(std::sync::atomic::AtomicUsize::new(1));
    {
        let stop_ticker_inner = stop_ticker.clone();
        let current_cut_inner = current_cut.clone();
        let api_inner = api.clone();
        crate::app::spawn_user_task(async move {
            let mut last_rendered = String::new();
            while !stop_ticker_inner.load(Ordering::Relaxed) {
                let elapsed_secs = job_start.elapsed().as_secs();
                let cur = current_cut_inner.load(Ordering::Relaxed);
                let elapsed_str = format!("{elapsed_secs}s");
                let text_key = format!("{cur}:{total_ranges}:{elapsed_secs}");
                if text_key != last_rendered {
                    last_rendered = text_key;
                    // ETA: avg secs per cut so far * remaining cuts
                    let completed = cur.saturating_sub(1);
                    let eta_param = if completed > 0 && cur <= total_ranges {
                        let avg = elapsed_secs as f64 / completed as f64;
                        let remaining = total_ranges - completed;
                        let eta_secs = (avg * remaining as f64) as u64;
                        let eta_str = format!("{eta_secs}s");
                        tf(
                            "studio.trim.status_job_ticker_eta",
                            &[("eta", &md_escape(&eta_str))],
                        )
                    } else {
                        String::new()
                    };
                    let raw_ticker = tf(
                        "studio.trim.status_job_ticker",
                        &[
                            ("current", &cur.to_string()),
                            ("total", &total_ranges.to_string()),
                            ("elapsed", &md_escape(&elapsed_str)),
                            ("eta", &eta_param),
                        ],
                    );
                    let ticker_text = apply_premium_to_md(&raw_ticker);
                    if let Err(e) = api_inner
                        .edit_message_text(
                            &EditMessageTextParams::builder()
                                .chat_id(chat_id)
                                .message_id(status_msg_id)
                                .text(&ticker_text)
                                .parse_mode(ParseMode::MarkdownV2)
                                .reply_markup(job_cancel_keyboard())
                                .build(),
                        )
                        .await
                    {
                        log_ev!("studio_trim", trace_id, "ticker_edit_failed", "cur" => cur, "elapsed" => elapsed_secs, "=>" => format!("err={e}"));
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    }

    let loop_start = Instant::now();

    for (idx, range) in ranges.into_iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            log_ev!("studio_trim", trace_id, "job_cancelled_loop", "idx" => idx);
            break;
        }
        current_cut.store(idx + 1, Ordering::Relaxed);

        let output_name = format!("cut_{}_{}.mp4", idx + 1, trace_id);
        let output_path = work_dir.join(&output_name);

        let source_path = source_video.clone();
        let start_ss = format_timestamp(range.start_secs);
        let end_to = format_timestamp(range.end_secs);

        let raw_line = range.raw_line.clone();
        let cancel_flag_inner = cancel_flag.clone();
        let cores_inner = cpu_guard.cores().to_vec();
        let threads_arg_inner = threads_arg.clone();
        let out_path_inner = output_path.clone();
        let work_dir_inner = work_dir.clone();
        let current_idx = idx + 1;

        let run_res = tokio::task::spawn_blocking(move || {
            if !cores_inner.is_empty() {
                crate::moebius::cpu::pin_current_thread(&cores_inner, trace_id);
            }

            // Fast copy attempt
            let mut child = match std::process::Command::new(crate::config::ffmpeg_path())
                .args([
                    "-y",
                    "-ss",
                    &start_ss,
                    "-to",
                    &end_to,
                    "-i",
                    source_path.to_str().unwrap_or_default(),
                    "-c",
                    "copy",
                    "-avoid_negative_ts",
                    "make_zero",
                    "-movflags",
                    "+faststart",
                    "-threads",
                    &threads_arg_inner,
                    out_path_inner.to_str().unwrap_or_default(),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => return Err(anyhow::anyhow!("ffmpeg copy spawn error: {e}")),
            };

            let mut success = false;
            while !cancel_flag_inner.load(Ordering::Relaxed) {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        success = status.success()
                            && out_path_inner.exists()
                            && std::fs::metadata(&out_path_inner).map(|m| m.len() > 0).unwrap_or(false);
                        break;
                    }
                    Ok(None) => {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    Err(e) => return Err(anyhow::anyhow!("ffmpeg try_wait error: {e}")),
                }
            }

            if cancel_flag_inner.load(Ordering::Relaxed) {
                let _ = child.kill();
                return Ok(false);
            }

            if success {
                log_ev!("studio_trim", trace_id, "copy_success", "range" => &raw_line);
            } else {
                // Fallback to re-encode if copy failed
                log_ev!("studio_trim", trace_id, "copy_failed_fallback_encode", "range" => &raw_line);
                let mut encode_child = match std::process::Command::new(crate::config::ffmpeg_path())
                    .args([
                        "-y",
                        "-ss",
                        &start_ss,
                        "-to",
                        &end_to,
                        "-i",
                        source_path.to_str().unwrap_or_default(),
                        "-c:v",
                        "libx264",
                        "-c:a",
                        "aac",
                        "-preset",
                        "fast",
                        "-avoid_negative_ts",
                        "make_zero",
                        "-movflags",
                        "+faststart",
                        "-threads",
                        &threads_arg_inner,
                        out_path_inner.to_str().unwrap_or_default(),
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => return Err(anyhow::anyhow!("ffmpeg encode spawn error: {e}")),
                };

                while !cancel_flag_inner.load(Ordering::Relaxed) {
                    match encode_child.try_wait() {
                        Ok(Some(status)) => {
                            success = status.success()
                                && out_path_inner.exists()
                                && std::fs::metadata(&out_path_inner).map(|m| m.len() > 0).unwrap_or(false);
                            break;
                        }
                        Ok(None) => {
                            std::thread::sleep(Duration::from_millis(200));
                        }
                        Err(e) => return Err(anyhow::anyhow!("ffmpeg try_wait error: {e}")),
                    }
                }

                if cancel_flag_inner.load(Ordering::Relaxed) {
                    let _ = encode_child.kill();
                    return Ok(false);
                }
            }

            if success {
                let thumb_name = format!("thumb_{}_{}.jpg", current_idx, trace_id);
                let thumb_path = work_dir_inner.join(&thumb_name);
                let _ = std::process::Command::new(crate::config::ffmpeg_path())
                    .args([
                        "-y",
                        "-ss",
                        "00:00:00.500",
                        "-i",
                        out_path_inner.to_str().unwrap_or_default(),
                        "-vframes",
                        "1",
                        "-q:v",
                        "3",
                        thumb_path.to_str().unwrap_or_default(),
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }

            Ok(success)
        })
        .await;

        let trim_ok = matches!(run_res, Ok(Ok(true)));

        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        if !trim_ok {
            log_ev!("studio_trim", trace_id, "trim_range_failed", "range" => &range.raw_line);
            let err_text = apply_premium_to_md(&tf(
                "studio.trim.error.trim_failed",
                &[("range", &md_escape(&range.raw_line))],
            ));
            let _ = send_text_md(api, chat_id, &err_text).await;
            continue;
        }

        // Check file size limit (2GB Bot API ceiling)
        let file_size = std::fs::metadata(&output_path)
            .map(|m| m.len())
            .unwrap_or(0);
        if file_size > 2000 * 1024 * 1024 {
            log_ev!("studio_trim", trace_id, "file_oversized", "size" => file_size);
            let over_text = apply_premium_to_md(&tf(
                "studio.trim.error.oversized",
                &[("range", &md_escape(&range.raw_line))],
            ));
            let _ = send_text_md(api, chat_id, &over_text).await;
            continue;
        }

        // Caption uses actual clamped timestamps (not raw_line which may have user's unclamped end)
        let caption_range = format!(
            "{} - {}",
            format_timestamp(range.start_secs),
            format_timestamp(range.end_secs)
        );
        let caption = apply_premium_to_md(&format!(
            "{}\n@{}",
            md_escape(&caption_range),
            md_escape(&bot_username)
        ));

        let clip_duration = range.end_secs.saturating_sub(range.start_secs);
        let thumb_path = work_dir.join(format!("thumb_{}_{}.jpg", idx + 1, trace_id));

        let upload_start = Instant::now();
        let mut send_video_params = SendVideoParams::builder()
            .chat_id(chat_id)
            .video(FileUpload::InputFile(InputFile {
                path: output_path.clone(),
            }))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .supports_streaming(true)
            .build();

        if let Ok(out_meta) = run_ffprobe(&output_path).await {
            if out_meta.width > 0 {
                send_video_params.width = Some(out_meta.width as u32);
            }
            if out_meta.height > 0 {
                send_video_params.height = Some(out_meta.height as u32);
            }
        }
        if clip_duration > 0 {
            send_video_params.duration = Some(clip_duration as u32);
        }
        if thumb_path.exists()
            && std::fs::metadata(&thumb_path)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        {
            send_video_params.thumbnail =
                Some(FileUpload::InputFile(InputFile { path: thumb_path }));
        }

        use crate::bot::send_file_with_upload_ticker;
        match send_file_with_upload_ticker::<_, frankenstein::types::Message>(
            api,
            "sendVideo",
            &send_video_params,
            &output_path,
            chat_id,
            status_msg_id,
            "transfer.stage.sending_video",
            None,
        )
        .await
        {
            Ok(_) => {
                let up_secs = upload_start.elapsed().as_secs();
                let up_elapsed = upload_start.elapsed();
                let up_speed = if up_elapsed.as_secs_f64() > 0.0 {
                    file_size as f64 / up_elapsed.as_secs_f64()
                } else {
                    0.0
                };
                if let Some(jid) = stats_job_id {
                    crate::stats::record_upload_done(
                        jid,
                        user_id,
                        file_size as i64,
                        Some(up_speed as i64),
                        Some((idx + 1) as i32),
                    )
                    .await;
                }
                total_upload_secs += up_secs;
                log_ev!("studio_trim", trace_id, "trim_delivered", "range" => &range.raw_line, "upload_secs" => up_secs);
                if let Some(ref db) = database {
                    let first_up = now_epoch();
                    if let Ok(client) = db.get().await {
                        let _ = add_traffic(&client, user_id, file_size as i64, first_up).await;
                    }
                    record_event_user(user_id, "studio_trim", "trim", "ok", 1).await;
                }
            }
            Err(e) => {
                log_ev!("studio_trim", trace_id, "send_video_failed", "=>" => format!("fail err={e}"));
            }
        }

        // Update ticker to next cut for outer task
        current_cut.store(idx + 2, Ordering::Relaxed);
    }

    stop_ticker.store(true, Ordering::Relaxed);
    let loop_secs = loop_start.elapsed().as_secs();
    let trim_secs = loop_secs.saturating_sub(total_upload_secs);
    let upload_secs = total_upload_secs;

    cpu_guard.release().await;

    // Delete ticker message
    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .build(),
        )
        .await;

    if cancel_flag.load(Ordering::Relaxed) {
        let _ = send_text_md(api, chat_id, &t("studio.trim.job_cancelled")).await;
    } else {
        // Summary message (0.5s after last video)
        tokio::time::sleep(Duration::from_millis(500)).await;

        let fmt_dur = |s: u64| -> String {
            let m = s / 60;
            let sec = s % 60;
            if m > 0 {
                format!("{m} دقیقه و {sec} ثانیه")
            } else {
                format!("{sec} ثانیه")
            }
        };
        let raw_summary = tf(
            "studio.trim.job_done",
            &[
                ("trim_time", &md_escape(&fmt_dur(trim_secs))),
                ("download_time", &md_escape(&fmt_dur(download_secs))),
                ("upload_time", &md_escape(&fmt_dur(upload_secs))),
            ],
        );
        let summary_text = apply_premium_to_md(&raw_summary);
        let _ = send_text_md(api, chat_id, &summary_text).await;

        tokio::time::sleep(Duration::from_millis(500)).await;

        // Re-arm user back to AwaitingStudioTrimRanges state
        flow_manager.set(
            user_id,
            FlowState::AwaitingStudioTrimRanges {
                file_id: file_id.to_string(),
                filename: filename.to_string(),
                duration_secs,
            },
        );

        let rearm_text = apply_premium_to_md(&t("studio.trim.ranges_prompt"));
        let _ = send_text_md_with_keyboard(api, chat_id, &rearm_text, cancel_keyboard()).await;
    }
}

pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
