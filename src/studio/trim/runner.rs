use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::{FileUpload, InputFile},
    methods::{
        DeleteMessageParams, EditMessageTextParams, SendAudioParams, SendDocumentParams,
        SendVideoParams,
    },
};

use crate::bot::{
    files::download_telegram_file,
    messaging::{send_text_md, send_text_md_with_keyboard},
};
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::common::format::fmt_secs;
use crate::database::postgresql::PostgresDatabase;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::rank::quota::add_traffic;
use crate::stats::record_event_user;
use crate::studio::pipeline::{
    TempDirGuard, job_guard, register_active_job, spawn_download_ticker,
};
use crate::studio::{is_audio_filename, is_telegram_audio_guaranteed};

use super::engine::trim_audio_clip;
use super::handle::{cancel_keyboard, job_cancel_keyboard};
use super::probe::{extract_cover_art, probe_audio_metadata, run_ffprobe};
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

    let is_audio = is_audio_filename(filename) || {
        let ext = source_video
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        matches!(
            ext.as_str(),
            "mp3" | "m4a" | "flac" | "wav" | "ogg" | "opus" | "aac" | "wma" | "alac" | "aiff"
        )
    };

    let (audio_meta, has_cover, cover_thumb_path, source_codec) = if is_audio {
        let meta = probe_audio_metadata(&source_video).await.ok();
        let thumb_p = work_dir.join(format!("cover_thumb_{trace_id}.jpg"));
        let cov = extract_cover_art(&source_video, &thumb_p)
            .await
            .unwrap_or(false);
        (meta, cov, thumb_p, "h264".to_string())
    } else {
        let video_meta = run_ffprobe(&source_video).await.ok();
        let codec = video_meta
            .map(|m| m.codec)
            .unwrap_or_else(|| "h264".to_string());
        (
            None,
            false,
            work_dir.join(format!("cover_thumb_{trace_id}.jpg")),
            codec,
        )
    };

    let original_title = audio_meta.as_ref().and_then(|m| m.title.clone());
    let original_artist = audio_meta.as_ref().and_then(|m| m.artist.clone());
    let base_stem = source_video
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string();
    let audio_ext = audio_meta
        .as_ref()
        .map(|m| m.ext.clone())
        .unwrap_or_else(|| {
            source_video
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("mp3")
                .to_string()
        });

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
    let ticker_task = {
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
        })
    };

    let loop_start = Instant::now();

    for (idx, range) in ranges.into_iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            log_ev!("studio_trim", trace_id, "job_cancelled_loop", "idx" => idx);
            break;
        }
        current_cut.store(idx + 1, Ordering::Relaxed);

        if is_audio {
            let cut_idx = idx + 1;
            let output_name = format!("{base_stem} - Part {cut_idx}.{audio_ext}");
            let output_path = work_dir.join(&output_name);

            let trim_res = trim_audio_clip(
                &source_video,
                &output_path,
                range.start_secs,
                range.end_secs,
                Some(cancel_flag.clone()),
                Some(&threads_arg),
                trace_id,
            )
            .await;

            if cancel_flag.load(Ordering::Relaxed) {
                break;
            }

            let actual_duration = match trim_res {
                Ok(res) => res.duration_secs,
                Err(e) => {
                    log_ev!(
                        "studio_trim",
                        trace_id,
                        "audio_trim_range_failed",
                        "range" => &range.raw_line,
                        "err" => format!("{e}")
                    );
                    let err_text = apply_premium_to_md(&tf(
                        "studio.trim.error.trim_failed",
                        &[("range", &md_escape(&range.raw_line))],
                    ));
                    let _ = send_text_md(api, chat_id, &err_text).await;
                    continue;
                }
            };

            let file_size = std::fs::metadata(&output_path)
                .map(|m| m.len())
                .unwrap_or(0);
            if file_size > 2000 * 1024 * 1024 {
                log_ev!("studio_trim", trace_id, "audio_oversized", "size" => file_size);
                let over_text = apply_premium_to_md(&tf(
                    "studio.trim.error.oversized",
                    &[("range", &md_escape(&range.raw_line))],
                ));
                let _ = send_text_md(api, chat_id, &over_text).await;
                continue;
            }

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

            let is_audio_guaranteed = is_telegram_audio_guaranteed(&audio_ext);

            let upload_start = Instant::now();

            let send_res = if is_audio_guaranteed {
                let track_title = original_title.clone().unwrap_or_else(|| base_stem.clone());
                let mut send_audio_params = SendAudioParams::builder()
                    .chat_id(chat_id)
                    .audio(FileUpload::InputFile(InputFile {
                        path: output_path.clone(),
                    }))
                    .caption(&caption)
                    .parse_mode(ParseMode::MarkdownV2)
                    .title(track_title)
                    .duration(actual_duration as u32)
                    .build();

                if let Some(ref perf) = original_artist {
                    send_audio_params.performer = Some(perf.clone());
                }
                if has_cover && cover_thumb_path.exists() {
                    send_audio_params.thumbnail = Some(FileUpload::InputFile(InputFile {
                        path: cover_thumb_path.clone(),
                    }));
                }

                crate::bot::transfer::send_file_with_upload_context::<_, frankenstein::types::Message>(
                    api, "sendAudio", &send_audio_params, &output_path,
                    crate::bot::transfer::UploadStatusContext {
                        chat_id, message_id: status_msg_id, file_key: "transfer.stage.sending_audio",
                        description: Some(&caption_range), part: Some((idx + 1, total_ranges)),
                        keyboard: Some(job_cancel_keyboard()),
                    }, Some(cancel_flag.clone()),
                    )
                .await
            } else {
                // Non-guaranteed audio formats (e.g. FLAC, WAV, OGG, OPUS, AIFF) are delivered
                // via sendDocument to guarantee lossless delivery without client playback failure or API rejection.
                let mut send_doc_params = SendDocumentParams::builder()
                    .chat_id(chat_id)
                    .document(FileUpload::InputFile(InputFile {
                        path: output_path.clone(),
                    }))
                    .caption(&caption)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build();

                if has_cover && cover_thumb_path.exists() {
                    send_doc_params.thumbnail = Some(FileUpload::InputFile(InputFile {
                        path: cover_thumb_path.clone(),
                    }));
                }

                crate::bot::transfer::send_file_with_upload_context::<_, frankenstein::types::Message>(
                    api, "sendDocument", &send_doc_params, &output_path,
                    crate::bot::transfer::UploadStatusContext {
                        chat_id, message_id: status_msg_id, file_key: "transfer.stage.sending_document",
                        description: Some(&caption_range), part: Some((idx + 1, total_ranges)),
                        keyboard: Some(job_cancel_keyboard()),
                    }, Some(cancel_flag.clone()),
                    )
                .await
            };

            match send_res {
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
                    log_ev!(
                        "studio_trim",
                        trace_id,
                        "audio_trim_delivered",
                        "range" => &range.raw_line,
                        "upload_secs" => up_secs
                    );
                    if let Some(ref db) = database {
                        let first_up = now_epoch();
                        if let Ok(client) = db.get().await {
                            let _ = add_traffic(&client, user_id, file_size as i64, first_up).await;
                        }
                        record_event_user(user_id, "studio_trim", "trim", "ok", 1).await;
                    }
                }
                Err(e) => {
                    log_ev!("studio_trim", trace_id, "send_audio_failed", "=>" => format!("fail err={e}"));
                }
            }
        } else {
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
            let source_codec_inner = source_codec.clone();
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
                    let v_args = crate::common::ffmpeg::video_encoder_args(&source_codec_inner);
                    let mut cmd = std::process::Command::new(crate::config::ffmpeg_path());
                    cmd.args([
                        "-y",
                        "-ss",
                        &start_ss,
                        "-to",
                        &end_to,
                        "-i",
                        source_path.to_str().unwrap_or_default(),
                    ]);
                    cmd.args(&v_args);
                    cmd.args([
                        "-c:a",
                        "aac",
                        "-avoid_negative_ts",
                        "make_zero",
                        "-movflags",
                        "+faststart",
                        "-threads",
                        &threads_arg_inner,
                        out_path_inner.to_str().unwrap_or_default(),
                    ]);
                    let mut encode_child = match cmd
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
                    let thumb_name = format!("thumb_{current_idx}_{trace_id}.jpg");
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

            match crate::bot::transfer::send_file_with_upload_context::<
                _,
                frankenstein::types::Message,
            >(
                api,
                "sendVideo",
                &send_video_params,
                &output_path,
                crate::bot::transfer::UploadStatusContext {
                    chat_id,
                    message_id: status_msg_id,
                    file_key: "transfer.stage.sending_video",
                    description: Some(&caption_range),
                    part: Some((idx + 1, total_ranges)),
                    keyboard: Some(job_cancel_keyboard()),
                },
                Some(cancel_flag.clone()),
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
        }

        // Update ticker to next cut for outer task
        current_cut.store(idx + 2, Ordering::Relaxed);
    }

    stop_ticker.store(true, Ordering::Relaxed);
    let _ = ticker_task.await;
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

        let raw_summary = tf(
            "studio.trim.job_done",
            &[
                ("trim_time", &md_escape(&fmt_secs(trim_secs as i64))),
                ("download_time", &md_escape(&fmt_secs(download_secs as i64))),
                ("upload_time", &md_escape(&fmt_secs(upload_secs as i64))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TestCleanupGuard(PathBuf);
    impl Drop for TestCleanupGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn create_test_dir(prefix: &str) -> (PathBuf, TestCleanupGuard) {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).expect("failed to create test dir");
        let guard = TestCleanupGuard(dir.clone());
        (dir, guard)
    }

    #[tokio::test]
    async fn test_audio_trim_delivery_params() {
        let (dir, _guard) = create_test_dir("runner_audio_test");
        let src_mp3 = dir.join("Queen - Bohemian Rhapsody.mp3");
        let cover_jpg = dir.join("cover.jpg");
        let raw_wav = dir.join("raw.wav");

        // 1. Generate cover image (400x400)
        let s1 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=purple:s=400x400:d=1",
                "-vframes",
                "1",
                cover_jpg.to_str().unwrap(),
            ])
            .status()
            .expect("create cover image");
        assert!(s1.success());

        // 2. Generate raw audio wav (6 seconds)
        let s2 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=6",
                raw_wav.to_str().unwrap(),
            ])
            .status()
            .expect("create raw wav");
        assert!(s2.success());

        // 3. Assemble MP3 with cover and tags
        let s3 = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                raw_wav.to_str().unwrap(),
                "-i",
                cover_jpg.to_str().unwrap(),
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:a",
                "libmp3lame",
                "-c:v",
                "copy",
                "-id3v2_version",
                "3",
                "-metadata",
                "title=Bohemian Rhapsody",
                "-metadata",
                "artist=Queen",
                "-metadata",
                "album=A Night at the Opera",
                src_mp3.to_str().unwrap(),
            ])
            .status()
            .expect("assemble mp3");
        assert!(s3.success());

        // 4. Test probing and extracting metadata as in execute_trim_job
        let meta = probe_audio_metadata(&src_mp3).await.expect("probe audio");
        assert_eq!(meta.title.as_deref(), Some("Bohemian Rhapsody"));
        assert_eq!(meta.artist.as_deref(), Some("Queen"));

        let thumb_p = dir.join("cover_thumb.jpg");
        let has_cover = extract_cover_art(&src_mp3, &thumb_p).await.unwrap_or(false);
        assert!(has_cover);
        assert!(thumb_p.exists());

        // 5. Trim cut using trim_audio_clip
        let out_cut = dir.join("Queen - Bohemian Rhapsody - Part 1.mp3");
        let res = trim_audio_clip(&src_mp3, &out_cut, 1, 4, None, None, 12345)
            .await
            .expect("trim audio clip");
        assert!((res.duration_secs as i64 - 3).abs() <= 1);
        assert!(out_cut.exists());

        // 6. Build SendAudioParams and verify all required properties
        let caption_range = format!("{} - {}", format_timestamp(1), format_timestamp(4));
        let bot_username = "test_bot";
        let caption = apply_premium_to_md(&format!(
            "{}\n@{}",
            md_escape(&caption_range),
            md_escape(bot_username)
        ));

        let track_title = meta.title.clone().unwrap_or_else(|| "audio".to_string());
        let mut send_audio_params = SendAudioParams::builder()
            .chat_id(123456)
            .audio(FileUpload::InputFile(InputFile {
                path: out_cut.clone(),
            }))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .title(track_title)
            .duration(res.duration_secs as u32)
            .build();

        if let Some(ref perf) = meta.artist {
            send_audio_params.performer = Some(perf.clone());
        }
        if has_cover && thumb_p.exists() {
            send_audio_params.thumbnail = Some(FileUpload::InputFile(InputFile {
                path: thumb_p.clone(),
            }));
        }

        // 7. Verify fields:
        assert_eq!(
            send_audio_params.title.as_deref(),
            Some("Bohemian Rhapsody")
        );
        assert_eq!(send_audio_params.performer.as_deref(), Some("Queen"));
        assert_eq!(send_audio_params.duration, Some(res.duration_secs as u32));
        assert!(send_audio_params.thumbnail.is_some());
        assert_eq!(send_audio_params.parse_mode, Some(ParseMode::MarkdownV2));
        assert!(
            send_audio_params
                .caption
                .as_ref()
                .unwrap()
                .contains("00:01")
        );
        assert!(
            send_audio_params
                .caption
                .as_ref()
                .unwrap()
                .contains("00:04")
        );
        assert!(
            send_audio_params
                .caption
                .as_ref()
                .unwrap()
                .contains("@test\\_bot")
        );

        // 8. Test delivery policy helper is_telegram_audio_guaranteed
        assert!(is_telegram_audio_guaranteed("mp3"));
        assert!(is_telegram_audio_guaranteed("m4a"));
        assert!(is_telegram_audio_guaranteed("MP3"));
        assert!(is_telegram_audio_guaranteed("M4A"));
        assert!(!is_telegram_audio_guaranteed("flac"));
        assert!(!is_telegram_audio_guaranteed("wav"));
        assert!(!is_telegram_audio_guaranteed("ogg"));
        assert!(!is_telegram_audio_guaranteed("opus"));
        assert!(!is_telegram_audio_guaranteed("aiff"));

        // 9. Test WAV non-guaranteed lossless delivery branch (SendDocument)
        let out_wav = dir.join("Queen - Bohemian Rhapsody - Part 1.wav");
        let wav_res = trim_audio_clip(&raw_wav, &out_wav, 0, 3, None, None, 12345)
            .await
            .expect("trim wav audio clip");
        assert_eq!(wav_res.duration_secs, 3);
        assert!(out_wav.exists());

        let wav_guaranteed = is_telegram_audio_guaranteed("wav");
        assert!(!wav_guaranteed, "WAV must not be guaranteed for sendAudio");

        let mut send_doc_params = SendDocumentParams::builder()
            .chat_id(123456)
            .document(FileUpload::InputFile(InputFile {
                path: out_wav.clone(),
            }))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        if has_cover && thumb_p.exists() {
            send_doc_params.thumbnail = Some(FileUpload::InputFile(InputFile {
                path: thumb_p.clone(),
            }));
        }

        assert_eq!(
            send_doc_params.chat_id,
            frankenstein::types::ChatId::Integer(123456)
        );
        assert!(send_doc_params.thumbnail.is_some());
        assert!(
            send_doc_params
                .caption
                .as_ref()
                .unwrap()
                .contains("@test\\_bot")
        );
        assert_eq!(send_doc_params.parse_mode, Some(ParseMode::MarkdownV2));

        // 10. Test FLAC non-guaranteed lossless delivery branch (SendDocument)
        let out_flac = dir.join("Queen - Bohemian Rhapsody - Part 1.flac");
        let flac_res = trim_audio_clip(&raw_wav, &out_flac, 0, 3, None, None, 12346)
            .await
            .expect("trim flac audio clip");
        assert_eq!(flac_res.duration_secs, 3);
        assert!(out_flac.exists());

        let flac_guaranteed = is_telegram_audio_guaranteed("flac");
        assert!(
            !flac_guaranteed,
            "FLAC must route to sendDocument for lossless delivery"
        );

        let mut flac_doc_params = SendDocumentParams::builder()
            .chat_id(123456)
            .document(FileUpload::InputFile(InputFile {
                path: out_flac.clone(),
            }))
            .caption(&caption)
            .parse_mode(ParseMode::MarkdownV2)
            .build();
        if has_cover && thumb_p.exists() {
            flac_doc_params.thumbnail = Some(FileUpload::InputFile(InputFile {
                path: thumb_p.clone(),
            }));
        }

        assert_eq!(
            flac_doc_params.chat_id,
            frankenstein::types::ChatId::Integer(123456)
        );
        assert!(flac_doc_params.thumbnail.is_some());
        assert!(
            flac_doc_params
                .caption
                .as_ref()
                .unwrap()
                .contains("@test\\_bot")
        );
        assert_eq!(flac_doc_params.parse_mode, Some(ParseMode::MarkdownV2));
    }
}
