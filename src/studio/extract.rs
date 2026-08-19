//! Audio & Subtitle Stream Extraction module for Photo & Video Magic Studio (`studio_extract`).
//!
//! Provides container stream-copy extraction of embedded audio tracks and subtitle streams
//! into standalone losslessly-extracted files delivered directly to the user.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use frankenstein::{
    AsyncTelegramApi, ParseMode,
    client_reqwest::Bot,
    input_file::InputFile,
    methods::{
        DeleteMessageParams, EditMessageTextParams, SendAudioParams, SendDocumentParams,
        SendMessageParams,
    },
    types::{InlineKeyboardMarkup, Message, ReplyMarkup},
};

use crate::bot::constants::{
    CB_STUDIO_EXTRACT, CB_STUDIO_EXTRACT_CANCEL, CB_STUDIO_EXTRACT_JOBCANCEL,
};
use crate::bot::transfer::AsyncTelegramApiMetered;
use crate::common::cpu_broker::CpuBrokerGuard;
use crate::emoji::panel::btn_icon_danger;
use crate::emoji::{FlowManager, FlowState};
use crate::i18n::{apply_premium_to_md, md_escape, t, tf};
use crate::log::next_trace_id;
use crate::studio::pipeline::{
    TempDirGuard, job_guard, register_active_job, spawn_download_ticker,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamKind {
    Audio,
    Subtitle,
}

#[derive(Debug, Clone)]
pub struct ExtractedStreamInfo {
    pub index: usize,
    pub kind: StreamKind,
    #[allow(dead_code)]
    pub codec_name: String,
    pub language: Option<String>,
    #[allow(dead_code)]
    pub title: Option<String>,
    pub suggested_ext: String,
}

pub fn cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.extract.cancel_btn"),
            CB_STUDIO_EXTRACT_CANCEL,
            "cancel",
        )]])
        .build()
}

pub fn job_cancel_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::builder()
        .inline_keyboard(vec![vec![btn_icon_danger(
            &t("studio.extract.cancel_btn"),
            CB_STUDIO_EXTRACT_JOBCANCEL,
            "cancel",
        )]])
        .build()
}

/// Maps codec name and stream kind to standard output file extension.
pub fn map_codec_to_ext(kind: &StreamKind, codec_name: &str) -> &'static str {
    match kind {
        StreamKind::Subtitle => match codec_name.to_lowercase().as_str() {
            "subrip" | "srt" => "srt",
            "ass" | "ssa" => "ass",
            "webvtt" | "vtt" => "vtt",
            "hdmv_pgs_subtitle" | "pgs" => "sup",
            "dvd_subtitle" | "vobsub" => "sub",
            "mov_text" => "srt",
            _ => "srt",
        },
        StreamKind::Audio => match codec_name.to_lowercase().as_str() {
            "aac" => "m4a",
            "mp3" => "mp3",
            "flac" => "flac",
            "opus" => "opus",
            "vorbis" => "ogg",
            "ac3" => "ac3",
            "eac3" => "eac3",
            "dts" => "dts",
            "truehd" => "thd",
            "pcm_s16le" | "pcm_s24le" | "wav" => "wav",
            _ => "m4a",
        },
    }
}

/// Enters the Extract Subtitle & Audio prompt, setting `FlowState::AwaitingStudioExtractVideo`.
pub async fn enter_extract_prompt(
    api: &Bot,
    chat_id: i64,
    message_id: i32,
    user_id: i64,
    flow_manager: &FlowManager,
) {
    let trace_id = next_trace_id();
    flow_manager.set(user_id, FlowState::AwaitingStudioExtractVideo);
    log_actor_id!("studio_extract", trace_id, user_id, "clicked" => CB_STUDIO_EXTRACT);

    let text = apply_premium_to_md(&t("studio.extract.send_video_prompt"));
    let params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(message_id)
        .text(&text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(cancel_keyboard())
        .build();

    let _ = api.edit_message_text(&params).await;
}

/// Discovers audio and subtitle streams using `ffprobe`.
pub async fn probe_media_streams(video_path: &Path) -> anyhow::Result<Vec<ExtractedStreamInfo>> {
    let output = tokio::process::Command::new(crate::config::ffprobe_path())
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(video_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to execute ffprobe: {e}"))?;

    if !output.status.success() {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe stream discovery failed: {err_msg}");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let streams = json
        .get("streams")
        .and_then(|s| s.as_array())
        .ok_or_else(|| anyhow::anyhow!("invalid JSON output from ffprobe"))?;

    let mut result = Vec::new();

    for stream in streams {
        let codec_type = stream
            .get("codec_type")
            .and_then(|ct| ct.as_str())
            .unwrap_or("");

        let kind = match codec_type {
            "audio" => StreamKind::Audio,
            "subtitle" => StreamKind::Subtitle,
            _ => continue,
        };

        let index = stream
            .get("index")
            .and_then(|idx| idx.as_u64())
            .unwrap_or(0) as usize;

        let codec_name = stream
            .get("codec_name")
            .and_then(|cn| cn.as_str())
            .unwrap_or("unknown")
            .to_string();

        let tags = stream.get("tags");
        let language = tags
            .and_then(|t| t.get("language"))
            .and_then(|l| l.as_str())
            .map(|s| s.to_string());
        let title = tags
            .and_then(|t| t.get("title"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());

        let suggested_ext = map_codec_to_ext(&kind, &codec_name).to_string();

        result.push(ExtractedStreamInfo {
            index,
            kind,
            codec_name,
            language,
            title,
            suggested_ext,
        });
    }

    Ok(result)
}

/// Formats duration in H:MM:SS or M:SS format.
pub fn format_duration_hms(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Handles uploaded video message for stream extraction.
pub async fn handle_video_upload(
    api: &Bot,
    msg: Message,
    user_id: i64,
    trace_id: u64,
    flow_manager: &FlowManager,
) {
    let chat_id = msg.chat.id;
    let wall_start = Instant::now();

    log_actor_id!("studio_extract", trace_id, user_id, "upload_ingest" => "started");

    if !crate::studio::is_video_message_metadata(&msg) {
        log_ev!("studio_extract", trace_id, "not_a_video_metadata", "=>" => "fail");
        let _ =
            crate::bot::send_text_md(api, chat_id, &t("studio.extract.error.not_a_video")).await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    }

    // Anti-spam concurrency check
    if CpuBrokerGuard::is_user_busy(user_id).await {
        log_ev!("studio_extract", trace_id, "user_busy_blocked", "user_id" => user_id);
        let _ = crate::bot::send_text_md(api, chat_id, &t("moebius.active_job_running")).await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    }

    // Extract file_id, original filename, and size from video/document
    let (file_id, orig_filename, total_bytes) = if let Some(vid) = &msg.video {
        let name = vid
            .file_name
            .clone()
            .unwrap_or_else(|| "video.mp4".to_string());
        (vid.file_id.clone(), name, vid.file_size.unwrap_or(0))
    } else if let Some(doc) = &msg.document {
        let name = doc
            .file_name
            .clone()
            .unwrap_or_else(|| "video.mp4".to_string());
        (doc.file_id.clone(), name, doc.file_size.unwrap_or(0))
    } else {
        let _ =
            crate::bot::send_text_md(api, chat_id, &t("studio.extract.error.not_a_video")).await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    };

    // Prepare workspace
    let work_dir = std::env::temp_dir().join(format!("studio_strex_run_{trace_id}_{user_id}"));
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log_ev!("studio_extract", trace_id, "mkdir_failed", "=>" => format!("fail err={e}"));
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.extract.error.download_failed"))
            .await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    }
    let _guard = TempDirGuard::new(work_dir.clone());

    let cancel_flag = Arc::new(AtomicBool::new(false));
    register_active_job(user_id, cancel_flag.clone());
    let _job_guard = job_guard(user_id);

    // Send initial status message
    let initial_status_text = apply_premium_to_md(&tf(
        "studio.extract.status_downloading",
        &[("elapsed", "0s"), ("detail", "")],
    ));
    let status_params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(&initial_status_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboardMarkup(job_cancel_keyboard()))
        .build();

    let status_msg = match api.send_message(&status_params).await {
        Ok(m) => m,
        Err(e) => {
            log_ev!("studio_extract", trace_id, "status_send_failed", "=>" => format!("fail err={e}"));

            crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
            return;
        }
    };
    let status_msg_id = status_msg.result.message_id;

    let input_path = work_dir.join(&orig_filename);
    let stats_job_id = crate::stats::record_download_start(user_id, "studio_extract").await;

    // Spawn download ticker
    let stop_dl_ticker = spawn_download_ticker(
        api.clone(),
        chat_id,
        status_msg_id,
        input_path.clone(),
        total_bytes,
        "studio.extract",
        Some(cancel_flag.clone()),
    );

    // Download telegram file
    let dl_result = match crate::bot::files::download_telegram_file(api, &file_id, &input_path)
        .await
    {
        Ok(res) => res,
        Err(e) => {
            stop_dl_ticker.store(true, Ordering::Relaxed);
            log_ev!("studio_extract", trace_id, "download_failed", "=>" => format!("fail err={e}"));

            let _ = api
                .delete_message(
                    &DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .build(),
                )
                .await;
            let _ =
                crate::bot::send_text_md(api, chat_id, &t("studio.extract.error.download_failed"))
                    .await;
            crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
            return;
        }
    };
    stop_dl_ticker.store(true, Ordering::Relaxed);

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

        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.extract.job_cancelled")).await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    }

    // Update status to probing
    let probing_text = apply_premium_to_md(&t("studio.extract.status_probing"));
    let edit_params = EditMessageTextParams::builder()
        .chat_id(chat_id)
        .message_id(status_msg_id)
        .text(&probing_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(job_cancel_keyboard())
        .build();
    let _ = api.edit_message_text(&edit_params).await;

    // Discover media streams
    let streams = match probe_media_streams(&input_path).await {
        Ok(s) => s,
        Err(e) => {
            log_ev!("studio_extract", trace_id, "ffprobe_failed", "=>" => format!("fail err={e}"));

            let _ = api
                .delete_message(
                    &DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .build(),
                )
                .await;
            let _ =
                crate::bot::send_text_md(api, chat_id, &t("studio.extract.error.extract_failed"))
                    .await;
            crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
            return;
        }
    };

    if streams.is_empty() {
        log_ev!("studio_extract", trace_id, "no_extractable_streams", "user_id" => user_id);

        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.extract.error.no_streams")).await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    }

    let audio_count = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio)
        .count();
    let sub_count = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle)
        .count();

    log_ev!("studio_extract", trace_id, "streams_discovered", "audio" => audio_count, "sub" => sub_count);

    // Acquire CPU broker
    let mut cpu_guard = CpuBrokerGuard::acquire(user_id, trace_id, "studio_extract").await;

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
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.extract.job_cancelled")).await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    }

    // Extraction processing ticker
    let extract_start = Instant::now();
    let stop_extract_ticker = Arc::new(AtomicBool::new(false));
    {
        let stop_inner = stop_extract_ticker.clone();
        let api_inner = api.clone();
        let cancel_inner = cancel_flag.clone();
        crate::app::spawn_user_task(async move {
            let mut last_rendered = String::new();
            while !stop_inner.load(Ordering::Relaxed) && !cancel_inner.load(Ordering::Relaxed) {
                let elapsed = extract_start.elapsed().as_secs();
                let elapsed_str = format!("{elapsed}s");
                if elapsed_str != last_rendered {
                    last_rendered = elapsed_str.clone();
                    let raw = tf(
                        "studio.extract.status_extracting",
                        &[("elapsed", &md_escape(&elapsed_str))],
                    );
                    let text = apply_premium_to_md(&raw);
                    let edit = EditMessageTextParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .text(&text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(job_cancel_keyboard())
                        .build();
                    let _ = api_inner.edit_message_text(&edit).await;
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }

    // Run stream extraction pass via ffmpeg
    let input_path_inner = input_path.clone();
    let work_dir_inner = work_dir.clone();
    let streams_inner = streams.clone();
    let cancel_flag_inner = cancel_flag.clone();
    let stem_name = Path::new(&orig_filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("media")
        .to_string();

    struct PreparedOutput {
        info: ExtractedStreamInfo,
        out_path: PathBuf,
        out_filename: String,
    }

    let mut outputs_to_extract = Vec::new();
    let mut audio_seq = 0usize;
    let mut sub_seq = 0usize;

    for info in &streams_inner {
        let (seq, prefix) = match info.kind {
            StreamKind::Audio => {
                audio_seq += 1;
                (audio_seq, "audio")
            }
            StreamKind::Subtitle => {
                sub_seq += 1;
                (sub_seq, "sub")
            }
        };

        let lang_tag = info.language.as_deref().unwrap_or("");
        let lang_part = if !lang_tag.is_empty() {
            format!("_{lang_tag}")
        } else {
            String::new()
        };

        let out_filename = format!(
            "{stem_name}_{prefix}_{seq}{lang_part}.{}",
            info.suggested_ext
        );
        let out_path = work_dir_inner.join(&out_filename);

        outputs_to_extract.push(PreparedOutput {
            info: info.clone(),
            out_path,
            out_filename,
        });
    }

    let cores_for_thread = cpu_guard.cores().to_vec();
    let extract_res = tokio::task::spawn_blocking(move || {
        if !cores_for_thread.is_empty() {
            crate::moebius::cpu::pin_current_thread(&cores_for_thread, trace_id);
        }

        let mut ffmpeg_args = vec![
            "-v".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            input_path_inner.to_str().unwrap_or_default().to_string(),
        ];

        for item in &outputs_to_extract {
            ffmpeg_args.push("-map".to_string());
            ffmpeg_args.push(format!("0:{}", item.info.index));
            ffmpeg_args.push("-c".to_string());
            ffmpeg_args.push("copy".to_string());
            ffmpeg_args.push(item.out_path.to_str().unwrap_or_default().to_string());
        }

        let mut child = match std::process::Command::new(crate::config::ffmpeg_path())
            .args(&ffmpeg_args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return Err(anyhow::anyhow!("ffmpeg stream copy spawn error: {e}")),
        };

        while !cancel_flag_inner.load(Ordering::Relaxed) {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return Err(anyhow::anyhow!(
                            "ffmpeg stream copy failed with exit status {status}"
                        ));
                    }
                    break;
                }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
                Err(e) => return Err(anyhow::anyhow!("error waiting on ffmpeg: {e}")),
            }
        }

        if cancel_flag_inner.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow::anyhow!("job cancelled by user"));
        }

        Ok(outputs_to_extract)
    })
    .await;

    stop_extract_ticker.store(true, Ordering::Relaxed);
    cpu_guard.release().await;

    let extracted_outputs = match extract_res {
        Ok(Ok(outs)) => outs,
        Ok(Err(e)) => {
            log_ev!("studio_extract", trace_id, "extraction_error", "=>" => format!("fail err={e}"));

            let _ = api
                .delete_message(
                    &DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .build(),
                )
                .await;
            if cancel_flag.load(Ordering::Relaxed) {
                let _ = crate::bot::send_text_md(api, chat_id, &t("studio.extract.job_cancelled"))
                    .await;
            } else {
                let _ = crate::bot::send_text_md(
                    api,
                    chat_id,
                    &t("studio.extract.error.extract_failed"),
                )
                .await;
            }
            crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
            return;
        }
        Err(e) => {
            log_ev!("studio_extract", trace_id, "join_error", "=>" => format!("fail err={e}"));

            let _ = api
                .delete_message(
                    &DeleteMessageParams::builder()
                        .chat_id(chat_id)
                        .message_id(status_msg_id)
                        .build(),
                )
                .await;
            let _ =
                crate::bot::send_text_md(api, chat_id, &t("studio.extract.error.extract_failed"))
                    .await;
            crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
            return;
        }
    };

    if cancel_flag.load(Ordering::Relaxed) {
        let _ = api
            .delete_message(
                &DeleteMessageParams::builder()
                    .chat_id(chat_id)
                    .message_id(status_msg_id)
                    .build(),
            )
            .await;
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.extract.job_cancelled")).await;
        crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
        return;
    }

    // Upload extracted stream files sequentially
    let total_outputs = extracted_outputs.len();
    let mut sent_audio_count = 0usize;
    let mut sent_sub_count = 0usize;

    for (idx, item) in extracted_outputs.into_iter().enumerate() {
        if cancel_flag.load(Ordering::Relaxed) {
            break;
        }

        if !item.out_path.exists()
            || std::fs::metadata(&item.out_path)
                .map(|m| m.len() == 0)
                .unwrap_or(true)
        {
            log_ev!("studio_extract", trace_id, "skip_empty_file", "path" => item.out_filename);
            continue;
        }

        // Update upload stage status ticker
        let upload_status_text = apply_premium_to_md(&tf(
            "studio.extract.status_uploading",
            &[
                ("current", &(idx + 1).to_string()),
                ("total", &total_outputs.to_string()),
                ("filename", &md_escape(&item.out_filename)),
            ],
        ));
        let edit_params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(status_msg_id)
            .text(&upload_status_text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(job_cancel_keyboard())
            .build();
        let _ = api.edit_message_text(&edit_params).await;

        let lang_info = if let Some(lang) = &item.info.language {
            format!("\\({}\\)", md_escape(lang))
        } else {
            String::new()
        };

        match item.info.kind {
            StreamKind::Subtitle => {
                sent_sub_count += 1;
                let cap = tf(
                    "studio.extract.caption_sub",
                    &[
                        ("num", &sent_sub_count.to_string()),
                        ("lang_info", &lang_info),
                    ],
                );
                let cap_text = apply_premium_to_md(&cap);

                let doc_params = SendDocumentParams::builder()
                    .chat_id(chat_id)
                    .document(InputFile::from(item.out_path))
                    .caption(&cap_text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .build();

                let _ = api.send_document_metered(&doc_params).await;
            }
            StreamKind::Audio => {
                sent_audio_count += 1;
                let cap = tf(
                    "studio.extract.caption_audio",
                    &[
                        ("num", &sent_audio_count.to_string()),
                        ("lang_info", &lang_info),
                    ],
                );
                let cap_text = apply_premium_to_md(&cap);

                let is_playable = matches!(
                    item.info.suggested_ext.as_str(),
                    "mp3" | "m4a" | "flac" | "opus" | "ogg"
                );

                if is_playable {
                    let audio_params = SendAudioParams::builder()
                        .chat_id(chat_id)
                        .audio(InputFile::from(item.out_path))
                        .caption(&cap_text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .build();

                    let _ = api.send_audio_metered(&audio_params).await;
                } else {
                    let doc_params = SendDocumentParams::builder()
                        .chat_id(chat_id)
                        .document(InputFile::from(item.out_path))
                        .caption(&cap_text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .build();

                    let _ = api.send_document_metered(&doc_params).await;
                }
            }
        }

        crate::stats::record_event_user(user_id, "studio_extract", "file_sent", "ok", 1).await;
    }

    let _ = api
        .delete_message(
            &DeleteMessageParams::builder()
                .chat_id(chat_id)
                .message_id(status_msg_id)
                .build(),
        )
        .await;

    if cancel_flag.load(Ordering::Relaxed) {
        let _ = crate::bot::send_text_md(api, chat_id, &t("studio.extract.job_cancelled")).await;
    } else {
        let total_secs = wall_start.elapsed().as_secs();
        let total_time_str = format_duration_hms(total_secs);
        let done_raw = tf(
            "studio.extract.job_done",
            &[
                ("audio_count", &sent_audio_count.to_string()),
                ("sub_count", &sent_sub_count.to_string()),
                ("total_time", &md_escape(&total_time_str)),
            ],
        );
        let done_text = apply_premium_to_md(&done_raw);
        let _ = crate::bot::send_text_md(api, chat_id, &done_text).await;
        crate::stats::record_event_user(user_id, "studio_extract", "success", "ok", 1).await;
    }

    crate::studio::send_studio_menu_new_msg(api, chat_id, user_id, flow_manager).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_codec_to_ext() {
        assert_eq!(map_codec_to_ext(&StreamKind::Subtitle, "subrip"), "srt");
        assert_eq!(map_codec_to_ext(&StreamKind::Subtitle, "ass"), "ass");
        assert_eq!(map_codec_to_ext(&StreamKind::Subtitle, "webvtt"), "vtt");
        assert_eq!(
            map_codec_to_ext(&StreamKind::Subtitle, "hdmv_pgs_subtitle"),
            "sup"
        );
        assert_eq!(
            map_codec_to_ext(&StreamKind::Subtitle, "dvd_subtitle"),
            "sub"
        );

        assert_eq!(map_codec_to_ext(&StreamKind::Audio, "aac"), "m4a");
        assert_eq!(map_codec_to_ext(&StreamKind::Audio, "mp3"), "mp3");
        assert_eq!(map_codec_to_ext(&StreamKind::Audio, "flac"), "flac");
        assert_eq!(map_codec_to_ext(&StreamKind::Audio, "opus"), "opus");
        assert_eq!(map_codec_to_ext(&StreamKind::Audio, "ac3"), "ac3");
        assert_eq!(map_codec_to_ext(&StreamKind::Audio, "dts"), "dts");
    }

    #[test]
    fn test_format_duration_hms() {
        assert_eq!(format_duration_hms(45), "00:45");
        assert_eq!(format_duration_hms(125), "02:05");
        assert_eq!(format_duration_hms(3661), "01:01:01");
    }
}
